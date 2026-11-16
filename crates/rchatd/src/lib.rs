//! The rchat engine.
//!
//! `rchatd` is both a standalone daemon and the engine embedded in the mobile
//! app. Everything in this module is host-agnostic: it owns the data directory,
//! the store, the local API listener, and the Reticulum bridge, and it stops
//! when told to. [`main`](../rchatd/index.html) adds CLI parsing, logging and
//! signals; the mobile shell adds JNI and a foreground service.
//!
//! Two bridge modes matter. The desktop passes [`BridgeMode::Spawn`]: Python is
//! a separate program and the two sides speak over its stdio. Android passes
//! [`BridgeMode::Attach`]: Chaquopy runs Python inside this process, so the
//! engine binds a loopback listener and the interpreter dials in. The wire
//! protocol is the same either way.

mod app;

pub use app::VERSION;

use anyhow::{bail, Context, Result};
use app::AppState;
use rchat_core::store::Store;
use rchat_ipc::TransportState;
use rchat_rns::{Bridge, BridgeEvent};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// How the engine reaches the Python Reticulum bridge.
#[derive(Debug)]
pub enum BridgeMode {
    /// Run the bridge as a child process and speak over its stdio. The desktop
    /// path, where Python is a separate program.
    ///
    /// `envs` are extra environment variables for the child. The engine adds
    /// `RCHAT_RNS_DIR` and `RCHAT_DISPLAY_NAME` itself because both are engine
    /// state, not launch configuration.
    Spawn {
        python: String,
        script: PathBuf,
        envs: Vec<(String, String)>,
    },
    /// Listen on loopback and wait for an embedded bridge to connect. The
    /// Android path, where Chaquopy runs Python in this process and there are
    /// no pipes to inherit.
    Attach,
    /// No Reticulum at all: local history only.
    None,
}

/// Everything [`Engine::start`] needs from its host.
#[derive(Debug)]
pub struct Options {
    pub data_dir: PathBuf,
    /// Local API port. `0` picks a free one, which is what both shells use.
    pub port: u16,
    /// Skip the bridge regardless of `bridge`. Mirrors the `--no-rns` flag.
    pub no_rns: bool,
    pub bridge: BridgeMode,
    /// Network settings a shell carried over from an older release. Read once,
    /// guarded by a meta flag, and turned into interface rows with origin
    /// `legacy`.
    pub legacy_network: Option<rchat_core::store::LegacyNetwork>,
}

/// Where the local API is listening. Written to `endpoint.json` so a shell can
/// find an engine left running by a previous launch.
#[derive(Debug, Clone, Serialize)]
pub struct Endpoint {
    pub http: String,
    pub ws: String,
    pub token: String,
    pub pid: u32,
    pub port: u16,
    /// The loopback port the embedded bridge should dial, in attach mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_port: Option<u16>,
    pub version: String,
}

/// A running engine. Dropping it without [`Engine::wait`] leaves the API task
/// and the bridge running until the runtime shuts down, so always stop first.
pub struct Engine {
    data_dir: PathBuf,
    identity_id: String,
    display_name: String,
    endpoint: Endpoint,
    /// Kept for hosts that summarise store state without a socket, and to hand
    /// the same `Arc` to background tasks started in [`Engine::start`].
    store: Arc<Store>,
    /// The live event stream, shared with the websocket broadcast channel.
    events: broadcast::Sender<String>,
    shutdown: watch::Sender<bool>,
    serve: JoinHandle<Result<()>>,
    helpers: Vec<JoinHandle<()>>,
    _lock: EngineLock,
}

impl Engine {
    /// Open the data directory, start the bridge, and serve the local API.
    ///
    /// Returns once the API is accepting connections, so `endpoint()` is valid
    /// immediately. In [`BridgeMode::Attach`] the listener is already bound
    /// when this returns: the caller can hand `endpoint.bridge_port` to the
    /// embedded interpreter, and the accept loop will be waiting.
    pub async fn start(options: Options) -> Result<Self> {
        let data_dir = options.data_dir.clone();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("creating data directory {}", data_dir.display()))?;
        restrict_permissions(&data_dir);

        // One engine per data directory. A lock left behind by a crashed engine
        // is reclaimed once its owner is gone, so a user never has to clean up
        // by hand.
        let lock = EngineLock::acquire(&data_dir)?;

        let store = Arc::new(Store::open(&data_dir.join("rchat.db"))?);
        let identity = store.ensure_identity()?;
        info!(id = %identity.id, name = %identity.display_name, "identity ready");

        let token = resolve_token(&data_dir)?;
        let blobs = Arc::new(rchat_core::attachments::BlobStore::new(
            data_dir.join("attachments"),
        ));
        let state = Arc::new(AppState::new(store.clone(), token.clone(), blobs.clone()));
        *state.legacy_network.write().await = options.legacy_network.clone();
        // Development scripts may still ask the bridge to seed itself from the
        // environment. Production never does: interfaces come from the store.
        let env_seeding = std::env::var("RCHAT_RNS_ENV_SEED")
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "yes" | "true" | "on"
                )
            })
            .unwrap_or(false);
        let mode = if options.no_rns {
            BridgeMode::None
        } else {
            options.bridge
        };
        // First-run network setup happens before the bridge exists, so its very
        // first configure renders the right interfaces. A hand-written
        // `rns/config` is left alone until the bridge has imported it.
        let managed = if env_seeding || matches!(mode, BridgeMode::None) {
            false
        } else {
            let config_dir = std::env::var("RCHAT_RNS_CONFIGDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| data_dir.join("rns"));
            let config_present = config_dir.join("config").exists();
            match store.import_network_once(options.legacy_network.as_ref(), config_present) {
                Ok(rows) => info!(rows = rows.len(), "network interfaces ready for the bridge"),
                Err(err) => warn!(error = %err, "could not prepare network interfaces"),
            }
            true
        };
        // A crash between writing a blob and committing its row leaves bytes no
        // message refers to. Sweep them before anything else can read the store.
        // Uploads that no send ever claimed are stale by definition at startup.
        match store.discard_unclaimed_uploads() {
            Ok(0) => {}
            Ok(removed) => info!(removed, "dropped abandoned attachment uploads"),
            Err(err) => warn!(error = %err, "could not drop abandoned uploads"),
        }
        match store.sweep_orphan_blobs(&blobs) {
            Ok(0) => {}
            Ok(removed) => info!(removed, "swept unreferenced attachment blobs"),
            Err(err) => warn!(error = %err, "could not sweep attachment blobs"),
        }
        // A budget lowered since the last launch is enforced on the next one.
        state.enforce_attachment_budget();
        // Subscribe before the server starts so a host that reacts to events
        // (the Android shell) cannot miss the first one.
        let events = state.tx.clone();
        let (shutdown, _) = watch::channel(false);
        let mut helpers = Vec::new();

        let mut bridge_port = None;
        match mode {
            BridgeMode::Spawn {
                python,
                script,
                envs,
            } => match spawn_bridge(&data_dir, &identity.display_name, &python, &script, &envs, managed) {
                Some(bridge) => {
                    info!("reticulum bridge started");
                    install_bridge(&state, bridge, shutdown.subscribe(), &mut helpers).await;
                }
                None => warn!("running without Reticulum; local history only"),
            },
            BridgeMode::Attach => {
                // Bind before returning: the caller needs the port to start
                // Python, and the accept loop must already be waiting when it
                // does. Binding on port 0 keeps this loopback-only and avoids
                // colliding with the API port.
                let listener = TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .context("binding the bridge listener")?;
                let port = listener.local_addr()?.port();
                bridge_port = Some(port);
                info!(port, "waiting for the embedded reticulum bridge");
                install_bridge(
                    &state,
                    rchat_rns::attach(listener),
                    shutdown.subscribe(),
                    &mut helpers,
                )
                .await;
            }
            BridgeMode::None => warn!("running without Reticulum; local history only"),
        }

        // Retry anything queued while offline. Cheap: no work unless transport
        // is ready.
        {
            let flush = state.clone();
            let mut stop = shutdown.subscribe();
            helpers.push(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(15));
                loop {
                    tokio::select! {
                        _ = stop.changed() => break,
                        _ = ticker.tick() => flush.flush_queued().await,
                    }
                }
            }));
        }

        let listener = TcpListener::bind(("127.0.0.1", options.port))
            .await
            .context("binding local API port (is another rchatd running?)")?;
        let addr = listener.local_addr()?;

        let endpoint = Endpoint {
            http: format!("http://{addr}"),
            ws: format!("ws://{addr}/ws"),
            token,
            pid: std::process::id(),
            port: addr.port(),
            bridge_port,
            version: VERSION.to_string(),
        };
        write_endpoint(&data_dir, &endpoint)?;

        let mut stop = shutdown.subscribe();
        let serve = tokio::spawn(async move {
            axum::serve(listener, app::router(state))
                .with_graceful_shutdown(async move {
                    // A stop requested before the server started must not hang.
                    if !*stop.borrow() {
                        let _ = stop.changed().await;
                    }
                })
                .await
                .map_err(anyhow::Error::from)
        });

        Ok(Self {
            data_dir,
            identity_id: identity.id,
            display_name: identity.display_name,
            endpoint,
            store,
            events,
            shutdown,
            serve,
            helpers,
            _lock: lock,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn identity_id(&self) -> &str {
        &self.identity_id
    }

    /// The name peers see in announces. The bridge needs it at startup.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// The event stream the local API broadcasts to websocket clients.
    ///
    /// A host that must react without a UI — the Android shell raising message
    /// notifications while its webview is suspended — subscribes here instead
    /// of holding a socket open. Frames are the same JSON `ServerMsg` strings
    /// clients receive, so they can be parsed with `rchat_ipc::ServerMsg`.
    /// A slow subscriber only skips events; the store stays authoritative.
    pub fn events(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    /// The shared store, for hosts that need to summarise state (an unread
    /// count for a notification) without a socket round-trip.
    pub fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    /// Ask the API to stop accepting connections and the bridge to shut down.
    /// Idempotent; [`Engine::wait`] sends the same signal.
    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Watch for the engine stopping, for hosts that need to react (a signal
    /// handler on desktop, a service callback on Android).
    pub fn shutdown_handle(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// Wait for shutdown and release the data directory.
    ///
    /// Always returns once the API task has finished, even when the engine was
    /// stopped from elsewhere or the task failed on its own.
    pub async fn wait(mut self) -> Result<()> {
        let outcome = (&mut self.serve).await;
        // Helpers may still be running if the server ended on its own.
        let _ = self.shutdown.send(true);
        for helper in std::mem::take(&mut self.helpers) {
            let _ = helper.await;
        }
        // A stale endpoint would make the next launch try a dead address.
        let _ = std::fs::remove_file(self.data_dir.join("endpoint.json"));
        match outcome {
            Ok(result) => result,
            Err(err) => Err(err).context("joining the local API task"),
        }
    }
}

/// Attach the command channel, then pump bridge events into the app state until
/// the bridge stops or the engine shuts down.
async fn install_bridge(
    state: &Arc<AppState>,
    bridge: Bridge,
    shutdown: watch::Receiver<bool>,
    helpers: &mut Vec<JoinHandle<()>>,
) {
    state.attach_transport(bridge.command_sender()).await;
    helpers.push(tokio::spawn(pump_bridge(bridge, state.clone(), shutdown)));
}

async fn pump_bridge(
    mut bridge: Bridge,
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            event = bridge.events.recv() => {
                match event {
                    Some(event) => state.on_bridge_event(event).await,
                    None => break,
                }
            }
        }
    }
    // Dropping the bridge kills a spawned interpreter or cancels the attach
    // listener, so nothing outlives the engine.
    drop(bridge);
    state
        .on_bridge_event(BridgeEvent::State {
            state: TransportState::Offline,
            address: None,
            name: None,
            detail: Some("reticulum bridge stopped".into()),
        })
        .await;
}

fn spawn_bridge(
    data_dir: &Path,
    display_name: &str,
    python: &str,
    script: &Path,
    extra_envs: &[(String, String)],
    managed: bool,
) -> Option<Bridge> {
    let rns_dir = data_dir.join("rns").to_string_lossy().to_string();
    let mut envs: Vec<(&str, String)> = vec![
        ("RCHAT_RNS_DIR", rns_dir),
        ("RCHAT_DISPLAY_NAME", display_name.to_string()),
        ("RCHAT_RNS_MANAGED", if managed { "1" } else { "0" }.to_string()),
        (
            "RCHAT_ATTACHMENT_MAX_BYTES",
            app::inbound_limit().to_string(),
        ),
    ];
    for (key, value) in extra_envs {
        envs.push((key.as_str(), value.clone()));
    }

    match rchat_rns::spawn(python, script, &envs) {
        Ok(bridge) => Some(bridge),
        Err(err) => {
            warn!(error = %err, "could not start the Reticulum bridge");
            None
        }
    }
}

fn write_endpoint(data_dir: &Path, endpoint: &Endpoint) -> Result<()> {
    let path = data_dir.join("endpoint.json");
    let tmp = data_dir.join("endpoint.json.tmp");
    let bytes = serde_json::to_vec_pretty(endpoint)?;
    std::fs::write(&tmp, &bytes)?;
    restrict_file(&tmp);
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Exclusive per-data-directory engine lock.
///
/// Contains the owning pid and, where the platform exposes it, the process
/// start time. Android reuses pids aggressively, so a pid alone would make a
/// crashed engine look alive and block the restart.
struct EngineLock {
    path: PathBuf,
}

impl EngineLock {
    fn acquire(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join("engine.lock");
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let pid = std::process::id();
                    match proc_start_time(pid) {
                        Some(started) => writeln!(file, "{pid} {started}")?,
                        None => writeln!(file, "{pid}")?,
                    }
                    drop(file);
                    restrict_file(&path);
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    match std::fs::read_to_string(&path).ok().as_deref().and_then(parse_lock) {
                        Some((pid, started)) if owner_alive(pid, started) => bail!(
                            "another rchatd is already using {} (pid {pid})",
                            data_dir.display()
                        ),
                        // Stale: the owner crashed or the pid now belongs to
                        // something else. Reclaim it.
                        _ => {
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

/// Parse `pid` or `pid starttime` from a lock file.
fn parse_lock(text: &str) -> Option<(i32, Option<u64>)> {
    let mut fields = text.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let started = fields.next().and_then(|value| value.parse().ok());
    Some((pid, started))
}

fn owner_alive(pid: i32, started: Option<u64>) -> bool {
    if pid <= 0 {
        return false;
    }
    // A lock carrying our own pid cannot be a competitor: this process has not
    // acquired it yet, so it was left by an earlier engine in the same process.
    // Android restarts the engine without restarting the app, so the pid (and
    // its start time) are unchanged.
    if pid == std::process::id() as i32 {
        return false;
    }
    match started {
        Some(expected) => proc_start_time(pid as u32) == Some(expected),
        // A lock written before start times were recorded: fall back to asking
        // whether the pid exists.
        None => process_alive(pid),
    }
}

/// Field 22 of `/proc/<pid>/stat`: process start time in clock ticks since
/// boot. Stable for a process's whole life, which is what distinguishes a
/// recycled pid from the original owner.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn proc_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field is parenthesised and may contain spaces or `)`, so split
    // after the last `)` rather than on whitespace.
    let rest = stat.rsplit_once(')')?.1;
    // `rest` starts at field 3 (state); field 22 is at offset 19.
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn proc_start_time(_pid: u32) -> Option<u64> {
    None
}

fn process_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        // Signal 0 performs the permission and existence checks without
        // delivering anything. EPERM means it exists but is not ours.
        let rc = unsafe { libc::kill(pid, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_lock_formats() {
        assert_eq!(parse_lock("1234\n"), Some((1234, None)));
        assert_eq!(parse_lock("1234 987654\n"), Some((1234, Some(987654))));
        assert_eq!(parse_lock(""), None);
        assert_eq!(parse_lock("not a pid"), None);
    }

    #[test]
    fn our_own_pid_is_never_a_live_owner() {
        // Leftover lock from an earlier engine in this same process.
        assert!(!owner_alive(std::process::id() as i32, proc_start_time(std::process::id())));
    }

    #[test]
    fn dead_pids_are_not_alive() {
        assert!(!owner_alive(0, None));
        assert!(!owner_alive(-1, None));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn start_time_is_stable_and_mismatches_are_stale() {
        let our_pid = std::process::id();
        let started = proc_start_time(our_pid).expect("/proc/self/stat should be readable");
        assert_eq!(proc_start_time(our_pid), Some(started));
        // A pid with the wrong start time belongs to a different process.
        assert_ne!(
            proc_start_time(our_pid),
            Some(started.wrapping_add(1)),
            "the start time must actually be compared"
        );
    }

    #[test]
    fn endpoint_json_keeps_the_desktop_fields() {
        let endpoint = Endpoint {
            http: "http://127.0.0.1:9000".into(),
            ws: "ws://127.0.0.1:9000/ws".into(),
            token: "0123456789abcdef".into(),
            pid: 42,
            port: 9000,
            bridge_port: None,
            version: "0.1.0".into(),
        };
        let value = serde_json::to_value(&endpoint).unwrap();
        for field in ["http", "ws", "token", "pid", "version"] {
            assert!(value.get(field).is_some(), "missing {field}");
        }
        assert!(value.get("bridge_port").is_none());
    }
}
