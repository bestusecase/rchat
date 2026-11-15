mod app;

use anyhow::{bail, Context, Result};
use rchat_core::store::Store;
use rchat_ipc::TransportState;
use rchat_rns::BridgeEvent;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
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

    let options = Options::from_args()?;
    let data_dir = options.data_dir.clone();
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;
    restrict_permissions(&data_dir);

    // One engine per data directory. A lock left behind by a crashed engine is
    // reclaimed once its pid is gone, so a user never has to clean up by hand.
    let _engine_lock = EngineLock::acquire(&data_dir)?;

    let store = Arc::new(Store::open(&data_dir.join("rchat.db"))?);
    let identity = store.ensure_identity()?;
    info!(id = %identity.id, name = %identity.display_name, "identity ready");

    let token = resolve_token(&data_dir)?;
    let state = Arc::new(app::AppState::new(store.clone(), token.clone()));

    if !options.no_rns {
        match start_bridge(&data_dir, &identity.display_name) {
            Some(mut bridge) => {
                info!("reticulum bridge started");
                state.attach_transport(bridge.command_sender()).await;
                let pump = state.clone();
                tokio::spawn(async move {
                    while let Some(event) = bridge.events.recv().await {
                        pump.on_bridge_event(event).await;
                    }
                    pump.on_bridge_event(BridgeEvent::State {
                        state: TransportState::Offline,
                        address: None,
                        name: None,
                        detail: Some("reticulum bridge stopped".into()),
                    })
                    .await;
                });
            }
            None => warn!("running without Reticulum; local history only"),
        }
    }

    // Retry anything queued while offline. Cheap: no work unless transport is ready.
    {
        let flush = state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(15));
            loop {
                ticker.tick().await;
                flush.flush_queued().await;
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", options.port))
        .await
        .context("binding local API port (is another rchatd running?)")?;
    let addr = listener.local_addr()?;

    let endpoint = serde_json::json!({
        "http": format!("http://{addr}"),
        "ws": format!("ws://{addr}/ws"),
        "token": token,
        "pid": std::process::id(),
        "version": app::VERSION,
    });
    let endpoint_path = data_dir.join("endpoint.json");
    let endpoint_bytes = serde_json::to_vec_pretty(&endpoint)?;
    let tmp_endpoint = data_dir.join("endpoint.json.tmp");
    std::fs::write(&tmp_endpoint, &endpoint_bytes)?;
    restrict_file(&tmp_endpoint);
    std::fs::rename(&tmp_endpoint, &endpoint_path)?;

    println!();
    println!("  rchatd {}", app::VERSION);
    println!("  data      {}", data_dir.display());
    println!("  identity  {}", identity.id);
    println!("  api       http://{addr}");
    println!("  ws        ws://{addr}/ws?token={token}");
    println!();
    println!("  Paste the token into the interface to connect.");
    println!();

    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(shutdown_signal(options.supervised))
        .await?;

    // A stale endpoint would make the next launch try a dead address.
    let _ = std::fs::remove_file(&endpoint_path);
    Ok(())
}

/// Exclusive per-data-directory engine lock. Contains the owning pid so a lock
/// left by a crash can be recognised and replaced.
struct EngineLock {
    path: PathBuf,
}

impl EngineLock {
    fn acquire(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join("engine.lock");
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    restrict_file(&path);
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let pid = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| text.trim().parse::<i32>().ok());
                    match pid {
                        Some(pid) if process_alive(pid) => bail!(
                            "another rchatd is already using {} (pid {pid})",
                            data_dir.display()
                        ),
                        _ => {
                            // Stale lock from a crash or a pid that is gone.
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("locking {}", path.display()));
                }
            }
        }
        bail!("could not lock {}", path.display());
    }
}

impl Drop for EngineLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        return PathBuf::from(format!("/proc/{pid}")).exists();
    }
    #[cfg(not(unix))]
    {
        true
    }
}

struct Options {
    data_dir: PathBuf,
    port: u16,
    no_rns: bool,
    supervised: bool,
}

impl Options {
    fn from_args() -> Result<Self> {
        let mut data_dir = default_data_dir();
        let mut port = std::env::var("RCHAT_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8787u16);
        let mut no_rns = false;
        let mut supervised = false;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    data_dir = args
                        .next()
                        .context("--data-dir needs a path")?
                        .into();
                }
                "--port" => {
                    port = args
                        .next()
                        .context("--port needs a number")?
                        .parse()
                        .context("--port must be a number")?;
                }
                "--no-rns" => no_rns = true,
                // The desktop shell holds our stdin open. EOF means the shell
                // exited, including when it was killed, so shut down cleanly.
                "--supervised" => supervised = true,
                "-h" | "--help" => {
                    println!(
                        "rchatd [--data-dir PATH] [--port PORT] [--no-rns] [--supervised]"
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
        })
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

fn resolve_token(data_dir: &Path) -> Result<String> {
    let path = data_dir.join("token");
    if let Ok(token) = std::env::var("RCHAT_TOKEN") {
        let token = token.trim().to_string();
        if token.len() >= 16 {
            // Persist a token supplied by the desktop shell so a later direct
            // launch reuses the same secret.
            if std::fs::read_to_string(&path).ok().as_deref().map(str::trim) != Some(&token) {
                std::fs::write(&path, &token)?;
                restrict_file(&path);
            }
            return Ok(token);
        }
    }
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if trimmed.len() >= 16 {
            return Ok(trimmed.to_string());
        }
    }
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("generating API token: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&path, &token)?;
    restrict_file(&path);
    Ok(token)
}

fn start_bridge(data_dir: &Path, display_name: &str) -> Option<rchat_rns::Bridge> {
    let script = find_sidecar()?;
    let python = std::env::var("RCHAT_PYTHON").unwrap_or_else(|_| "python3".to_string());

    let mut envs = vec![
        (
            "RCHAT_RNS_DIR",
            data_dir.join("rns").to_string_lossy().to_string(),
        ),
        ("RCHAT_DISPLAY_NAME", display_name.to_string()),
    ];
    for key in [
        "RCHAT_RNS_MODE",
        "RCHAT_RNS_TCP_SERVER",
        "RCHAT_RNS_TCP_CLIENT",
        "RCHAT_RNS_AUTO",
        "RCHAT_RNS_GROUP",
        "RCHAT_RNS_CONFIGDIR",
    ] {
        if let Ok(value) = std::env::var(key) {
            envs.push((key, value));
        }
    }

    match rchat_rns::spawn(&python, &script, &envs) {
        Ok(bridge) => Some(bridge),
        Err(err) => {
            warn!(error = %err, "could not start the Reticulum bridge");
            None
        }
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

fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
}

fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
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
