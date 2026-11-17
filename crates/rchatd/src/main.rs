//! Standalone daemon: argument parsing, logging, and signal handling.
//!
//! The engine itself lives in the library so the mobile app can run it in
//! process. This binary is the desktop and development entry point: it parses
//! the CLI, prints the connection banner, and turns a signal into a clean stop.

use anyhow::{Context, Result};
use rchatd::{BridgeMode, Engine, Options};
use std::path::PathBuf;
use tokio::io::AsyncReadExt;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,rchatd=debug,rchat_rns=debug")),
        )
        .with_target(false)
        .init();

    let cli = Cli::from_args()?;
    let data_dir = cli.data_dir.clone();

    let engine = Engine::start(Options {
        data_dir: data_dir.clone(),
        config_dir: None,
        port: cli.port,
        no_rns: cli.no_rns,
        bridge: if cli.no_rns {
            BridgeMode::None
        } else {
            bridge_mode()
        },
        legacy_network: cli.legacy_network,
    })
    .await?;

    let endpoint = engine.endpoint();
    println!();
    println!("  rchatd {}", endpoint.version);
    println!("  data      {}", data_dir.display());
    println!("  identity  {}", engine.identity_id());
    println!("  api       {}", endpoint.http);
    println!("  ws        {}?token={}", endpoint.ws, endpoint.token);
    println!();
    println!("  Paste the token into the interface to connect.");
    println!();

    let mut stopped = engine.shutdown_handle();
    tokio::select! {
        _ = shutdown_signal(cli.supervised) => {}
        // Someone else asked the engine to stop.
        _ = stopped.changed() => {}
    }
    engine.stop();
    engine.wait().await
}

/// Desktop launch configuration. The data directory, port and bridge mode go to
/// the engine; `supervised` only matters to this process.
struct Cli {
    data_dir: PathBuf,
    port: u16,
    no_rns: bool,
    supervised: bool,
    /// Network settings a shell carried over from an older release, as JSON
    /// `{"auto_discovery": bool|null, "tcp_relay": string|null}`.
    legacy_network: Option<rchat_core::store::LegacyNetwork>,
}

impl Cli {
    fn from_args() -> Result<Self> {
        let mut data_dir = default_data_dir();
        let mut port = std::env::var("RCHAT_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8787u16);
        let mut no_rns = false;
        let mut supervised = false;
        let mut legacy_network = None;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    data_dir = args.next().context("--data-dir needs a path")?.into();
                }
                "--port" => {
                    port = args
                        .next()
                        .context("--port needs a number")?
                        .parse()
                        .context("--port must be a number")?;
                }
                "--legacy-network" => {
                    let raw = args.next().context("--legacy-network needs JSON")?;
                    let parsed: LegacyNetworkArg = serde_json::from_str(&raw)
                        .context("--legacy-network must be a JSON object")?;
                    legacy_network = Some(parsed.into());
                }
                "--no-rns" => no_rns = true,
                // The desktop shell holds our stdin open. EOF means the shell
                // exited, including when it was killed, so shut down cleanly.
                "--supervised" => supervised = true,
                "-h" | "--help" => {
                    println!(
                        "rchatd [--data-dir PATH] [--port PORT] [--no-rns] \
                         [--legacy-network JSON] [--supervised]"
                    );
                    std::process::exit(0);
                }
                other => warn!(arg = other, "ignoring unknown argument"),
            }
        }
        Ok(Self {
            data_dir,
            port,
            no_rns,
            supervised,
            legacy_network,
        })
    }
}

/// The shell's old network settings, as it writes them into its launch flag.
#[derive(serde::Deserialize)]
struct LegacyNetworkArg {
    #[serde(default)]
    auto_discovery: Option<bool>,
    #[serde(default)]
    tcp_relay: Option<String>,
}

impl From<LegacyNetworkArg> for rchat_core::store::LegacyNetwork {
    fn from(arg: LegacyNetworkArg) -> Self {
        Self {
            auto_discovery: arg.auto_discovery,
            tcp_relay: arg.tcp_relay,
        }
    }
}

fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RCHAT_DATA_DIR") {
        return dir.into();
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("rchat");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/share/rchat")
}

/// Describe the stdio bridge child. Transport options are environment variables
/// so the user can switch between LAN, TCP relay and radio without a rebuild.
fn bridge_mode() -> BridgeMode {
    let Some(script) = find_sidecar() else {
        warn!("the Reticulum bridge script was not found; running with local history only");
        return BridgeMode::None;
    };
    let python = std::env::var("RCHAT_PYTHON").unwrap_or_else(|_| "python3".to_string());
    let mut envs = Vec::new();
    for key in [
        "RCHAT_RNS_MODE",
        "RCHAT_RNS_TCP_SERVER",
        "RCHAT_RNS_TCP_CLIENT",
        "RCHAT_RNS_AUTO",
        "RCHAT_RNS_GROUP",
        "RCHAT_RNS_CONFIGDIR",
    ] {
        if let Ok(value) = std::env::var(key) {
            envs.push((key.to_string(), value));
        }
    }
    BridgeMode::Spawn {
        python,
        script,
        envs,
    }
}

fn find_sidecar() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RCHAT_SIDECAR") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    let mut candidates = vec![PathBuf::from("sidecar/bridge.py")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("sidecar/bridge.py"));
            candidates.push(dir.join("../sidecar/bridge.py"));
            candidates.push(dir.join("../../../sidecar/bridge.py"));
        }
    }
    candidates.into_iter().find(|p| p.exists())
}

async fn shutdown_signal(supervised: bool) {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let parent_closed = async {
        if !supervised {
            std::future::pending::<()>().await;
            return;
        }
        // rchatd is launched with a piped stdin. Reading to EOF tells us the
        // desktop shell is gone even if it never got to send a signal.
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 256];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    };

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
        _ = parent_closed => {}
    }
    info!("shutting down");
}
