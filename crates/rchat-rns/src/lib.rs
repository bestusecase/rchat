//! Supervises the Reticulum sidecar.
//!
//! The reference Reticulum stack is Python. Rather than reimplementing a
//! protocol with its own crypto and transport rules, the daemon runs a small
//! Python bridge and speaks newline-delimited JSON to it. The bridge owns the
//! Reticulum instance and LXMF router; the Rust side owns identity, storage,
//! and routing policy.
//!
//! Two ways to reach it, same protocol:
//!
//! * [`spawn`] runs the bridge as a child process over stdio. This is the
//!   desktop path, where Python is a separate program.
//! * [`attach`] listens on loopback and waits for the bridge to connect. This
//!   is the Android path: Python is embedded in the app process by Chaquopy,
//!   so there are no pipes to inherit and the bridge dials out instead.
//!
//! If the bridge cannot start (no Python, no script, broken install) the daemon
//! keeps running with local history and reports `offline`. Chat should degrade,
//! not crash.

use anyhow::{Context, Result};
use rchat_ipc::TransportState;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub enum BridgeEvent {
    State {
        state: TransportState,
        address: Option<String>,
        name: Option<String>,
        detail: Option<String>,
    },
    Message {
        id: String,
        from: String,
        body: String,
        ts: i64,
        /// Payloads the bridge staged on disk. Bytes never cross the JSON
        /// channel; the engine reads these paths and deletes them.
        attachments: Vec<BridgeAttachment>,
    },
    Announce {
        address: String,
        name: Option<String>,
        aspect: Option<String>,
        interface: Option<String>,
        hops: Option<u32>,
    },
    /// A sampled mesh snapshot from the bridge. The engine caches it and turns
    /// it into the client-facing status; the bridge payload carries the full
    /// path table, which no single client event should repeat.
    Mesh {
        mesh: serde_json::Value,
    },
    /// The answer to a token-correlated bridge request (`configure`, `describe`,
    /// `import_config`, `list_devices`).
    Reply {
        token: u64,
        ok: bool,
        data: Option<serde_json::Value>,
        error: Option<String>,
        /// The interface the bridge refused, when a configure failed.
        name: Option<String>,
    },
    Receipt {
        id: String,
        state: String,
    },
    Error {
        message: String,
    },
    Log {
        level: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BridgeAttachment {
    /// Where the bridge wrote the payload. Empty when it refused the bytes.
    pub path: String,
    pub name: String,
    pub ordinal: u32,
    pub size: u64,
    /// Set when the payload was not written: too large, or unreadable.
    pub reason: Option<String>,
}

pub struct Bridge {
    child: Option<Child>,
    cmd_tx: mpsc::UnboundedSender<String>,
    pub events: mpsc::UnboundedReceiver<BridgeEvent>,
    cancel: Option<oneshot::Sender<()>>,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Best effort. The process bridge also exits when its stdin closes;
        // the socket bridge sees its listener disappear when the task stops.
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

impl Bridge {
    pub fn command_sender(&self) -> mpsc::UnboundedSender<String> {
        self.cmd_tx.clone()
    }

    pub fn send(&self, value: serde_json::Value) -> Result<()> {
        self.cmd_tx
            .send(value.to_string())
            .map_err(|_| anyhow::anyhow!("reticulum bridge is not accepting commands"))
    }

    pub fn send_text(&self, id: &str, to: &str, body: &str) -> Result<()> {
        self.send(serde_json::json!({
            "cmd": "send",
            "id": id,
            "to": to,
            "body": body,
            "title": "",
        }))
    }

    pub fn announce(&self) -> Result<()> {
        self.send(serde_json::json!({ "cmd": "announce" }))
    }
}

/// Start the Python bridge as a child process.
///
/// `envs` are passed through to the child so the user can pick transport mode
/// (embedded instance, TCP server, TCP client, AutoInterface) without changing
/// code.
pub fn spawn(python: &str, script: &Path, envs: &[(&str, String)]) -> Result<Bridge> {
    let mut command = Command::new(python);
    command
        .arg("-u")
        .arg(script)
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in envs {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("starting {python} {}", script.display()))?;

    let stdin = child.stdin.take().context("bridge stdin unavailable")?;
    let stdout = child.stdout.take().context("bridge stdout unavailable")?;
    let stderr = child.stderr.take().context("bridge stderr unavailable")?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<String>();
    let (evt_tx, evt_rx) = mpsc::unbounded_channel::<BridgeEvent>();

    // Commands: Rust -> Python stdin.
    tokio::spawn(async move {
        let mut stdin = stdin;
        let mut cmd_rx = cmd_rx;
        while let Some(line) = cmd_rx.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if stdin.write_all(b"\n").await.is_err() {
                break;
            }
            if stdin.flush().await.is_err() {
                break;
            }
        }
        let _ = stdin.shutdown().await;
    });

    // Events: Python stdout -> Rust.
    let evt_tx_out = evt_tx.clone();
    tokio::spawn(async move {
        read_events(stdout, evt_tx_out).await;
    });

    // Diagnostics: Python stderr -> tracing.
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            if evt_tx
                .send(BridgeEvent::Log {
                    level: "debug".into(),
                    message: line,
                })
                .is_err()
            {
                break;
            }
        }
    });

    Ok(Bridge {
        child: Some(child),
        cmd_tx,
        events: evt_rx,
        cancel: None,
    })
}

/// Wait for an embedded bridge to connect to `listener`.
///
/// The bridge may reconnect (a crashed Python side, an app restart racing the
/// engine), so the listener stays open and every accepted connection resumes
/// the protocol. Commands issued while nothing is connected are dropped: the
/// daemon retries queued messages once transport is ready again.
pub fn attach(listener: TcpListener) -> Bridge {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<String>();
    let (evt_tx, evt_rx) = mpsc::unbounded_channel::<BridgeEvent>();
    let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let mut cmd_rx = cmd_rx;
        // Warn once if the embedded bridge never dials in: a broken Python
        // install or a crashed interpreter would otherwise leave the transport
        // at "starting" forever. The listener keeps waiting, so a late bridge
        // is still accepted.
        let mut awaiting_first = true;
        loop {
            let accepted = tokio::select! {
                _ = &mut cancel_rx => return,
                accepted = accept(&listener, awaiting_first) => accepted,
            };
            let (stream, peer) = match accepted {
                Ok(pair) => pair,
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {
                    let _ = evt_tx.send(BridgeEvent::State {
                        state: TransportState::Offline,
                        address: None,
                        name: None,
                        detail: Some("the reticulum bridge has not connected yet".into()),
                    });
                    awaiting_first = false;
                    continue;
                }
                Err(err) => {
                    let _ = evt_tx.send(BridgeEvent::State {
                        state: TransportState::Offline,
                        address: None,
                        name: None,
                        detail: Some(format!("bridge listener failed: {err}")),
                    });
                    return;
                }
            };
            awaiting_first = false;
            if !peer.ip().is_loopback() {
                // Never expose the bridge protocol off-device.
                continue;
            }

            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let offline = loop {
                tokio::select! {
                    _ = &mut cancel_rx => return,
                    line = lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if line.trim().is_empty() {
                                    continue;
                                }
                                if let Some(event) = parse_event(&line) {
                                    if evt_tx.send(event).is_err() {
                                        return;
                                    }
                                }
                            }
                            Ok(None) => break "the bridge disconnected",
                            Err(_) => break "the bridge connection failed",
                        }
                    }
                    command = cmd_rx.recv() => {
                        match command {
                            Some(line) => {
                                if write.write_all(line.as_bytes()).await.is_err()
                                    || write.write_all(b"\n").await.is_err()
                                    || write.flush().await.is_err()
                                {
                                    break "the bridge stopped accepting commands";
                                }
                            }
                            None => return,
                        }
                    }
                }
            };

            let _ = evt_tx.send(BridgeEvent::State {
                state: TransportState::Offline,
                address: None,
                name: None,
                detail: Some(offline.into()),
            });
        }
    });

    Bridge {
        child: None,
        cmd_tx,
        events: evt_rx,
        cancel: Some(cancel_tx),
    }
}

/// Accept the next bridge connection. Before the first connection, give up
/// after a while so a bridge that never starts is reported instead of silently
/// leaving the transport at "starting".
async fn accept(
    listener: &TcpListener,
    warn_if_slow: bool,
) -> std::io::Result<(tokio::net::TcpStream, std::net::SocketAddr)> {
    if !warn_if_slow {
        return listener.accept().await;
    }
    match tokio::time::timeout(std::time::Duration::from_secs(15), listener.accept()).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the bridge has not connected",
        )),
    }
}

async fn read_events<R>(stream: R, evt_tx: mpsc::UnboundedSender<BridgeEvent>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(stream).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(event) = parse_event(&line) {
                    if evt_tx.send(event).is_err() {
                        break;
                    }
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let _ = evt_tx.send(BridgeEvent::State {
        state: TransportState::Offline,
        address: None,
        name: None,
        detail: Some("bridge process exited".into()),
    });
}

fn parse_event(line: &str) -> Option<BridgeEvent> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = value.get("ev")?.as_str()?;
    match kind {
        "state" => {
            let state = match value.get("state").and_then(|v| v.as_str()) {
                Some("ready") => TransportState::Ready,
                Some("starting") => TransportState::Starting,
                _ => TransportState::Offline,
            };
            Some(BridgeEvent::State {
                state,
                address: value
                    .get("address")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                name: value
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                detail: value
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        }
        "message" => Some(BridgeEvent::Message {
            id: value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            from: value
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            body: value
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            ts: value
                .get("ts")
                .and_then(|v| v.as_i64())
                .unwrap_or_else(crate_now_ms),
            attachments: parse_attachments(&value),
        }),
        "announce" => Some(BridgeEvent::Announce {
            address: value
                .get("address")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: value
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            aspect: value
                .get("aspect")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            interface: value
                .get("interface")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            hops: value
                .get("hops")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok()),
        }),
        "mesh" => value
            .get("mesh")
            .cloned()
            .map(|mesh| BridgeEvent::Mesh { mesh }),
        "bridge_reply" => Some(BridgeEvent::Reply {
            token: value.get("token").and_then(|v| v.as_u64()).unwrap_or(0),
            ok: value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
            data: value.get("data").cloned(),
            error: value
                .get("error")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            name: value
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        }),
        "receipt" => Some(BridgeEvent::Receipt {
            id: value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            state: value
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("failed")
                .to_string(),
        }),
        "error" => Some(BridgeEvent::Error {
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown bridge error")
                .to_string(),
        }),
        "log" => Some(BridgeEvent::Log {
            level: value
                .get("level")
                .and_then(|v| v.as_str())
                .unwrap_or("debug")
                .to_string(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        }),
        _ => None,
    }
}

fn parse_attachments(value: &serde_json::Value) -> Vec<BridgeAttachment> {
    let Some(items) = value.get("attachments").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| BridgeAttachment {
            path: item
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            ordinal: item
                .get("ordinal")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(index as u32),
            size: item.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
            reason: item
                .get("reason")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
        .collect()
}

fn crate_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_event_carries_paths_not_bytes() {
        let line = r#"{"ev":"message","id":"h","from":"ab","body":"look","ts":7,
                        "attachments":[
                          {"path":"/tmp/stage/one","name":"one.png","ordinal":0,"size":12},
                          {"name":"big.pdf","ordinal":1,"size":999,
                           "reason":"Attachment was too large to receive"}]}"#;
        let Some(BridgeEvent::Message {
            attachments, body, ..
        }) = parse_event(line)
        else {
            panic!("expected a message event");
        };
        assert_eq!(body, "look");
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].path, "/tmp/stage/one");
        assert_eq!(attachments[0].ordinal, 0);
        assert_eq!(attachments[0].size, 12);
        assert!(attachments[0].reason.is_none());
        assert!(attachments[1].path.is_empty());
        assert_eq!(
            attachments[1].reason.as_deref(),
            Some("Attachment was too large to receive")
        );
    }

    #[test]
    fn a_refused_configure_names_the_interface() {
        let line = r#"{"ev":"bridge_reply","token":9,"ok":false,
                        "name":"Bad Server","error":"Reticulum could not attach interface \"Bad Server\"."}"#;
        let Some(BridgeEvent::Reply {
            ok,
            name,
            error,
            token,
            ..
        }) = parse_event(line)
        else {
            panic!("expected a reply event");
        };
        assert!(!ok);
        assert_eq!(token, 9);
        assert_eq!(name.as_deref(), Some("Bad Server"));
        assert!(error.unwrap().contains("Bad Server"));
    }

    #[test]
    fn a_plain_message_has_no_attachments() {
        let line = r#"{"ev":"message","id":"h","from":"ab","body":"hi","ts":7}"#;
        let Some(BridgeEvent::Message { attachments, .. }) = parse_event(line) else {
            panic!("expected a message event");
        };
        assert!(attachments.is_empty());
    }
}
