//! Supervises the Reticulum sidecar.
//!
//! The reference Reticulum stack is Python. Rather than reimplementing a
//! protocol with its own crypto and transport rules, the daemon runs a small
//! Python bridge as a child process and speaks newline-delimited JSON to it.
//! The bridge owns the Reticulum instance and LXMF router; the Rust side owns
//! identity, storage, and routing policy.
//!
//! If the bridge cannot start (no Python, no script, broken install) the daemon
//! keeps running with local history and reports `offline`. Chat should degrade,
//! not crash.

use anyhow::{Context, Result};
use rchat_ipc::TransportState;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

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
    },
    Announce {
        address: String,
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

pub struct Bridge {
    child: Child,
    cmd_tx: mpsc::UnboundedSender<String>,
    pub events: mpsc::UnboundedReceiver<BridgeEvent>,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Best effort. The bridge also exits when its stdin closes.
        let _ = self.child.start_kill();
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

/// Start the Python bridge.
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

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
    let (evt_tx, evt_rx) = mpsc::unbounded_channel::<BridgeEvent>();

    // Commands: Rust -> Python stdin.
    tokio::spawn(async move {
        let mut stdin = stdin;
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
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Some(event) = parse_event(&line) {
                        if evt_tx_out.send(event).is_err() {
                            break;
                        }
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        let _ = evt_tx_out.send(BridgeEvent::State {
            state: TransportState::Offline,
            address: None,
            name: None,
            detail: Some("bridge process exited".into()),
        });
    });

    // Diagnostics: Python stderr -> tracing.
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            if evt_tx.send(BridgeEvent::Log {
                level: "debug".into(),
                message: line,
            }).is_err()
            {
                break;
            }
        }
    });

    Ok(Bridge {
        child,
        cmd_tx,
        events: evt_rx,
    })
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
                address: value.get("address").and_then(|v| v.as_str()).map(str::to_string),
                name: value.get("name").and_then(|v| v.as_str()).map(str::to_string),
                detail: value.get("detail").and_then(|v| v.as_str()).map(str::to_string),
            })
        }
        "message" => Some(BridgeEvent::Message {
            id: value.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            from: value.get("from").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            body: value.get("body").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            ts: value.get("ts").and_then(|v| v.as_i64()).unwrap_or_else(crate_now_ms),
        }),
        "announce" => Some(BridgeEvent::Announce {
            address: value.get("address").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            name: value.get("name").and_then(|v| v.as_str()).map(str::to_string),
        }),
        "receipt" => Some(BridgeEvent::Receipt {
            id: value.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            state: value.get("state").and_then(|v| v.as_str()).unwrap_or("failed").to_string(),
        }),
        "error" => Some(BridgeEvent::Error {
            message: value.get("message").and_then(|v| v.as_str()).unwrap_or("unknown bridge error").to_string(),
        }),
        "log" => Some(BridgeEvent::Log {
            level: value.get("level").and_then(|v| v.as_str()).unwrap_or("debug").to_string(),
            message: value.get("message").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        }),
        _ => None,
    }
}

fn crate_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
