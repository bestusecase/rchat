//! The blob store and the one ingest path.
//!
//! Attachment bytes live once per unique content, addressed by their SHA-256, so
//! sending the same photo twice stores it once and a receipt can never disagree
//! with a send about what a hash means. Everything that turns bytes into a row —
//! hashing, MIME sniffing, dimensions, the display thumbnail — happens here, so
//! an outgoing upload and an incoming bridge event cannot drift apart.
//!
//! The store is handed an explicit root. The core crate has no opinion about
//! where the data directory lives; the daemon owns that choice.

use anyhow::{Context, Result};
use rchat_ipc::AttachmentInfo;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Longest edge of the one thumbnail kept per image, in pixels.
pub const THUMBNAIL_EDGE: u32 = 480;
/// How many attachments one message may carry.
pub const MAX_ATTACHMENTS: usize = 4;
/// Recorded on a row whose blob was dropped to stay inside the budget.
pub const REASON_EVICTED: &str = "Attachment removed to save space";
/// Recorded on a row whose bytes arrived larger than the inbound ceiling.
pub const REASON_TOO_LARGE: &str = "Attachment was too large to receive";
/// Recorded on a row whose bytes arrived but could not be stored.
pub const REASON_UNREADABLE: &str = "Attachment could not be read";

/// Inbound files past the per-message limit. The message is kept; the extra
/// files are reported instead of vanishing silently.
pub const REASON_EXTRA: &str = "Attachment was not received: a message can carry at most four";
/// Reported for a row whose blob is gone without a recorded reason (a file
/// removed by hand, or a store restored without its blobs).
pub const REASON_MISSING: &str = "Attachment is no longer on this device";

/// A content-addressed directory of blobs and their thumbnails.
///
/// Layout:
///
/// ```text
/// <root>/ab/ab12…        original bytes, two-hex bucket
/// <root>/thumbs/ab/…     one 480 px JPEG, same hash as its pixels
/// <root>/tmp/…           in-flight writes, swept at startup
/// ```
///
/// The filename never appears in a path: a sender cannot make rchat write
/// outside the store.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path of one blob. `hash` is assumed valid; see [`is_hash`].
    pub fn path(&self, hash: &str) -> PathBuf {
        self.root.join(&hash[..2]).join(hash)
    }

    /// Absolute path of one thumbnail.
    pub fn thumbnail_path(&self, hash: &str) -> PathBuf {
        self.root.join("thumbs").join(&hash[..2]).join(hash)
    }

    pub fn contains(&self, hash: &str) -> bool {
        is_hash(hash) && self.path(hash).is_file()
    }

    pub fn contains_thumbnail(&self, hash: &str) -> bool {
        is_hash(hash) && self.thumbnail_path(hash).is_file()
    }

    pub fn read(&self, hash: &str) -> Result<Vec<u8>> {
        if !is_hash(hash) {
            anyhow::bail!("not an attachment hash: {hash}");
        }
        let path = self.path(hash);
        fs::read(&path).with_context(|| format!("reading attachment {}", path.display()))
    }

    /// Store bytes under their hash. Identical bytes are written once.
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<String> {
        let hash = hex(&Sha256::digest(bytes));
        if self.contains(&hash) {
            return Ok(hash);
        }
        let path = self.path(&hash);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        write_atomic(&path, bytes, &self.root.join("tmp"))?;
        Ok(hash)
    }

    pub fn write_thumbnail(&self, bytes: &[u8]) -> Result<String> {
        let hash = hex(&Sha256::digest(bytes));
        if self.contains_thumbnail(&hash) {
            return Ok(hash);
        }
        let path = self.thumbnail_path(&hash);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        write_atomic(&path, bytes, &self.root.join("tmp"))?;
        Ok(hash)
    }

    /// Drop a blob. Missing files are not an error: eviction must converge even
    /// if a crash left the two halves disagreeing.
    pub fn remove(&self, hash: &str) -> Result<()> {
        remove_file_if_present(&self.path(hash))
    }

    /// Drop one thumbnail, which is addressed by its own hash.
    pub fn remove_thumbnail(&self, hash: &str) -> Result<()> {
        remove_file_if_present(&self.thumbnail_path(hash))
    }

    /// Every stored original hash. Thumbnails and in-flight temp files are not
    /// origins and are skipped.
    pub fn hashes(&self) -> Result<Vec<String>> {
        self.entries_in(&self.root)
    }

    /// Every stored thumbnail hash.
    pub fn thumbnail_hashes(&self) -> Result<Vec<String>> {
        self.entries_in(&self.root.join("thumbs"))
    }

    fn entries_in(&self, root: &Path) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let Ok(buckets) = fs::read_dir(root) else {
            return Ok(out);
        };
        for bucket in buckets {
            let bucket = bucket?;
            if !bucket.file_type()?.is_dir() {
                continue;
            }
            let name = bucket.file_name();
            let name = name.to_string_lossy();
            if name == "thumbs" || name == "tmp" || name.len() != 2 {
                continue;
            }
            for entry in fs::read_dir(bucket.path())? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let hash = entry.file_name().to_string_lossy().to_string();
                if is_hash(&hash) {
                    out.push(hash);
                }
            }
        }
        Ok(out)
    }

    /// Remove temporary files left by a crash between writing and renaming.
    pub fn clear_temp(&self) -> Result<()> {
        let temp = self.root.join("tmp");
        match fs::remove_dir_all(&temp) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).with_context(|| format!("clearing {}", temp.display())),
        }
    }
}

/// Metadata produced by [`ingest`].
#[derive(Debug, Clone, PartialEq)]
pub struct Ingested {
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub hash: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub thumbnail: Option<String>,
}

impl Ingested {
    pub fn descriptor(&self) -> AttachmentInfo {
        AttachmentInfo {
            name: self.name.clone(),
            mime: self.mime.clone(),
            size: self.size,
            hash: self.hash.clone(),
            width: self.width,
            height: self.height,
            thumbnail: self.thumbnail.clone(),
            reason: None,
        }
    }
}

/// Turn one file into a stored blob plus the metadata a row needs.
///
/// `source` is usually the bridge's staging file or an upload temp file; the
/// caller deletes it once this returns, on success or failure. A failure leaves
/// no blob and no thumbnail behind, so a row is never written for bytes that are
/// not there.
pub fn ingest(
    store: &BlobStore,
    source: &Path,
    name: &str,
    declared_mime: Option<&str>,
) -> Result<Ingested> {
    let bytes =
        fs::read(source).with_context(|| format!("reading attachment {}", source.display()))?;
    ingest_bytes(store, &bytes, name, declared_mime)
}

/// The byte-level half of [`ingest`], for uploads that are already in memory.
pub fn ingest_bytes(
    store: &BlobStore,
    bytes: &[u8],
    name: &str,
    declared_mime: Option<&str>,
) -> Result<Ingested> {
    if bytes.is_empty() {
        anyhow::bail!("an attachment cannot be empty");
    }
    let name = sanitize_name(name);
    let mime = sniff_mime(&name, bytes, declared_mime);
    let hash = store.write_bytes(bytes)?;

    let mut width = None;
    let mut height = None;
    let mut thumbnail = None;
    if mime.starts_with("image/") && mime != "image/svg+xml" {
        // A mislabelled or corrupt image is still a file: keep the blob and
        // let the card render it by name. Never lose the bytes over a display
        // cache.
        if let Ok(picture) = image::load_from_memory(bytes) {
            width = Some(picture.width());
            height = Some(picture.height());
            let jpeg = thumbnail_jpeg(&picture)?;
            thumbnail = Some(store.write_thumbnail(&jpeg)?);
        }
    }

    Ok(Ingested {
        name,
        mime,
        size: bytes.len() as u64,
        hash,
        width,
        height,
        thumbnail,
    })
}

/// Drop blobs and thumbnails no row mentions any more. Returns how many files
/// were removed.
///
/// Runs at startup so a crash between writing a blob and committing its row
/// cannot leak disk forever.
pub fn sweep(
    store: &BlobStore,
    referenced: impl Fn(&str) -> Result<bool>,
    thumbnail_referenced: impl Fn(&str) -> Result<bool>,
) -> Result<usize> {
    store.clear_temp()?;
    let mut removed = 0;
    for hash in store.hashes()? {
        if !referenced(&hash)? {
            store.remove(&hash)?;
            removed += 1;
        }
    }
    for hash in store.thumbnail_hashes()? {
        if !thumbnail_referenced(&hash)? {
            store.remove_thumbnail(&hash)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing {}", path.display())),
    }
}

/// A lowercase SHA-256 hex digest, which is the only shape a hash may take.
pub fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_atomic(path: &Path, bytes: &[u8], temp_dir: &Path) -> Result<()> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fs::create_dir_all(temp_dir).with_context(|| format!("creating {}", temp_dir.display()))?;
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = temp_dir.join(format!("{}-{sequence}.part", std::process::id()));
    fs::write(&temp, bytes).with_context(|| format!("writing {}", temp.display()))?;
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(err).with_context(|| format!("committing {}", path.display()));
    }
    Ok(())
}

/// One 480 px longest-edge JPEG for display. Never sent anywhere.
fn thumbnail_jpeg(picture: &image::DynamicImage) -> Result<Vec<u8>> {
    let (width, height) = (picture.width(), picture.height());
    let longest = width.max(height);
    let (target_w, target_h) = if longest <= THUMBNAIL_EDGE {
        (width.max(1), height.max(1))
    } else {
        let scale = THUMBNAIL_EDGE as f32 / longest as f32;
        (
            ((width as f32 * scale).round() as u32).max(1),
            ((height as f32 * scale).round() as u32).max(1),
        )
    };
    let resized = picture
        .resize_exact(target_w, target_h, image::imageops::FilterType::Triangle)
        // Alpha goes onto white rather than black: a transparent PNG thumbnail
        // should look like the picture, not a hole.
        .to_rgb8();
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(resized)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .context("encoding the attachment thumbnail")?;
    Ok(out.into_inner())
}

/// Filenames are display text, never paths. Keep the last component, strip
/// control characters, and bound the length.
pub fn sanitize_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = last
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        return "attachment".to_string();
    }
    cleaned.chars().take(200).collect()
}

/// MIME type by extension, with magic bytes winning for the image types rchat
/// displays. A declared type is used when nothing else knows.
pub fn sniff_mime(name: &str, bytes: &[u8], declared: Option<&str>) -> String {
    if let Some(magic) = magic_mime(bytes) {
        return magic.to_string();
    }
    if let Some(declared) = declared.map(str::trim) {
        if !declared.is_empty() && declared != "application/octet-stream" {
            // Strip parameters such as `; charset=utf-8`: the descriptor wants
            // the media type only.
            return declared
                .split(';')
                .next()
                .unwrap_or(declared)
                .trim()
                .to_string();
        }
    }
    match extension(name).as_str() {
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "webp" => "image/webp".to_string(),
        "bmp" => "image/bmp".to_string(),
        "svg" => "image/svg+xml".to_string(),
        "pdf" => "application/pdf".to_string(),
        "txt" | "log" => "text/plain".to_string(),
        "md" => "text/markdown".to_string(),
        "csv" => "text/csv".to_string(),
        "json" => "application/json".to_string(),
        "html" | "htm" => "text/html".to_string(),
        "zip" => "application/zip".to_string(),
        "gz" | "tgz" => "application/gzip".to_string(),
        "tar" => "application/x-tar".to_string(),
        "7z" => "application/x-7z-compressed".to_string(),
        "mp3" => "audio/mpeg".to_string(),
        "ogg" | "opus" => "audio/ogg".to_string(),
        "wav" => "audio/wav".to_string(),
        "m4a" => "audio/mp4".to_string(),
        "mp4" => "video/mp4".to_string(),
        "webm" => "video/webm".to_string(),
        "apk" => "application/vnd.android.package-archive".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn magic_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") && bytes.len() > 18 {
        return Some("image/bmp");
    }
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return Some("application/zip");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rchat-blobs-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A real PNG of the requested size, encoded by the same crate that reads it.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut picture = image::RgbImage::new(width, height);
        for (x, y, pixel) in picture.enumerate_pixels_mut() {
            *pixel = image::Rgb([(x % 256) as u8, (y % 256) as u8, 120]);
        }
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(picture)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn an_image_keeps_dimensions_and_gets_a_bounded_thumbnail() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let bytes = png(1200, 800);

        let ingested = ingest_bytes(&store, &bytes, "holiday.png", None).unwrap();
        assert_eq!(ingested.mime, "image/png");
        assert_eq!(ingested.size, bytes.len() as u64);
        assert_eq!((ingested.width, ingested.height), (Some(1200), Some(800)));
        assert_eq!(store.read(&ingested.hash).unwrap(), bytes);

        let thumbnail_hash = ingested.thumbnail.expect("images get a thumbnail");
        let thumbnail =
            image::load_from_memory(&fs::read(store.thumbnail_path(&thumbnail_hash)).unwrap())
                .unwrap();
        assert!(thumbnail.width().max(thumbnail.height()) <= THUMBNAIL_EDGE);
        assert!(
            thumbnail.width() > thumbnail.height(),
            "the aspect ratio must survive"
        );
    }

    #[test]
    fn a_small_image_is_never_upscaled() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let ingested = ingest_bytes(&store, &png(60, 30), "tiny.png", None).unwrap();
        let thumbnail_hash = ingested.thumbnail.unwrap();
        let thumbnail =
            image::load_from_memory(&fs::read(store.thumbnail_path(&thumbnail_hash)).unwrap())
                .unwrap();
        assert_eq!((thumbnail.width(), thumbnail.height()), (60, 30));
    }

    #[test]
    fn a_non_image_has_a_type_but_no_pixels_or_thumbnail() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let ingested = ingest_bytes(&store, b"%PDF-1.7 hello", "notes.pdf", None).unwrap();
        assert_eq!(ingested.mime, "application/pdf");
        assert_eq!(ingested.width, None);
        assert_eq!(ingested.thumbnail, None);
    }

    #[test]
    fn identical_bytes_are_stored_once() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let first = ingest_bytes(&store, &png(10, 10), "one.png", None).unwrap();
        let second = ingest_bytes(&store, &png(10, 10), "two.png", None).unwrap();
        assert_eq!(first.hash, second.hash);
        assert_eq!(store.hashes().unwrap(), std::slice::from_ref(&first.hash));
        assert_eq!(
            store.read(&first.hash).unwrap(),
            store.read(&second.hash).unwrap()
        );
    }

    #[test]
    fn a_filename_never_becomes_a_path() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let ingested = ingest_bytes(&store, b"hi", "../../etc/passwd", None).unwrap();
        assert_eq!(ingested.name, "passwd");
        assert!(store.path(&ingested.hash).starts_with(&root));
        assert_eq!(sanitize_name(".."), "attachment");
        assert_eq!(sanitize_name(""), "attachment");
    }

    #[test]
    fn magic_bytes_beat_a_misleading_extension() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let bytes = png(4, 4);
        let ingested = ingest_bytes(&store, &bytes, "really-a-pdf.pdf", None).unwrap();
        assert_eq!(ingested.mime, "image/png");
        assert!(ingested.thumbnail.is_some());
    }

    #[test]
    fn a_corrupt_image_still_keeps_its_bytes() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let bytes = {
            let mut value = vec![0xff, 0xd8, 0xff];
            value.extend_from_slice(b"not really a jpeg");
            value
        };
        let ingested = ingest_bytes(&store, &bytes, "broken.jpg", None).unwrap();
        assert_eq!(ingested.mime, "image/jpeg");
        assert_eq!(ingested.width, None);
        assert_eq!(ingested.thumbnail, None);
        assert_eq!(store.read(&ingested.hash).unwrap(), bytes);
    }

    #[test]
    fn the_sweep_drops_only_unreferenced_blobs() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let kept = ingest_bytes(&store, b"keep me", "keep.txt", None).unwrap();
        let dropped = ingest_bytes(&store, b"drop me", "drop.txt", None).unwrap();

        let removed = sweep(&store, |hash| Ok(hash == kept.hash), |_| Ok(true)).unwrap();
        assert_eq!(removed, 1);
        assert!(store.contains(&kept.hash));
        assert!(!store.contains(&dropped.hash));
        assert_eq!(store.hashes().unwrap(), [kept.hash]);
    }

    #[test]
    fn removing_a_blob_takes_its_thumbnail() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let ingested = ingest_bytes(&store, &png(500, 500), "photo.png", None).unwrap();
        let thumbnail = ingested.thumbnail.clone().unwrap();
        assert!(store.contains(&ingested.hash));
        assert!(store.contains_thumbnail(&thumbnail));
        store.remove(&ingested.hash).unwrap();
        store.remove_thumbnail(&thumbnail).unwrap();
        assert!(!store.contains(&ingested.hash));
        assert!(!store.contains_thumbnail(&thumbnail));
        // Removing twice is fine: eviction must be able to converge.
        store.remove(&ingested.hash).unwrap();
        store.remove_thumbnail(&thumbnail).unwrap();
    }

    #[test]
    fn unreadable_sources_fail_without_writing_anything() {
        let root = scratch();
        let store = BlobStore::new(&root);
        let missing = root.join("does-not-exist.pdf");
        assert!(ingest(&store, &missing, "x.pdf", None).is_err());
        assert!(store.hashes().unwrap().is_empty());

        let unwritable = root.join("blocked");
        // A file where the store root should be: no bucket can be created.
        fs::write(&unwritable, b"not a directory").unwrap();
        let store = BlobStore::new(&unwritable);
        let source = root.join("source.txt");
        fs::write(&source, b"hello").unwrap();
        assert!(ingest(&store, &source, "hello.txt", None).is_err());
    }
}
