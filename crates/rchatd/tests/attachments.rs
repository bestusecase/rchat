//! Attachment behaviour through the real engine and a scripted bridge.
//!
//! These tests drive the engine exactly as a shell does: a WebSocket for
//! commands, plain HTTP for bytes, and a TCP socket playing the Python bridge so
//! a send can be inspected and an inbox can be scripted. Nothing here asserts on
//! private helpers or on the order in which files were written.
//!
//! The attachment budget and inbound ceiling are process environment variables,
//! so every test takes [`env_lock`] for its whole life. That keeps the suite
//! deterministic at the cost of running one engine test at a time.

use futures_util::{SinkExt, StreamExt};
use rchat_core::attachments::{REASON_EVICTED, REASON_EXTRA, REASON_MISSING, REASON_TOO_LARGE};
use rchat_ipc::{Caps, PeerInfo};
use rchatd::{BridgeMode, Engine, Options};
use serde_json::{json, Value};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const BUDGET: u64 = 500 * 1024 * 1024;
const INBOUND: u64 = 8 * 1024 * 1024;

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

fn scratch_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("rchatd-att-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One running engine with a connected shell and a scripted bridge.
struct Harness {
    engine: Option<Engine>,
    dir: PathBuf,
    port: u16,
    token: String,
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    bridge: BufReader<OwnedReadHalf>,
    bridge_tx: OwnedWriteHalf,
    next_id: u64,
}

impl Harness {
    /// Start an engine in the mobile bridge mode and dial in as both the shell
    /// and the bridge. The default limits apply unless the test overrides them.
    async fn start(name: &str) -> Self {
        Self::start_with(name, BUDGET, INBOUND).await
    }

    async fn start_with(name: &str, budget: u64, inbound: u64) -> Self {
        std::env::set_var("RCHAT_ATTACHMENT_BUDGET_BYTES", budget.to_string());
        std::env::set_var("RCHAT_ATTACHMENT_MAX_BYTES", inbound.to_string());

        let dir = scratch_dir(name);
        let engine = Engine::start(Options {
            data_dir: dir.clone(),
            config_dir: None,
            port: 0,
            no_rns: false,
            bridge: BridgeMode::Attach,
            legacy_network: None,
        })
        .await
        .expect("engine should start");
        let endpoint = engine.endpoint().clone();
        let bridge_port = endpoint.bridge_port.expect("attach mode bridge port");

        let (ws, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{}/ws?token={}",
            endpoint.port, endpoint.token
        ))
        .await
        .expect("websocket should connect");

        let bridge = TcpStream::connect(("127.0.0.1", bridge_port))
            .await
            .expect("bridge port should accept");
        let (bridge_read, bridge_tx) = bridge.into_split();
        let mut harness = Self {
            engine: Some(engine),
            dir,
            port: endpoint.port,
            token: endpoint.token,
            ws,
            bridge: BufReader::new(bridge_read),
            bridge_tx,
            next_id: 0,
        };
        harness
            .bridge_event(
                json!({"ev":"state","state":"ready","address":"ab".repeat(16),"name":"test"}),
            )
            .await;
        // A live bridge samples the mesh immediately; `ready` needs an online
        // interface, not just a started bridge.
        harness
            .bridge_event(json!({"ev":"mesh","mesh":{
                "transport":"ready",
                "address":"ab".repeat(16),
                "interfaces":[{"name":"LAN","kind":"AutoInterface","online":true,"bitrate":10000000}],
                "counters":{},
                "paths":[],
                "other_services":[],
            }}))
            .await;
        harness.wait_ready().await;
        harness
    }

    async fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(body) = http_text(self.port, "/health").await {
                if body.contains("\"transport\":\"ready\"") {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the scripted bridge never became ready");
    }

    fn store(&self) -> std::sync::Arc<rchat_core::store::Store> {
        self.engine.as_ref().unwrap().store()
    }

    /// The conversation a peer's messages land in, after the engine created it.
    fn conversation_with_peer(&self, address: &str) -> String {
        let peer = self
            .store()
            .peer_by_address(address)
            .unwrap()
            .expect("the peer should exist after its message");
        self.store()
            .list_conversations()
            .unwrap()
            .into_iter()
            .find(|conversation| conversation.peer_ids.contains(&peer.id))
            .expect("the peer should have a conversation")
            .id
    }

    async fn command(&mut self, cmd: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut payload = cmd;
        payload["id"] = json!(id);
        self.ws
            .send(WsMessage::Text(payload.to_string().into()))
            .await
            .expect("websocket send");
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(10), self.ws.next())
                .await
                .expect("the engine did not answer a command")
                .expect("websocket closed")
                .expect("websocket error");
            let WsMessage::Text(text) = frame else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value.get("type").and_then(Value::as_str) != Some("reply") {
                continue;
            }
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            return if value.get("ok").and_then(Value::as_bool) == Some(true) {
                Ok(value.get("data").cloned().unwrap_or(Value::Null))
            } else {
                Err(value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("command failed")
                    .to_string())
            };
        }
    }

    /// Register a peer with a chosen bandwidth class, then open its conversation.
    async fn conversation_with(&mut self, address: &str, class: &str) -> String {
        let peer = PeerInfo {
            id: address.to_string(),
            address: Some(address.to_string()),
            nickname: Some("Alice".into()),
            announced_name: None,
            last_seen: Some(1),
            bandwidth_class: class.into(),
            caps: Caps::default(),
            notes: None,
        };
        self.store().upsert_peer(&peer).unwrap();
        let reply = self
            .command(json!({"cmd": "open_direct", "peer_id": address}))
            .await
            .expect("opening a direct conversation");
        reply["conversation"]["id"].as_str().unwrap().to_string()
    }

    async fn send(
        &mut self,
        conversation: &str,
        body: &str,
        hashes: &[String],
    ) -> Result<Value, String> {
        self.command(json!({
            "cmd": "send_message",
            "conversation_id": conversation,
            "body": body,
            "attachments": hashes,
        }))
        .await
    }

    async fn list(&mut self, conversation: &str) -> Vec<Value> {
        let reply = self
            .command(json!({"cmd": "list_messages", "conversation_id": conversation}))
            .await
            .expect("listing messages");
        reply["messages"].as_array().cloned().unwrap_or_default()
    }

    async fn upload(&self, name: &str, mime: &str, bytes: &[u8]) -> HttpResponse {
        upload(self.port, &self.token, name, mime, bytes).await
    }

    async fn upload_ok(&self, name: &str, mime: &str, bytes: &[u8]) -> Value {
        let response = self.upload(name, mime, bytes).await;
        assert_eq!(
            response.status,
            200,
            "upload failed: {}",
            response.body_text()
        );
        response.json()["attachment"].clone()
    }

    async fn download(&self, hash: &str, variant: Option<&str>) -> HttpResponse {
        let mut path = format!("/attachments/{hash}?token={}", self.token);
        if let Some(variant) = variant {
            path.push_str("&variant=");
            path.push_str(variant);
        }
        http_get(self.port, &path).await
    }

    async fn bridge_event(&mut self, event: Value) {
        self.bridge_tx
            .write_all(format!("{event}\n").as_bytes())
            .await
            .expect("writing a bridge event");
    }

    async fn bridge_command(&mut self) -> Value {
        loop {
            let mut line = String::new();
            let read =
                tokio::time::timeout(Duration::from_secs(5), self.bridge.read_line(&mut line))
                    .await
                    .expect("the engine sent no bridge command")
                    .expect("reading the bridge command");
            assert!(read > 0, "the bridge socket closed");
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line).unwrap();
            // Startup housekeeping (`import_config`) may precede the message.
            if value.get("cmd").and_then(Value::as_str) == Some("send") {
                return value;
            }
        }
    }

    /// The next published event with this name, skipping the rest. Only for use
    /// when no command reply is outstanding: replies are skipped here.
    async fn next_event(&mut self, name: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the {name} event never arrived");
            let frame = tokio::time::timeout(left, self.ws.next())
                .await
                .expect("the engine published nothing")
                .expect("websocket closed")
                .expect("websocket error");
            let WsMessage::Text(text) = frame else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value.get("type").and_then(Value::as_str) == Some("event")
                && value.get("name").and_then(Value::as_str) == Some(name)
            {
                return value;
            }
        }
    }

    async fn close(&mut self) {
        // A live websocket would keep axum's graceful shutdown waiting.
        let _ = self.ws.close(None).await;
        if let Some(engine) = self.engine.take() {
            engine.stop();
            engine.wait().await.expect("clean shutdown");
        }
    }

    /// Stop the engine but keep the data directory, for restart checks.
    async fn shutdown(mut self) {
        self.close().await;
    }

    async fn stop(mut self) {
        self.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---- tiny HTTP client -----------------------------------------------------

struct HttpResponse {
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn http_text(port: u16, path: &str) -> Option<String> {
    let response = http_get(port, path).await;
    if response.status == 200 {
        Some(response.body_text())
    } else {
        None
    }
}

async fn http_get(port: u16, path: &str) -> HttpResponse {
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    exchange(port, request.into_bytes()).await
}

async fn upload(port: u16, token: &str, name: &str, mime: &str, bytes: &[u8]) -> HttpResponse {
    let head = format!(
        "POST /attachments?token={} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
         Content-Type: {mime}\r\nX-Rchat-Filename: {}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        token,
        percent_encode(name),
        bytes.len()
    );
    let mut request = head.into_bytes();
    request.extend_from_slice(bytes);
    exchange(port, request).await
}

async fn exchange(port: u16, request: Vec<u8>) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the API should be listening");
    stream.write_all(&request).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();

    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("an HTTP response always has a header terminator");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = raw[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("a status line");
    let content_type = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim().to_string());
    HttpResponse {
        status,
        content_type,
        body,
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ---- fixtures -------------------------------------------------------------

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut picture = image::RgbImage::new(width, height);
    for (x, y, pixel) in picture.enumerate_pixels_mut() {
        *pixel = image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]);
    }
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(picture)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn staged(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let staging = dir.join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    let path = staging.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn blobs(dir: &Path) -> PathBuf {
    dir.join("attachments")
}

/// Every regular file under a directory, for "no blob was written" assertions.
fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else if path.is_file() {
            out.push(path);
        }
    }
    out
}

fn attachment(message: &Value) -> &Value {
    &message["attachments"][0]
}

// ---- storage and ingest ---------------------------------------------------

#[tokio::test]
async fn an_uploaded_image_arrives_whole_with_a_display_thumbnail() {
    let _env = env_lock().lock().await;
    let harness = Harness::start("image").await;
    let bytes = png(900, 600);

    let descriptor = harness.upload_ok("holiday.png", "image/png", &bytes).await;
    assert_eq!(descriptor["name"], "holiday.png");
    assert_eq!(descriptor["mime"], "image/png");
    assert_eq!(descriptor["size"].as_u64(), Some(bytes.len() as u64));
    assert_eq!(descriptor["width"].as_u64(), Some(900));
    assert_eq!(descriptor["height"].as_u64(), Some(600));
    let hash = descriptor["hash"].as_str().unwrap().to_string();
    let thumbnail = descriptor["thumbnail"]
        .as_str()
        .expect("images get a thumbnail")
        .to_string();

    let original = harness.download(&hash, None).await;
    assert_eq!(original.status, 200);
    assert_eq!(original.content_type.as_deref(), Some("image/png"));
    assert_eq!(
        original.body, bytes,
        "the original must be sent, not a re-encode"
    );

    let thumbnail = harness.download(&thumbnail, None).await;
    assert_eq!(thumbnail.status, 200);
    assert_eq!(thumbnail.content_type.as_deref(), Some("image/jpeg"));
    let picture = image::load_from_memory(&thumbnail.body).expect("the thumbnail must decode");
    assert!(picture.width().max(picture.height()) <= 480);
    assert!(
        picture.width() > picture.height(),
        "the aspect ratio must survive"
    );

    harness.stop().await;
}

#[tokio::test]
async fn a_non_image_carries_its_type_but_no_pixels() {
    let _env = env_lock().lock().await;
    let harness = Harness::start("pdf").await;

    let descriptor = harness
        .upload_ok("board minutes.pdf", "application/pdf", b"%PDF-1.7\nhello")
        .await;
    assert_eq!(descriptor["name"], "board minutes.pdf");
    assert_eq!(descriptor["mime"], "application/pdf");
    assert!(descriptor.get("width").is_none());
    assert!(
        descriptor.get("thumbnail").is_none(),
        "a PDF has nothing to preview"
    );

    harness.stop().await;
}

#[tokio::test]
async fn the_same_bytes_cost_one_blob_however_many_messages_use_them() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("dedup").await;
    let conversation = harness.conversation_with(&"cd".repeat(16), "lan").await;
    let bytes = png(30, 20);
    let descriptor = harness.upload_ok("photo.png", "image/png", &bytes).await;
    let hash = descriptor["hash"].as_str().unwrap().to_string();

    harness
        .send(&conversation, "first", &[hash.clone()])
        .await
        .unwrap();
    harness
        .send(&conversation, "second", &[hash.clone()])
        .await
        .unwrap();
    let _ = harness.bridge_command().await;
    let _ = harness.bridge_command().await;

    let messages = harness.list(&conversation).await;
    assert_eq!(messages.len(), 2);
    for message in &messages {
        assert_eq!(attachment(message)["hash"].as_str(), Some(hash.as_str()));
    }
    let stored = files_under(&blobs(&harness.dir));
    assert_eq!(
        stored
            .iter()
            .filter(|path| path.file_stem().and_then(|s| s.to_str()) == Some(hash.as_str()))
            .count(),
        1,
        "identical bytes are stored once: {stored:?}"
    );

    harness.stop().await;
}

#[tokio::test]
async fn a_captioned_photo_is_an_image_message() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("caption").await;
    let conversation = harness.conversation_with(&"ce".repeat(16), "lan").await;
    let descriptor = harness
        .upload_ok("sunset.png", "image/png", &png(40, 30))
        .await;
    let hash = descriptor["hash"].as_str().unwrap().to_string();

    harness
        .send(&conversation, "  the view from here  ", &[hash])
        .await
        .unwrap();
    let _ = harness.bridge_command().await;

    let messages = harness.list(&conversation).await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["kind"], "image");
    assert_eq!(messages[0]["body"], "the view from here");
    assert_eq!(attachment(&messages[0])["name"], "sunset.png");

    harness.stop().await;
}

#[tokio::test]
async fn a_failed_ingest_leaves_neither_row_nor_blob() {
    let _env = env_lock().lock().await;
    let harness = Harness::start("unwritable").await;
    let root = blobs(&harness.dir);
    std::fs::create_dir_all(&root).unwrap();
    let mut permissions = std::fs::metadata(&root).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o500);
        std::fs::set_permissions(&root, permissions).unwrap();
    }

    let response = harness.upload("blocked.png", "image/png", &png(8, 8)).await;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut restore = std::fs::metadata(&root).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&root, restore).unwrap();
    }
    assert_ne!(
        response.status, 200,
        "an unwritable store cannot accept bytes"
    );
    assert_eq!(harness.store().attachment_usage().unwrap(), 0);
    assert!(
        files_under(&root).is_empty(),
        "no blob should have been written"
    );

    harness.stop().await;
}

#[tokio::test]
async fn the_startup_sweep_keeps_referenced_blobs_and_drops_the_rest() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("sweep").await;
    let conversation = harness.conversation_with(&"e4".repeat(16), "lan").await;
    let descriptor = harness
        .upload_ok("keep.txt", "text/plain", b"keep me")
        .await;
    let kept = descriptor["hash"].as_str().unwrap().to_string();
    harness
        .send(&conversation, "keep", &[kept.clone()])
        .await
        .unwrap();
    let _ = harness.bridge_command().await;

    let dir = harness.dir.clone();
    harness.shutdown().await;

    // A crash between writing bytes and committing a row leaves a blob behind.
    let orphan = "ff".repeat(32);
    let orphan_path = blobs(&dir).join(&orphan[..2]).join(&orphan);
    std::fs::create_dir_all(orphan_path.parent().unwrap()).unwrap();
    std::fs::write(&orphan_path, b"nobody owns me").unwrap();

    let engine = Engine::start(Options {
        data_dir: dir.clone(),
        config_dir: None,
        port: 0,
        no_rns: true,
        bridge: BridgeMode::None,
        legacy_network: None,
    })
    .await
    .expect("restart should succeed");
    assert!(!orphan_path.exists(), "the orphan should be swept");
    assert!(
        files_under(&blobs(&dir))
            .iter()
            .any(|path| path.file_stem().and_then(|s| s.to_str()) == Some(kept.as_str())),
        "a referenced blob must survive the sweep"
    );
    engine.stop();
    engine.wait().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_upload_that_was_never_sent_does_not_survive_a_restart() {
    let _env = env_lock().lock().await;
    let harness = Harness::start("stale-upload").await;
    let descriptor = harness
        .upload_ok("draft.txt", "text/plain", b"never sent")
        .await;
    let hash = descriptor["hash"].as_str().unwrap().to_string();
    let dir = harness.dir.clone();
    harness.shutdown().await;

    let engine = Engine::start(Options {
        data_dir: dir.clone(),
        config_dir: None,
        port: 0,
        no_rns: true,
        bridge: BridgeMode::None,
        legacy_network: None,
    })
    .await
    .expect("restart should succeed");
    assert!(
        engine.store().attachment_by_hash(&hash).unwrap().is_none(),
        "an unclaimed upload is stale once the engine restarts"
    );
    assert!(
        !files_under(&blobs(&dir))
            .iter()
            .any(|path| path.file_stem().and_then(|s| s.to_str()) == Some(hash.as_str())),
        "its blob goes too"
    );
    engine.stop();
    engine.wait().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_message_whose_blob_vanished_reads_back_with_a_reason() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("vanished").await;
    let conversation = harness.conversation_with(&"cf".repeat(16), "lan").await;
    let descriptor = harness
        .upload_ok("photo.png", "image/png", &png(20, 20))
        .await;
    let hash = descriptor["hash"].as_str().unwrap().to_string();
    harness
        .send(&conversation, "look", &[hash.clone()])
        .await
        .unwrap();
    let _ = harness.bridge_command().await;

    std::fs::remove_file(blobs(&harness.dir).join(&hash[..2]).join(&hash)).unwrap();

    let messages = harness.list(&conversation).await;
    let row = attachment(&messages[0]);
    assert_eq!(row["name"], "photo.png");
    assert_eq!(row["size"].as_u64(), descriptor["size"].as_u64());
    assert_eq!(row["reason"].as_str(), Some(REASON_MISSING));
    assert!(
        row.get("thumbnail").is_none(),
        "a missing blob has no thumbnail"
    );
    assert_eq!(messages[0]["body"], "look");

    harness.stop().await;
}

// ---- size policy ----------------------------------------------------------

#[tokio::test]
async fn an_over_limit_send_is_refused_and_writes_nothing() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("radio-limit").await;
    let conversation = harness.conversation_with(&"da".repeat(16), "radio").await;
    let bytes = vec![7u8; 65 * 1024];
    let descriptor = harness
        .upload_ok("big.bin", "application/octet-stream", &bytes)
        .await;
    let hash = descriptor["hash"].as_str().unwrap().to_string();

    let error = harness
        .send(&conversation, "too big", &[hash])
        .await
        .unwrap_err();
    assert!(error.contains("radio"), "the class must be named: {error}");
    assert!(
        error.contains("64 KB"),
        "the limit must be legible: {error}"
    );
    assert!(
        error.contains("65 KB"),
        "the actual size must be named: {error}"
    );
    assert!(
        harness.list(&conversation).await.is_empty(),
        "no message row may exist"
    );

    harness.stop().await;
}

#[tokio::test]
async fn the_limit_is_the_sum_not_the_largest_file() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("sum-limit").await;
    let conversation = harness.conversation_with(&"db".repeat(16), "radio").await;

    // Exactly 64 KB in one file is allowed.
    let exact = harness
        .upload_ok(
            "exact.bin",
            "application/octet-stream",
            &vec![1u8; 64 * 1024],
        )
        .await;
    harness
        .send(
            &conversation,
            "",
            &[exact["hash"].as_str().unwrap().to_string()],
        )
        .await
        .expect("exactly the limit is allowed");
    let _ = harness.bridge_command().await;

    // Two 40 KB files are 80 KB together, over the same limit.
    let first = harness
        .upload_ok("one.bin", "application/octet-stream", &vec![2u8; 40 * 1024])
        .await;
    let second = harness
        .upload_ok("two.bin", "application/octet-stream", &vec![3u8; 40 * 1024])
        .await;
    let error = harness
        .send(
            &conversation,
            "pair",
            &[
                first["hash"].as_str().unwrap().to_string(),
                second["hash"].as_str().unwrap().to_string(),
            ],
        )
        .await
        .unwrap_err();
    assert!(error.contains("80 KB"), "the sum is what counts: {error}");
    assert_eq!(
        harness.list(&conversation).await.len(),
        1,
        "the refusal writes nothing"
    );

    harness.stop().await;
}

#[tokio::test]
async fn five_attachments_are_refused_regardless_of_size() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("count-limit").await;
    let conversation = harness.conversation_with(&"dc".repeat(16), "lan").await;
    let mut hashes = Vec::new();
    for index in 0..5 {
        let descriptor = harness
            .upload_ok(&format!("tiny-{index}.txt"), "text/plain", b"x")
            .await;
        hashes.push(descriptor["hash"].as_str().unwrap().to_string());
    }

    let error = harness
        .send(&conversation, "five", &hashes)
        .await
        .unwrap_err();
    assert!(error.contains("4"), "the cap must be stated: {error}");
    assert!(harness.list(&conversation).await.is_empty());

    harness.stop().await;
}

#[tokio::test]
async fn an_unknown_peer_gets_the_cautious_ceiling() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("unknown-limit").await;
    let conversation = harness.conversation_with(&"dd".repeat(16), "unknown").await;
    let descriptor = harness
        .upload_ok(
            "over.bin",
            "application/octet-stream",
            &vec![9u8; 1024 * 1024 + 1],
        )
        .await;

    let error = harness
        .send(
            &conversation,
            "",
            &[descriptor["hash"].as_str().unwrap().to_string()],
        )
        .await
        .unwrap_err();
    assert!(
        error.contains("unknown"),
        "an unmeasured path is not fast: {error}"
    );
    assert!(error.contains("1.0 MB"));

    harness.stop().await;
}

#[tokio::test]
async fn an_oversized_inbound_attachment_becomes_a_placeholder() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start_with("inbound-limit", BUDGET, 128).await;
    let bytes = vec![4u8; 4096];
    let path = staged(&harness.dir, "huge.bin", &bytes);

    harness
        .bridge_event(json!({
            "ev": "message",
            "id": "big",
            "from": "de".repeat(16),
            "body": "that file is enormous",
            "ts": 10,
            "attachments": [
                {"path": path.to_string_lossy(), "name": "huge.bin", "ordinal": 0, "size": bytes.len()}
            ]
        }))
        .await;

    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.store().get_message("rns-big").unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "the inbound message never arrived"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let conversations = harness.store().list_conversations().unwrap();
    assert_eq!(conversations.len(), 1);
    let messages = harness.list(&conversations[0].id).await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["body"], "that file is enormous");
    let row = attachment(&messages[0]);
    assert_eq!(row["name"], "huge.bin");
    assert_eq!(row["reason"].as_str(), Some(REASON_TOO_LARGE));
    assert_eq!(row["hash"].as_str(), Some(""));
    assert!(harness.store().attachment_usage().unwrap() == 0);
    assert!(files_under(&blobs(&harness.dir)).is_empty());

    harness.stop().await;
}

#[tokio::test]
async fn an_inbound_attachment_evicted_on_arrival_still_reads_with_a_reason() {
    let _env = env_lock().lock().await;
    // The budget is smaller than the one file, so the same ingest that writes
    // the blob has to take it away again.
    let mut harness = Harness::start_with("inbound-evict", 64, INBOUND).await;
    let bytes = vec![7u8; 1024];
    let path = staged(&harness.dir, "clip.bin", &bytes);

    harness
        .bridge_event(json!({
            "ev": "message",
            "id": "evicted",
            "from": "ee".repeat(16),
            "body": "watch this",
            "ts": 12,
            "attachments": [
                {"path": path.to_string_lossy(), "name": "clip.bin", "ordinal": 0, "size": bytes.len()}
            ]
        }))
        .await;

    // The live event, not a later reload: a bubble must not show an image whose
    // bytes were already gone when the message arrived.
    let event = harness.next_event("message.new").await;
    let message = &event["data"]["message"];
    assert_eq!(message["body"], "watch this");
    let row = attachment(message);
    assert_eq!(row["name"], "clip.bin");
    assert!(row["hash"].as_str().is_some_and(|hash| !hash.is_empty()));
    assert_eq!(row["reason"].as_str(), Some(REASON_EVICTED));
    assert!(row.get("thumbnail").is_none());
    assert!(files_under(&blobs(&harness.dir)).is_empty());

    harness.stop().await;
}

#[tokio::test]
async fn attachments_past_the_fourth_arrive_as_a_placeholder() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("inbound-extras").await;
    let mut entries = Vec::new();
    for index in 0..5 {
        let name = format!("part-{index}.bin");
        let bytes = vec![index as u8; 32];
        let path = staged(&harness.dir, &name, &bytes);
        entries.push(json!({
            "path": path.to_string_lossy(),
            "name": name,
            "ordinal": index,
            "size": bytes.len(),
        }));
    }

    harness
        .bridge_event(json!({
            "ev": "message",
            "id": "five",
            "from": "ef".repeat(16),
            "body": "a burst",
            "ts": 13,
            "attachments": entries,
        }))
        .await;

    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.store().get_message("rns-five").unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "the inbound message never arrived"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let conversation = harness.conversation_with_peer(&"ef".repeat(16));
    let messages = harness.list(&conversation).await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["body"], "a burst");
    let rows = messages[0]["attachments"].as_array().unwrap();
    assert_eq!(rows.len(), 5, "the extras keep their place in the message");
    for row in &rows[..4] {
        assert!(row["hash"].as_str().is_some_and(|hash| !hash.is_empty()));
        assert!(row.get("reason").is_none());
    }
    let extra = &rows[4];
    assert_eq!(extra["name"], "part-4.bin");
    assert_eq!(extra["hash"].as_str(), Some(""));
    assert_eq!(extra["reason"].as_str(), Some(REASON_EXTRA));
    assert_eq!(files_under(&blobs(&harness.dir)).len(), 4);

    harness.stop().await;
}

#[tokio::test]
async fn eviction_takes_received_bytes_before_sent_ones() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("eviction").await;
    let conversation = harness.conversation_with(&"df".repeat(16), "lan").await;

    let sent = harness
        .upload_ok("mine.txt", "text/plain", &[1u8; 40])
        .await;
    harness
        .send(
            &conversation,
            "mine",
            &[sent["hash"].as_str().unwrap().to_string()],
        )
        .await
        .unwrap();
    let _ = harness.bridge_command().await;

    let oldest = png(50, 40);
    let oldest_path = staged(&harness.dir, "oldest.png", &oldest);
    harness
        .bridge_event(json!({
            "ev": "message", "id": "oldest", "from": "e0".repeat(16),
            "body": "first photo", "ts": 1000,
            "attachments": [{"path": oldest_path.to_string_lossy(), "name": "oldest.png",
                             "ordinal": 0, "size": oldest.len()}]
        }))
        .await;
    let newest_path = staged(&harness.dir, "newest.txt", b"newest message body\n");
    harness
        .bridge_event(json!({
            "ev": "message", "id": "newest", "from": "e0".repeat(16),
            "body": "second file", "ts": 2000,
            "attachments": [{"path": newest_path.to_string_lossy(), "name": "newest.txt",
                             "ordinal": 0, "size": 20}]
        }))
        .await;

    // Wait until both received messages are stored.
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.store().get_message("rns-oldest").unwrap().is_none()
        || harness.store().get_message("rns-newest").unwrap().is_none()
    {
        assert!(Instant::now() < deadline, "inbound events never arrived");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let inbound = harness.conversation_with_peer(&"e0".repeat(16));

    let image_row = harness
        .store()
        .get_message("rns-oldest")
        .unwrap()
        .and_then(|message| message.attachments.into_iter().next())
        .expect("the image message should carry its row");
    let image_hash = image_row.hash.clone();
    let thumbnail = image_row
        .thumbnail
        .clone()
        .expect("an image gets a thumbnail");
    let usage = harness.store().attachment_usage().unwrap();

    // Lower the budget to exactly what evicting only the oldest image costs,
    // then trigger enforcement with a small upload.
    let fresh = 4u64;
    std::env::set_var(
        "RCHAT_ATTACHMENT_BUDGET_BYTES",
        (usage + fresh - image_row.size).to_string(),
    );
    harness
        .upload_ok("trigger.txt", "text/plain", &vec![5u8; fresh as usize])
        .await;

    let after = harness.list(&inbound).await;
    let image = after
        .iter()
        .find(|message| message["id"] == "rns-oldest")
        .unwrap()
        .clone();
    assert_eq!(image["body"], "first photo");
    assert_eq!(image["ts"], 1000);
    assert_eq!(
        image["state"], "delivered",
        "eviction never touches message state"
    );
    assert_eq!(attachment(&image)["reason"].as_str(), Some(REASON_EVICTED));
    assert_ne!(harness.download(&image_hash, None).await.status, 200);
    assert_ne!(harness.download(&thumbnail, None).await.status, 200);

    let newest = after
        .iter()
        .find(|message| message["id"] == "rns-newest")
        .unwrap()
        .clone();
    assert!(
        attachment(&newest).get("reason").is_none(),
        "newer bytes survive"
    );
    let sent_conversation = harness.list(&conversation).await;
    let mine = sent_conversation
        .iter()
        .find(|message| message["body"] == "mine")
        .unwrap()
        .clone();
    assert!(
        attachment(&mine).get("reason").is_none(),
        "sent bytes are the last resort"
    );

    // Now squeeze further: the remaining received blob goes before the sent one.
    std::env::set_var("RCHAT_ATTACHMENT_BUDGET_BYTES", "20");
    harness
        .upload_ok("trigger2.txt", "text/plain", b"zzzz")
        .await;
    let squeezed_in = harness.list(&inbound).await;
    let newest = squeezed_in
        .iter()
        .find(|message| message["id"] == "rns-newest")
        .unwrap()
        .clone();
    assert_eq!(attachment(&newest)["reason"].as_str(), Some(REASON_EVICTED));
    let squeezed_out = harness.list(&conversation).await;
    let mine = squeezed_out
        .iter()
        .find(|message| message["body"] == "mine")
        .unwrap()
        .clone();
    assert_eq!(attachment(&mine)["reason"].as_str(), Some(REASON_EVICTED));

    harness.stop().await;
}

// ---- wire, send command and API ------------------------------------------

#[tokio::test]
async fn a_sent_attachment_reaches_the_bridge_as_a_path() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("dispatch").await;
    let conversation = harness.conversation_with(&"e1".repeat(16), "lan").await;
    let bytes = png(24, 18);
    let descriptor = harness.upload_ok("note.png", "image/png", &bytes).await;
    let hash = descriptor["hash"].as_str().unwrap().to_string();

    harness
        .send(&conversation, "see attached", &[hash])
        .await
        .unwrap();
    let command = harness.bridge_command().await;

    assert_eq!(command["cmd"], "send");
    assert_eq!(command["body"], "see attached");
    let sent = &command["attachments"][0];
    assert_eq!(sent["name"], "note.png");
    let path = PathBuf::from(sent["path"].as_str().unwrap());
    assert!(
        path.starts_with(blobs(&harness.dir)),
        "the path stays inside the store"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "the bridge reads the original bytes"
    );

    harness.stop().await;
}

#[tokio::test]
async fn an_upload_over_the_transport_limit_is_refused_by_the_engine() {
    let _env = env_lock().lock().await;
    let harness = Harness::start("upload-limit").await;
    let too_big = vec![0u8; 16 * 1024 * 1024 + 1];

    let response = harness
        .upload("huge.bin", "application/octet-stream", &too_big)
        .await;
    assert_eq!(response.status, 413);
    assert!(
        response.body_text().contains("this engine accepts"),
        "the engine's own reason, not the transport's: {}",
        response.body_text()
    );
    assert_eq!(harness.store().attachment_usage().unwrap(), 0);

    harness.stop().await;
}

#[tokio::test]
async fn unknown_hashes_and_empty_sends_are_refused() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("bad-sends").await;
    let conversation = harness.conversation_with(&"e2".repeat(16), "lan").await;

    let error = harness
        .send(&conversation, "ghost", &["ab".repeat(32)])
        .await
        .unwrap_err();
    assert!(
        error.contains("not uploaded"),
        "unexpected refusal: {error}"
    );

    let error = harness.send(&conversation, "   ", &[]).await.unwrap_err();
    assert!(error.contains("empty"), "unexpected refusal: {error}");

    assert!(harness.list(&conversation).await.is_empty());

    // Attachments without a caption are a complete message.
    let descriptor = harness
        .upload_ok("silent.png", "image/png", &png(12, 9))
        .await;
    harness
        .send(
            &conversation,
            "",
            &[descriptor["hash"].as_str().unwrap().to_string()],
        )
        .await
        .expect("a photo speaks for itself");
    let _ = harness.bridge_command().await;
    assert_eq!(harness.list(&conversation).await.len(), 1);

    harness.stop().await;
}

#[tokio::test]
async fn messages_carry_descriptors_and_plain_text_omits_them() {
    let _env = env_lock().lock().await;
    let mut harness = Harness::start("descriptors").await;
    let conversation = harness.conversation_with(&"e3".repeat(16), "lan").await;

    harness
        .send(&conversation, "just words", &[])
        .await
        .unwrap();
    let _ = harness.bridge_command().await;
    let plain = harness.list(&conversation).await;
    assert_eq!(plain.len(), 1);
    assert!(
        plain[0].get("attachments").is_none(),
        "an old client must see exactly what it used to"
    );

    let descriptor = harness
        .upload_ok("doc.pdf", "application/pdf", b"%PDF-1.7 doc")
        .await;
    let hash = descriptor["hash"].as_str().unwrap().to_string();
    harness
        .send(&conversation, "the doc", &[hash.clone()])
        .await
        .unwrap();
    let _ = harness.bridge_command().await;

    let messages = harness.list(&conversation).await;
    let with_file = messages
        .iter()
        .find(|message| message["body"] == "the doc")
        .unwrap();
    assert_eq!(with_file["kind"], "file");
    let row = attachment(with_file);
    assert_eq!(row["name"], "doc.pdf");
    assert_eq!(row["mime"], "application/pdf");
    assert_eq!(row["size"].as_u64(), Some(12));
    assert!(row.get("reason").is_none());

    let events = harness.store().events_since(0).unwrap();
    let event = events
        .iter()
        .rev()
        .find(|event| event.name == "message.new")
        .expect("a message.new event");
    assert_eq!(event.data["message"]["attachments"][0]["hash"], hash);

    harness.stop().await;
}

#[tokio::test]
async fn downloads_need_the_token_and_absent_blobs_are_404() {
    let _env = env_lock().lock().await;
    let harness = Harness::start("download-auth").await;
    let descriptor = harness
        .upload_ok("secret.png", "image/png", &png(10, 10))
        .await;
    let hash = descriptor["hash"].as_str().unwrap().to_string();

    let unauthenticated = http_get(harness.port, &format!("/attachments/{hash}")).await;
    assert_eq!(unauthenticated.status, 401);
    let wrong_token = http_get(harness.port, &format!("/attachments/{hash}?token=nope")).await;
    assert_eq!(wrong_token.status, 401);

    let unknown = harness.download(&"ab".repeat(32), None).await;
    assert_eq!(unknown.status, 404, "an unknown hash must not leak");
    assert!(unknown.body.len() < 64, "a 404 carries no bytes");

    std::fs::remove_file(blobs(&harness.dir).join(&hash[..2]).join(&hash)).unwrap();
    let evicted = harness.download(&hash, None).await;
    assert_eq!(
        evicted.status, 404,
        "an absent blob is a 404, not an empty 200"
    );

    harness.stop().await;
}
