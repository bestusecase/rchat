//! Real end-to-end check of `BridgeMode::Attach` with the actual Reticulum
//! bridge, not a hand-written event. Needs a Python 3 with `rns` and `lxmf`
//! installed (the desktop dev environment has one).
//!
//!     cargo test -p rchatd --test socket_bridge -- --ignored --nocapture
//!
//! Ignored by default: a plain `cargo test` should not depend on Python.

use rchatd::{BridgeMode, Engine, Options};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;

fn scratch_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rchatd-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn health(port: u16) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .ok()?;
    let mut body = String::new();
    stream.read_to_string(&mut body).await.ok()?;
    Some(body)
}

async fn wait_for(predicate: impl Fn(&str) -> bool, port: u16, secs: u64) -> String {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut last = String::new();
    while Instant::now() < deadline {
        if let Some(body) = health(port).await {
            if predicate(&body) {
                return body;
            }
            last = body;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("health never matched; last response: {last}");
}

#[tokio::test]
#[ignore = "needs python3 with RNS and LXMF installed"]
async fn the_real_bridge_connects_over_the_socket() {
    let python = std::env::var("RCHAT_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let sidecar = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sidecar");
    let dir = scratch_dir("socket-bridge");
    // bridge.py was run from the package dir in development; keep the import
    // path explicit so the test works from anywhere.
    let rns_dir = dir.join("rns");

    let engine = Engine::start(Options {
        data_dir: dir.clone(),
        port: 0,
        no_rns: false,
        bridge: BridgeMode::Attach,
        legacy_network: None,
    })
    .await
    .expect("engine should start");

    let endpoint = engine.endpoint().clone();
    let bridge_port = endpoint.bridge_port.expect("attach mode port");
    wait_for(
        |body| body.contains("\"transport\":\"starting\""),
        endpoint.port,
        5,
    )
    .await;

    // Exactly what the Android shell will call through Chaquopy.
    let script = format!(
        "import sys; sys.path.insert(0, {sidecar:?}); import bridge; sys.exit(bridge.main({port}, {dir:?}, 'Pixel'))",
        sidecar = sidecar.to_string_lossy(),
        port = bridge_port,
        dir = rns_dir.to_string_lossy(),
    );
    let mut child = Command::new(&python)
        .arg("-c")
        .arg(&script)
        .kill_on_drop(true)
        .spawn()
        .expect("spawning python");

    let body = wait_for(|body| body.contains("\"transport\":\"ready\""), endpoint.port, 90).await;
    println!("ready: {body}");
    // Reticulum must have stored the LXMF address for this bridge instance.
    let address = body
        .split("\"address\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("health should carry the address")
        .to_string();
    assert_eq!(address.len(), 32, "not a hex LXMF address: {address}");
    assert!(rns_dir.join("identity").exists(), "bridge identity was not stored");
    assert!(
        rns_dir.join("config").exists(),
        "bridge config was not written"
    );

    // Losing the bridge must surface as offline, not as a hang.
    child.start_kill().unwrap();
    let _ = child.wait().await;
    let body = wait_for(|body| body.contains("\"transport\":\"offline\""), endpoint.port, 10).await;
    println!("after kill: {body}");

    engine.stop();
    engine.wait().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
