//! End-to-end checks for the engine library: the attach handshake the mobile
//! app relies on, and recovery from a lock left by a crashed engine.

use rchatd::{BridgeMode, Engine, Options};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

fn scratch_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rchatd-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn options(dir: &Path, bridge: BridgeMode) -> Options {
    Options {
        data_dir: dir.to_path_buf(),
        config_dir: None,
        port: 0,
        no_rns: false,
        bridge,
        legacy_network: None,
    }
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

async fn wait_for(predicate: impl Fn(&str) -> bool, port: u16) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    while Instant::now() < deadline {
        if let Some(body) = health(port).await {
            if predicate(&body) {
                return body;
            }
            last = body;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("health never matched; last response: {last}");
}

/// The mobile path: the engine binds a loopback port before Python exists, the
/// embedded bridge dials in, and its events reach the API.
#[tokio::test]
async fn attach_bridge_dials_in_and_reports_ready() {
    let dir = scratch_dir("attach");
    let engine = Engine::start(options(&dir, BridgeMode::Attach))
        .await
        .expect("engine should start in attach mode");

    let endpoint = engine.endpoint().clone();
    let bridge_port = endpoint
        .bridge_port
        .expect("attach mode must publish a bridge port");
    assert!(bridge_port > 0 && bridge_port != endpoint.port);

    // The handshake has not happened yet, so the API reports starting.
    let body = wait_for(
        |body| body.contains("\"transport\":\"starting\""),
        endpoint.port,
    )
    .await;
    assert!(body.contains("\"ok\":true"), "unexpected health: {body}");

    // The bridge connects and announces it is ready, exactly as bridge.py does.
    let mut bridge = TcpStream::connect(("127.0.0.1", bridge_port))
        .await
        .expect("bridge port should accept");
    let address = "ab".repeat(16);
    bridge
        .write_all(
            format!(
                "{{\"ev\":\"state\",\"state\":\"ready\",\"address\":\"{address}\",\"name\":\"phone\"}}\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    // A started bridge alone is not a working mesh: until an interface reports
    // online, the API says degraded rather than ready.
    let body = wait_for(
        |body| body.contains("\"transport\":\"degraded\""),
        endpoint.port,
    )
    .await;
    assert!(
        body.contains(&address),
        "the announced address should be persisted: {body}"
    );

    bridge
        .write_all(
            format!(
                "{{\"ev\":\"mesh\",\"mesh\":{{\"transport\":\"ready\",\"address\":\"{address}\",\
                  \"interfaces\":[{{\"name\":\"LAN\",\"kind\":\"AutoInterface\",\"online\":true}}],\
                  \"counters\":{{}},\"paths\":[],\"other_services\":[]}}}}\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let body = wait_for(
        |body| body.contains("\"transport\":\"ready\""),
        endpoint.port,
    )
    .await;
    assert!(
        body.contains(&address),
        "the announced address should be persisted: {body}"
    );

    // Shutdown closes the API and the socket, and removes the discovery files.
    let shutdown = engine.shutdown_handle();
    engine.stop();
    assert!(*shutdown.borrow(), "stop must be visible on the handle");
    engine.wait().await.expect("clean shutdown");
    assert!(!dir.join("endpoint.json").exists());
    assert!(!dir.join("engine.lock").exists());
    assert!(
        TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .is_err(),
        "the API port should be closed after wait()"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Hosts can observe the engine without a socket: the Android shell posts
/// message notifications from this feed while its webview is suspended. A
/// received bridge message must show up as `message.new` with the sender as
/// the author, and the store must already count it as unread.
#[tokio::test]
async fn engine_events_expose_incoming_messages() {
    let dir = scratch_dir("events");
    let engine = Engine::start(options(&dir, BridgeMode::Attach))
        .await
        .expect("engine should start in attach mode");
    let endpoint = engine.endpoint().clone();
    let bridge_port = endpoint
        .bridge_port
        .expect("attach mode must publish a bridge port");
    let mut events = engine.events();

    let mut bridge = TcpStream::connect(("127.0.0.1", bridge_port))
        .await
        .expect("bridge port should accept");
    let local = "ab".repeat(16);
    bridge
        .write_all(
            format!(
                "{{\"ev\":\"state\",\"state\":\"ready\",\"address\":\"{local}\",\"name\":\"phone\"}}\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    // Any address but our own: the engine drops messages that look like echoes
    // of something this node sent.
    let sender = "cd".repeat(16);
    bridge
        .write_all(
            format!(
                "{{\"ev\":\"message\",\"id\":\"deadbeef\",\"from\":\"{sender}\",\"body\":\"hello\",\"ts\":1}}\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value.get("name").and_then(|field| field.as_str()) == Some("message.new") {
                        return value;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("the event feed closed before the message arrived")
                }
            }
        }
    })
    .await
    .expect("no message.new event arrived");

    assert_eq!(
        event
            .pointer("/data/message/author_id")
            .and_then(|field| field.as_str()),
        Some(sender.as_str())
    );
    let conversations = engine.store().list_conversations().unwrap();
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].unread, 1, "the new message starts unread");

    engine.stop();
    engine.wait().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fresh install has local discovery before any configuration, and an
/// upgrade carries the old toggle and relay into interface rows exactly once.
#[tokio::test]
async fn first_run_seeds_discovery_and_carries_legacy_network_settings() {
    use rchat_core::store::{LegacyNetwork, DEFAULT_DISCOVERY_NAME, LEGACY_RELAY_NAME};

    let dir = scratch_dir("first-run");
    let legacy = LegacyNetwork {
        auto_discovery: Some(true),
        tcp_relay: Some("relay.example:4242".into()),
    };
    let start = |dir: PathBuf, legacy: Option<LegacyNetwork>| {
        Engine::start(Options {
            data_dir: dir,
            config_dir: None,
            port: 0,
            no_rns: false,
            bridge: BridgeMode::Attach,
            legacy_network: legacy,
        })
    };

    let engine = start(dir.clone(), Some(legacy.clone()))
        .await
        .expect("engine should start");
    let rows = engine.store().list_interfaces().unwrap();
    assert_eq!(rows.len(), 2);
    let discovery = rows
        .iter()
        .find(|row| row.name == DEFAULT_DISCOVERY_NAME)
        .expect("discovery row");
    assert!(discovery.enabled, "discovery is on from first run");
    let relay = rows
        .iter()
        .find(|row| row.name == LEGACY_RELAY_NAME)
        .expect("legacy relay row");
    assert_eq!(relay.config["target_host"], "relay.example");
    assert_eq!(relay.config["target_port"], 4242);
    assert_eq!(relay.origin, "legacy");
    engine.stop();
    engine.wait().await.unwrap();

    // A second start supplies the same settings and must not duplicate rows.
    let engine = start(dir.clone(), Some(legacy))
        .await
        .expect("engine should start again");
    assert_eq!(engine.store().list_interfaces().unwrap().len(), 2);
    engine.stop();
    engine.wait().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A hand-written `rns/config` is the user's, not a seed target: first-run
/// setup leaves it and the (empty) row list alone until the bridge imports it.
#[tokio::test]
async fn a_hand_written_config_is_not_overwritten_by_first_run_seeding() {
    use rchat_core::store::{NetworkSource, NETWORK_IMPORTED};

    let dir = scratch_dir("hand-written");
    std::fs::create_dir_all(dir.join("rns")).unwrap();
    let config = "[interfaces]\n  [[Mine]]\n    type = TCPClientInterface\n    \
                  target_host = mine.example\n    target_port = 4242\n";
    std::fs::write(dir.join("rns/config"), config).unwrap();

    let engine = Engine::start(options(&dir, BridgeMode::Attach))
        .await
        .expect("engine should start");
    assert!(
        engine.store().list_interfaces().unwrap().is_empty(),
        "importing the config is the bridge's job, not a reason to seed"
    );
    assert_eq!(
        engine.store().meta(NETWORK_IMPORTED).unwrap(),
        None,
        "the gate stays open until the config has actually been read"
    );
    assert_eq!(
        engine.store().network_source().unwrap(),
        None,
        "nothing has settled, so there is no source to record"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("rns/config")).unwrap(),
        config,
        "the config must be byte-identical before the bridge reads it"
    );

    // The embedded bridge dials in and reports ready. The engine then asks it
    // to import the hand-written config; reply with what bridge.py would parse
    // from the file above: one TCP interface and no extra sections.
    let bridge_port = engine
        .endpoint()
        .bridge_port
        .expect("attach mode must publish a bridge port");
    let bridge = TcpStream::connect(("127.0.0.1", bridge_port))
        .await
        .expect("bridge port should accept");
    let (read, mut write) = bridge.into_split();
    write
        .write_all(
            format!(
                "{{\"ev\":\"state\",\"state\":\"ready\",\"address\":\"{}\",\"name\":\"test\"}}\n",
                "ab".repeat(16)
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let command: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
            let token = command
                .get("token")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            let reply = match command.get("cmd").and_then(serde_json::Value::as_str) {
                Some("import_config") => serde_json::json!({
                    "ev": "bridge_reply",
                    "token": token,
                    "ok": true,
                    "data": {
                        "interfaces": [{
                            "name": "Mine",
                            "kind": "TCPClientInterface",
                            "config": {"target_host": "mine.example", "target_port": 4242},
                            "enabled": true
                        }],
                        "extra": [],
                        "existed": true
                    }
                }),
                _ => serde_json::json!({
                    "ev": "bridge_reply",
                    "token": token,
                    "ok": true,
                    "data": {}
                }),
            };
            if write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
            let _ = write.flush().await;
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if engine.store().meta(NETWORK_IMPORTED).unwrap().as_deref() == Some("1") {
            break;
        }
        if Instant::now() >= deadline {
            panic!("the config import never settled the legacy gate");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        engine.store().network_source().unwrap(),
        Some(NetworkSource::Config)
    );
    let rows = engine.store().list_interfaces().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].origin, "imported");
    engine.stop();
    engine.wait().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A crash leaves `engine.lock` behind. The restart must notice the owner is
/// gone and take over, otherwise the phone can never start the engine again.
#[tokio::test]
async fn a_stale_lock_does_not_block_a_restart() {
    let dir = scratch_dir("stale-lock");

    // A pid that cannot exist, written in both lock formats.
    for contents in ["999999", "999999 1234"] {
        std::fs::write(dir.join("engine.lock"), contents).unwrap();
        let engine = Engine::start(Options {
            data_dir: dir.clone(),
            config_dir: None,
            port: 0,
            no_rns: true,
            bridge: BridgeMode::None,
            legacy_network: None,
        })
        .await
        .unwrap_or_else(|err| panic!("stale lock {contents:?} blocked start: {err}"));

        // The lock now belongs to us and holds a start time on Linux/Android.
        let lock = std::fs::read_to_string(dir.join("engine.lock")).unwrap();
        assert!(lock.starts_with(&std::process::id().to_string()));
        #[cfg(any(target_os = "linux", target_os = "android"))]
        assert_eq!(lock.split_whitespace().count(), 2, "lock: {lock:?}");

        engine.stop();
        engine.wait().await.unwrap();
    }

    // Restarting twice in the same process (same pid) must also work: the
    // leftover lock carries our own pid and is not a competitor.
    let engine = Engine::start(Options {
        data_dir: dir.clone(),
        config_dir: None,
        port: 0,
        no_rns: true,
        bridge: BridgeMode::None,
        legacy_network: None,
    })
    .await
    .expect("a second engine in the same process should start");
    engine.stop();
    engine.wait().await.unwrap();

    let _ = std::fs::remove_dir_all(&dir);
}
