//! Update checks and installation.
//!
//! AppImage installs replace themselves through `tauri-plugin-updater`. Debian
//! installs belong to dpkg: the package ships its own apt source and a daily
//! systemd timer (`packaging/deb/`), so this module only reports what happened
//! and never tries to copy a binary under `/usr/bin`.

use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

const APT_SOURCE: &str = "/etc/apt/sources.list.d/rchat.list";
const DEB_MARKER: &str = "/var/lib/dpkg/info/rchat.list";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    running_version: String,
    install_kind: &'static str,
    package_version: Option<String>,
    pending_restart: bool,
    auto_updates: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    available: bool,
    current: String,
    latest: Option<String>,
    notes: Option<String>,
    date: Option<String>,
}

/// How this copy of rchat was installed, which decides who is allowed to
/// replace it. `APPIMAGE` is set by the AppImage runtime, so it is authoritative
/// at run time; the dpkg metadata file identifies a system install.
fn install_kind() -> &'static str {
    if std::env::var_os("APPIMAGE").is_some() {
        "appimage"
    } else if Path::new(DEB_MARKER).exists() {
        "deb"
    } else {
        "other"
    }
}

fn package_version() -> Option<String> {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Version}", "rchat"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

/// dpkg versions may carry an epoch and a Debian revision; the app reports the
/// plain upstream version, so compare only that part.
fn upstream(version: &str) -> &str {
    let without_epoch = version.rsplit(':').next().unwrap_or(version);
    without_epoch.split('-').next().unwrap_or(without_epoch)
}

fn collect_update_state() -> UpdateState {
    let running_version = env!("CARGO_PKG_VERSION").to_string();
    let install_kind = install_kind();
    let package_version = if install_kind == "deb" {
        package_version()
    } else {
        None
    };
    let pending_restart = package_version
        .as_deref()
        .map(|version| upstream(version) != running_version)
        .unwrap_or(false);
    UpdateState {
        auto_updates: install_kind == "deb" && Path::new(APT_SOURCE).exists(),
        running_version,
        install_kind,
        package_version,
        pending_restart,
    }
}

fn build_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let mut builder = app.updater_builder().timeout(Duration::from_secs(30));
    // Development hook: point the updater at a local manifest without editing
    // the shipped configuration. Signatures are still verified, so an override
    // cannot install anything the release key did not sign.
    if let Ok(endpoint) = std::env::var("RCHAT_UPDATE_ENDPOINT") {
        if !endpoint.trim().is_empty() {
            let url = tauri::Url::parse(endpoint.trim())
                .map_err(|err| format!("invalid endpoint: {err}"))?;
            builder = builder
                .endpoints(vec![url])
                .map_err(|err| err.to_string())?;
        }
    }
    builder.build().map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<UpdateCheck, String> {
    let update = build_updater(&app)?
        .check()
        .await
        .map_err(|err| err.to_string())?;
    match update {
        Some(update) => Ok(UpdateCheck {
            available: true,
            current: update.current_version.clone(),
            latest: Some(update.version.clone()),
            notes: update.body.clone(),
            date: update.date.map(|date| date.to_string()),
        }),
        None => Ok(UpdateCheck {
            available: false,
            current: env!("CARGO_PKG_VERSION").to_string(),
            latest: None,
            notes: None,
            date: None,
        }),
    }
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    if install_kind() != "appimage" {
        return Err("This copy of rchat is updated by your package manager.".into());
    }
    let update = build_updater(&app)?
        .check()
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "No update is available.".to_string())?;

    let downloaded = AtomicU64::new(0);
    let progress_app = app.clone();
    let finished_app = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                let done = downloaded.fetch_add(chunk as u64, Ordering::Relaxed) + chunk as u64;
                let _ = progress_app.emit(
                    "update-progress",
                    serde_json::json!({ "downloaded": done, "total": total }),
                );
            },
            move || {
                let _ = finished_app.emit("update-downloaded", ());
            },
        )
        .await
        .map_err(|err| err.to_string())?;

    let _ = app.emit("update-installed", ());
    Ok(())
}

#[tauri::command]
pub fn update_state() -> UpdateState {
    collect_update_state()
}

/// Stop the engine before replacing the process, then restart once the reply to
/// the UI has been sent.
#[tauri::command]
pub fn restart_for_update(app: AppHandle) -> Result<(), String> {
    if let Some(state) = app.try_state::<crate::AppState>() {
        state.engine.stop();
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        handle.restart();
    });
    Ok(())
}
