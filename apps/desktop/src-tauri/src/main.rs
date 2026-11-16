//! rchat desktop shell.
//!
//! The shell is deliberately thin: it owns the window, tray, notifications and
//! engine lifecycle, while the Svelte UI talks to the same local API a browser
//! would. Keeping that boundary means the engine can be developed and tested
//! without Tauri, and a future web or mobile client reuses it unchanged.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod updates;

use engine::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, UserAttentionType, WebviewUrl,
    WebviewWindowBuilder, WindowEvent, Wry,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub muted: bool,
}

/// The network fields settings used to carry before interfaces moved into
/// `rchat.db`. Read only, so an upgrade can notice them; the engine imports
/// them into interface rows and the shell never writes them back.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LegacyNetworkSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_discovery: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_relay: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { muted: false }
    }
}

impl Settings {
    fn load(data_dir: &std::path::Path) -> Self {
        std::fs::read_to_string(data_dir.join("desktop.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn legacy_network(data_dir: &std::path::Path) -> Option<LegacyNetworkSettings> {
        let text = std::fs::read_to_string(data_dir.join("desktop.json")).ok()?;
        serde_json::from_str::<LegacyNetworkSettings>(&text)
            .ok()
            .filter(|legacy| legacy.auto_discovery.is_some() || legacy.tcp_relay.is_some())
    }

    fn save(&self, data_dir: &std::path::Path) {
        let path = data_dir.join("desktop.json");
        if let Ok(text) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(&path, text);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    muted: bool,
    autostart: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkResult {
    base: String,
    token: String,
}

pub struct AppState {
    pub engine: Engine,
    pub settings: Mutex<Settings>,
    data_dir: PathBuf,
    mute_item: Mutex<Option<CheckMenuItem<Wry>>>,
    autostart_item: Mutex<Option<CheckMenuItem<Wry>>>,
    last_chime: Mutex<Option<Instant>>,
}

fn main() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_window(app);
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_desktop_settings,
            set_muted,
            set_autostart,
            notify_activity,
            clear_activity,
            restart_engine,
            updates::check_for_update,
            updates::install_update,
            updates::update_state,
            updates::restart_for_update,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = data_dir();
            std::fs::create_dir_all(&data_dir)?;
            let settings = Settings::load(&data_dir);
            let legacy_network = Settings::legacy_network(&data_dir).map(|legacy| {
                eprintln!(
                    "rchat: importing the old network settings (auto discovery {:?}, relay {:?}) \
                     into interfaces; this happens once",
                    legacy.auto_discovery, legacy.tcp_relay
                );
                serde_json::to_string(&legacy).unwrap_or_default()
            });

            let engine = Engine::discover(&handle, data_dir.clone(), legacy_network)
                .map_err(|err| Box::<dyn std::error::Error>::from(err))?;
            let endpoint = engine.start();
            let (base, token, boot_error) = match &endpoint {
                Ok(endpoint) => (endpoint.base(), endpoint.token.clone(), None),
                Err(err) => (String::new(), String::new(), Some(err.clone())),
            };

            app.manage(AppState {
                engine,
                settings: Mutex::new(settings.clone()),
                data_dir: data_dir.clone(),
                mute_item: Mutex::new(None),
                autostart_item: Mutex::new(None),
                last_chime: Mutex::new(None),
            });

            let boot = json!({
                "desktop": true,
                "base": base,
                "token": token,
                "error": boot_error,
            });
            let script = format!(
                "window.__RCHAT_BOOT__ = {};",
                serde_json::to_string(&boot).unwrap_or_else(|_| "{}".into())
            );

            let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;
            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::App("index.html".into()),
            )
            .title("rchat")
            .inner_size(1180.0, 760.0)
            .min_inner_size(920.0, 560.0)
            .center()
            .visible(false)
            .background_color(Color(20, 22, 27, 255))
            .icon(icon)?
            .initialization_script(&script)
            .build()?;

            let window_handle = handle.clone();
            window.on_window_event(move |event| match event {
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    if let Some(window) = window_handle.get_webview_window("main") {
                        let _ = window.hide();
                    }
                    emit_window_state(&window_handle);
                }
                WindowEvent::Focused(true) => {
                    if let Some(window) = window_handle.get_webview_window("main") {
                        let _ = window.request_user_attention(None);
                    }
                    emit_window_state(&window_handle);
                }
                WindowEvent::Focused(false) => emit_window_state(&window_handle),
                _ => {}
            });

            build_tray(&handle, &settings)?;
            window.show()?;
            window.set_focus()?;
            Ok(())
        });

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(err) => {
            eprintln!("rchat could not start: {err}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| {
        if matches!(event, RunEvent::Exit) {
            if let Some(state) = app.try_state::<AppState>() {
                state.engine.stop();
            }
            set_badge(0);
        }
    });
}

fn build_tray(app: &AppHandle, settings: &Settings) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open rchat", true, None::<&str>)?;
    let mute = CheckMenuItem::with_id(
        app,
        "mute",
        "Mute sounds",
        true,
        settings.muted,
        None::<&str>,
    )?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start at login",
        true,
        autostart_enabled(),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit rchat", true, None::<&str>)?;
    let top = PredefinedMenuItem::separator(app)?;
    let bottom = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&open, &top, &mute, &autostart, &bottom, &quit],
    )?;

    {
        let state = app.state::<AppState>();
        *state.mute_item.lock().unwrap() = Some(mute.clone());
        *state.autostart_item.lock().unwrap() = Some(autostart.clone());
    }

    let tray_app = app.clone();
    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .tooltip("rchat")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "quit" => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.engine.stop();
                }
                set_badge(0);
                app.exit(0);
            }
            "mute" => {
                let state = app.state::<AppState>();
                let muted = {
                    let mut settings = state.settings.lock().unwrap();
                    settings.muted = !settings.muted;
                    settings.save(&state.data_dir);
                    settings.muted
                };
                if let Some(item) = state.mute_item.lock().unwrap().as_ref() {
                    let _ = item.set_checked(muted);
                }
                emit_settings(&state, app);
            }
            "autostart" => {
                let enabled = !autostart_enabled();
                set_autostart_enabled(enabled);
                let state = app.state::<AppState>();
                if let Some(item) = state.autostart_item.lock().unwrap().as_ref() {
                    let _ = item.set_checked(enabled);
                }
                emit_settings(&state, app);
            }
            _ => {}
        })
        .on_tray_icon_event(move |_tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(&tray_app);
            }
        })
        .build(app)?;
    Ok(())
}

fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);
    if visible && focused {
        let _ = window.hide();
        emit_window_state(app);
    } else {
        show_window(app);
    }
}

fn show_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    // Clear a pending needs-attention hint before asking for focus, otherwise
    // some X11 window managers keep the focus request pending.
    let _ = window.request_user_attention(None);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();

    // A focus request sent while the window is still being mapped is dropped
    // by Cinnamon/Muffin. Once it is viewable, the same request is honoured.
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        if let Some(window) = handle.get_webview_window("main") {
            let _ = window.set_focus();
            let _ = window.request_user_attention(None);
        }
    });
    emit_window_state(app);
}

fn emit_window_state(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = app.emit(
        "window-state",
        json!({
            "visible": window.is_visible().unwrap_or(false),
            "focused": window.is_focused().unwrap_or(false),
        }),
    );
}

fn emit_settings(state: &AppState, app: &AppHandle) {
    let view = settings_view(state);
    let _ = app.emit("settings-changed", view);
}

fn settings_view(state: &AppState) -> SettingsView {
    let settings = state.settings.lock().unwrap().clone();
    SettingsView {
        muted: settings.muted,
        autostart: autostart_enabled(),
    }
}

// ---- commands -------------------------------------------------------------

#[tauri::command]
fn get_desktop_settings(state: State<'_, AppState>) -> SettingsView {
    settings_view(&state)
}

#[tauri::command]
fn restart_engine(app: AppHandle, state: State<'_, AppState>) -> Result<NetworkResult, String> {
    let endpoint = state.engine.restart()?;
    set_badge(0);
    let _ = app.emit("engine-restarted", json!({ "base": endpoint.base() }));
    Ok(NetworkResult {
        base: endpoint.base(),
        token: endpoint.token,
    })
}

#[tauri::command]
fn set_muted(app: AppHandle, state: State<'_, AppState>, muted: bool) {
    {
        let mut settings = state.settings.lock().unwrap();
        settings.muted = muted;
        settings.save(&state.data_dir);
    }
    if let Some(item) = state.mute_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(muted);
    }
    emit_settings(&state, &app);
}

#[tauri::command]
fn set_autostart(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    set_autostart_enabled(enabled);
    if let Some(item) = state.autostart_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(enabled);
    }
    emit_settings(&state, &app);
}

#[tauri::command]
fn notify_activity(app: AppHandle, state: State<'_, AppState>, count: u32, chime: bool) {
    if count == 0 {
        clear_activity(app, state);
        return;
    }

    let window = app.get_webview_window("main");
    let alert = window
        .as_ref()
        .map(|window| {
            !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false)
        })
        .unwrap_or(true);

    if chime && !state.settings.lock().unwrap().muted {
        play_chime(&state, &app);
    }

    if alert {
        set_tray_unread(&app, true);
        set_badge(count);
        if let Some(window) = window {
            let _ = window.request_user_attention(Some(UserAttentionType::Critical));
        }
    } else {
        // A focused window is its own notification. Keep the tray quiet.
        set_tray_unread(&app, false);
        set_badge(0);
    }
}

#[tauri::command]
fn clear_activity(app: AppHandle, state: State<'_, AppState>) {
    set_tray_unread(&app, false);
    set_badge(0);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.request_user_attention(None);
    }
    let _ = state;
}

fn play_chime(state: &AppState, app: &AppHandle) {
    {
        let mut last = state.last_chime.lock().unwrap();
        if let Some(at) = *last {
            if at.elapsed() < Duration::from_millis(1400) {
                return;
            }
        }
        *last = Some(Instant::now());
    }

    let Some(path) = chime_path(app) else {
        return;
    };
    for player in ["paplay", "pw-play", "aplay"] {
        match Command::new(player)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return;
            }
            Err(_) => continue,
        }
    }
}

fn chime_path(app: &AppHandle) -> Option<PathBuf> {
    let resource = app
        .path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join("assets/chime.wav"))?;
    if resource.exists() {
        return Some(resource);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/chime.wav");
    manifest.exists().then_some(manifest)
}

fn set_tray_unread(app: &AppHandle, unread: bool) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let bytes: &[u8] = if unread {
        include_bytes!("../icons/tray-unread.png")
    } else {
        include_bytes!("../icons/tray.png")
    };
    if let Ok(image) = Image::from_bytes(bytes) {
        let _ = tray.set_icon(Some(image));
    }
    let _ = tray.set_tooltip(Some(if unread {
        "rchat — new messages"
    } else {
        "rchat"
    }));
}

pub(crate) fn set_badge(count: u32) {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    // Unity-style launcher counters work on Unity, GNOME (with a dash-to-dock
    // style extension), KDE and Pantheon. Cinnamon has no badge API, so it gets
    // the urgency hint from request_user_attention instead.
    if !["unity", "gnome", "kde", "pantheon"]
        .iter()
        .any(|name| desktop.contains(name))
    {
        return;
    }
    let payload = format!(
        "{{'application': <'rchat.desktop'>, 'count': <{count}>, 'count-visible': <{}>, 'urgent': <{}>}}",
        count > 0,
        count > 0
    );
    let _ = Command::new("gdbus")
        .args([
            "emit",
            "--session",
            "--object-path",
            "/network/rchat/desktop",
            "--signal",
            "com.canonical.Unity.LauncherEntry.Update",
            &payload,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        });
}

// ---- startup helpers ------------------------------------------------------

pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RCHAT_DATA_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("rchat");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/share/rchat")
}

fn autostart_path() -> PathBuf {
    let config = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".config")
        });
    config.join("autostart/rchat.desktop")
}

fn autostart_enabled() -> bool {
    autostart_path().exists()
}

fn set_autostart_enabled(enabled: bool) {
    let path = autostart_path();
    if !enabled {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let Some(parent) = path.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(parent);
    let exec = std::env::var("APPIMAGE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|path| path.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "rchat".into());
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=rchat\n\
         Comment=Messaging over Reticulum\n\
         Exec=\"{}\"\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec.replace('"', "\\\"")
    );
    let _ = std::fs::write(path, entry);
}
