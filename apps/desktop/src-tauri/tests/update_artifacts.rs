//! Guards the release signing chain without launching the app.
//!
//! The updater refuses to install anything whose minisign signature does not
//! match the pinned public key, so these tests check the same thing the runtime
//! does: the generated artifact verifies, and `latest.json` describes exactly
//! that artifact. They skip when bundles have not been built yet.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use std::fs;
use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn config() -> serde_json::Value {
    let text = fs::read_to_string(manifest_dir().join("tauri.conf.json")).expect("tauri.conf.json");
    serde_json::from_str(&text).expect("valid tauri.conf.json")
}

fn pubkey() -> String {
    config()["plugins"]["updater"]["pubkey"]
        .as_str()
        .expect("plugins.updater.pubkey")
        .to_string()
}

fn find_artifact(dir: &Path, suffix: &str) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(suffix))
        })
}

#[test]
fn appimage_signature_verifies_against_pinned_key() {
    let dir = manifest_dir().join("target/release/bundle/appimage");
    let Some(appimage) = find_artifact(&dir, "_amd64.AppImage") else {
        eprintln!("skipping: build the bundle first (cargo tauri build)");
        return;
    };
    let sig_path = appimage.with_extension("AppImage.sig");
    // Tauri stores both the key and the signature base64-encoded so they can be
    // embedded in JSON; decode exactly like the runtime verifier does.
    let signature_text = String::from_utf8(
        BASE64
            .decode(fs::read_to_string(&sig_path).unwrap().trim())
            .unwrap(),
    )
    .unwrap();
    let signature = minisign_verify::Signature::decode(&signature_text).expect("signature parses");
    let key_text = String::from_utf8(BASE64.decode(pubkey()).unwrap()).unwrap();
    let key = minisign_verify::PublicKey::decode(&key_text).expect("public key parses");
    let bytes = fs::read(&appimage).unwrap();
    // `true` matches tauri-plugin-updater: signatures made by older minisign
    // versions are still accepted.
    key.verify(&bytes, &signature, true)
        .expect("artifact matches the configured update key");
}

#[test]
fn latest_json_matches_the_staged_artifact() {
    let repo_root = manifest_dir().join("../..");
    let manifest = repo_root.join("dist/latest.json");
    if !manifest.exists() {
        eprintln!("skipping: run scripts/release.mjs first");
        return;
    }
    let latest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    let version = config()["version"].as_str().unwrap().to_string();
    assert_eq!(latest["version"], version);

    let platform = &latest["platforms"]["linux-x86_64"];
    let url = platform["url"].as_str().expect("platform url");
    assert!(
        url.ends_with(&format!("rchat_{version}_amd64.AppImage")),
        "unexpected asset url: {url}"
    );
    assert!(
        String::from_utf8(
            BASE64
                .decode(platform["signature"].as_str().unwrap())
                .unwrap()
        )
        .unwrap()
        .starts_with("untrusted comment: signature from tauri secret key"),
        "signature is not a tauri minisign signature"
    );

    let sig = repo_root
        .join("apps/desktop/src-tauri/target/release/bundle/appimage")
        .join(format!("rchat_{version}_amd64.AppImage.sig"));
    if sig.exists() {
        assert_eq!(
            platform["signature"].as_str().unwrap().trim(),
            fs::read_to_string(sig).unwrap().trim(),
            "manifest signature must be the artifact signature"
        );
    }
}

#[test]
fn updater_endpoint_is_https() {
    let conf = config();
    let endpoints = conf["plugins"]["updater"]["endpoints"]
        .as_array()
        .expect("endpoints");
    assert!(!endpoints.is_empty());
    for endpoint in endpoints {
        let url = endpoint.as_str().unwrap();
        assert!(
            url.starts_with("https://"),
            "update endpoints must be https in release builds: {url}"
        );
    }
    assert_eq!(
        conf["plugins"]["updater"]["requireSignedVersion"], true,
        "the manifest version must match the version inside the signature"
    );
}
