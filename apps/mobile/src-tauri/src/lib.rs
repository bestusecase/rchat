//! rchat mobile shell.
//!
//! The phone is its own Reticulum node: the engine (`rchatd`) and the Python
//! bridge run inside this process, and the webview talks to the same loopback
//! HTTP/WS API the desktop UI uses. This crate owns only the app lifecycle, the
//! foreground service and the small set of commands the mobile UI needs.
//!
//! Startup order (see `RchatService`):
//!
//! 1. `nativeStart(dataDir)` below starts the engine in attach mode. It binds a
//!    loopback port and returns it before any Python exists.
//! 2. Kotlin starts the embedded bridge in Python, passing that port.
//! 3. Python dials in; the engine reports `ready` once Reticulum is up.
//!
//! The engine lives in a process-wide slot, not in Tauri state: it is owned by
//! the foreground service and must outlive every activity.

mod android;
mod notify;

use rchatd::{BridgeMode, Engine, Options};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A started engine plus the runtime that drives it.
struct Running {
    engine: Engine,
    runtime: tokio::runtime::Runtime,
    data_dir: PathBuf,
    display_name: String,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// Why the last start failed. Read by `boot` so the UI can say something useful
/// instead of spinning forever.
static BOOT_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Start the engine on `data_dir`. Called from Kotlin through JNI.
///
/// Idempotent: a second call while an engine is running returns the same bridge
/// port, which is what a service restart wants.
pub fn start_engine(data_dir: PathBuf) -> Result<u16, String> {
    if let Some(running) = RUNNING.lock().unwrap().as_ref() {
        if let Some(port) = running.engine.endpoint().bridge_port {
            return Ok(port);
        }
    }
    *BOOT_ERROR.lock().unwrap() = None;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("rchat-engine")
        .enable_all()
        .build()
        .map_err(|err| format!("creating the engine runtime: {err}"))?;

    let options = Options {
        data_dir: data_dir.clone(),
        port: 0,
        no_rns: false,
        bridge: BridgeMode::Attach,
        legacy_network: legacy_network(&data_dir),
    };
    let engine = runtime
        .block_on(Engine::start(options))
        .map_err(|err| format!("{err:#}"))?;
    let bridge_port = engine
        .endpoint()
        .bridge_port
        .ok_or_else(|| "the engine did not open a bridge port".to_string())?;
    android::log_info(&format!(
        "engine listening on {} bridge port {bridge_port}",
        engine.endpoint().http
    ));

    let running = Running {
        data_dir,
        display_name: engine.display_name().to_string(),
        engine,
        runtime,
    };
    notify::spawn(&running);
    *RUNNING.lock().unwrap() = Some(running);
    Ok(bridge_port)
}

/// Stop the engine and release its data directory. Called from Kotlin.
pub fn stop_engine() {
    let running = RUNNING.lock().unwrap().take();
    let Some(Running {
        engine, runtime, ..
    }) = running
    else {
        return;
    };
    engine.stop();
    if let Err(err) = runtime.block_on(engine.wait()) {
        android::log_error(&format!("engine shutdown: {err:#}"));
    }
    runtime.shutdown_timeout(Duration::from_secs(2));
    // Nothing a dead engine accumulated is worth keeping: a restarted process
    // must not inherit threads it cannot account for.
    notify::clear();
}

pub fn display_name() -> String {
    RUNNING
        .lock()
        .unwrap()
        .as_ref()
        .map(|running| running.display_name.clone())
        .unwrap_or_else(|| "rchat".to_string())
}

pub fn record_boot_error(message: &str) {
    *BOOT_ERROR.lock().unwrap() = Some(message.to_string());
    android::log_error(message);
}

fn data_dir() -> Option<PathBuf> {
    RUNNING
        .lock()
        .unwrap()
        .as_ref()
        .map(|running| running.data_dir.clone())
}

// ---- persisted settings ----------------------------------------------------

/// The network fields `desktop.json` carried before interfaces moved into the
/// engine. Read-only: the engine imports them once on upgrade.
#[derive(Debug, Default, Deserialize)]
struct LegacyNetwork {
    #[serde(default)]
    auto_discovery: Option<bool>,
    #[serde(default)]
    tcp_relay: Option<String>,
}

fn legacy_network(dir: &PathBuf) -> Option<rchat_core::store::LegacyNetwork> {
    let text = std::fs::read_to_string(dir.join("desktop.json")).ok()?;
    let legacy: LegacyNetwork = serde_json::from_str(&text).ok()?;
    if legacy.auto_discovery.is_none() && legacy.tcp_relay.is_none() {
        return None;
    }
    Some(rchat_core::store::LegacyNetwork {
        auto_discovery: legacy.auto_discovery,
        tcp_relay: legacy.tcp_relay,
    })
}

/// The subset of the desktop settings file that means something on a phone.
/// `desktop.json` is reused so the file format does not fork.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    muted: bool,
}

impl Settings {
    fn load(dir: Option<&PathBuf>) -> Self {
        dir.and_then(|dir| std::fs::read_to_string(dir.join("desktop.json")).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, dir: Option<&PathBuf>) {
        let Some(dir) = dir else { return };
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(dir.join("desktop.json"), bytes);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    auto_discovery: bool,
    tcp_relay: String,
    muted: bool,
    autostart: bool,
}

fn settings_view() -> SettingsView {
    let dir = data_dir();
    let settings = Settings::load(dir.as_ref());
    SettingsView {
        // The bridge always runs AutoInterface on the phone; the relay and
        // autostart concepts have no meaning here.
        auto_discovery: true,
        tcp_relay: String::new(),
        muted: settings.muted,
        autostart: false,
    }
}

// ---- commands -------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BootInfo {
    role: &'static str,
    base: String,
    token: String,
    error: Option<String>,
}

/// Hand the UI its connection details.
///
/// The service starts the engine asynchronously, so this waits briefly instead
/// of racing it. The webview calls this once, before its first connection.
#[tauri::command]
async fn boot() -> BootInfo {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let endpoint = {
            let running = RUNNING.lock().unwrap();
            running
                .as_ref()
                .map(|running| running.engine.endpoint().clone())
        };
        if let Some(endpoint) = endpoint {
            return BootInfo {
                role: "mobile",
                base: endpoint.http,
                token: endpoint.token,
                error: None,
            };
        }
        if let Some(error) = BOOT_ERROR.lock().unwrap().clone() {
            return BootInfo {
                role: "mobile",
                base: String::new(),
                token: String::new(),
                error: Some(error),
            };
        }
        if Instant::now() >= deadline {
            return BootInfo {
                role: "mobile",
                base: String::new(),
                token: String::new(),
                error: Some("the engine did not start in time".into()),
            };
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tauri::command]
fn get_desktop_settings() -> SettingsView {
    settings_view()
}

#[tauri::command]
fn set_muted(muted: bool) {
    let dir = data_dir();
    let mut settings = Settings::load(dir.as_ref());
    settings.muted = muted;
    settings.save(dir.as_ref());
}

/// Whether the user has silenced new-message notifications.
fn muted() -> bool {
    let dir = data_dir();
    Settings::load(dir.as_ref()).muted
}

/// Drop every message notification once the user is back in the app.
///
/// The accumulator is cleared here too, not just the shade: the user can now
/// see these messages, so nothing may bring them back on the next background
/// message.
#[tauri::command]
fn clear_activity() {
    notify::clear();
    let _ = android::clear_activity();
}

#[tauri::command]
fn open_background_settings() -> Result<(), String> {
    android::open_background_settings()
}

/// Save one attachment into the system media store so it shows up in Downloads.
///
/// The bytes are read here, from the engine's blob store, and cross into Kotlin
/// only for the write: the webview never holds a file larger than a thumbnail.
/// Returns false when the platform refused, with the reason in the error.
#[tauri::command]
fn save_attachment(hash: String, name: String) -> Result<bool, String> {
    let dir = data_dir().ok_or_else(|| "the engine is not running".to_string())?;
    let blobs = rchat_core::attachments::BlobStore::new(dir.join("attachments"));
    let bytes = blobs
        .read(&hash)
        .map_err(|err| format!("This attachment is no longer on this device: {err:#}"))?;
    android::save_attachment(&name, &bytes)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    android::init_logging();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            boot,
            get_desktop_settings,
            set_muted,
            clear_activity,
            open_background_settings,
            save_attachment
        ])
        .run(tauri::generate_context!())
        .expect("error while running rchat");
}
