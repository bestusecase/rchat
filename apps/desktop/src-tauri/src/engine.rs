//! Supervises the bundled `rchatd` engine.
//!
//! The desktop shell owns one engine for its data directory. On launch it
//! reuses a healthy engine left by a previous run; otherwise it starts one on
//! a free loopback port and hands the UI the endpoint. Closing the shell does
//! not kill an engine that another client may be using, but Quit does.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

use crate::Settings;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub http: String,
    pub ws: String,
    pub token: String,
    pub pid: u32,
    #[serde(default)]
    pub version: String,
}

impl Endpoint {
    pub fn base(&self) -> String {
        self.http.trim_end_matches('/').to_string()
    }

    fn port(&self) -> Option<u16> {
        let rest = self.http.rsplit(':').next()?;
        rest.trim_end_matches('/').parse().ok()
    }
}

pub struct Engine {
    pub data_dir: PathBuf,
    rchatd: PathBuf,
    python: PathBuf,
    site_packages: PathBuf,
    sidecar: PathBuf,
    token: Mutex<String>,
    endpoint: Mutex<Option<Endpoint>>,
    child: Mutex<Option<Child>>,
}

impl Engine {
    pub fn discover(app: &AppHandle, data_dir: PathBuf) -> Result<Self, String> {
        let rchatd = find_asset(app, "rchatd")
            .or_else(|| dev_path("target/release/rchatd"))
            .or_else(|| dev_path("target/debug/rchatd"))
            .ok_or_else(|| "the rchat engine binary is missing from the bundle".to_string())?;

        let runtime = find_asset(app, "runtime")
            .or_else(|| dev_path("runtime"))
            .ok_or_else(|| {
                "the bundled Python runtime is missing; reinstall rchat".to_string()
            })?;
        let python = runtime.join("py/bin/python3");
        let site_packages = runtime.join("site-packages");
        if !python.exists() {
            return Err(format!(
                "the bundled Python runtime is incomplete ({})",
                python.display()
            ));
        }
        let sidecar = find_asset(app, "sidecar/bridge.py")
            .or_else(|| workspace_path("sidecar/bridge.py"))
            .ok_or_else(|| "the Reticulum bridge script is missing".to_string())?;

        Ok(Self {
            data_dir,
            rchatd,
            python,
            site_packages,
            sidecar,
            token: Mutex::new(String::new()),
            endpoint: Mutex::new(None),
            child: Mutex::new(None),
        })
    }

    /// Reuse a healthy engine if one is already serving this data directory.
    pub fn start(&self, settings: &Settings) -> Result<Endpoint, String> {
        if let Some(endpoint) = self.reusable_endpoint() {
            *self.token.lock().unwrap() = endpoint.token.clone();
            *self.endpoint.lock().unwrap() = Some(endpoint.clone());
            *self.child.lock().unwrap() = None;
            return Ok(endpoint);
        }

        let token = {
            let mut token = self.token.lock().unwrap();
            if token.is_empty() {
                *token = ensure_token(&self.data_dir)?;
            }
            token.clone()
        };
        self.spawn(settings, &token, None)
    }

    /// Stop the engine and start it again on the same port, so the UI can
    /// simply reconnect to the address it already knows.
    pub fn restart(&self, settings: &Settings) -> Result<Endpoint, String> {
        let previous_port = self
            .endpoint
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Endpoint::port);
        let token = self.token.lock().unwrap().clone();
        self.stop();

        match self.spawn(settings, &token, previous_port) {
            Ok(endpoint) => Ok(endpoint),
            Err(first) => {
                // The old port may have been taken in the split second between
                // shutdown and bind. A fresh port always works.
                self.spawn(settings, &token, None)
                    .map_err(|second| format!("{first}; retry also failed: {second}"))
            }
        }
    }

    fn spawn(
        &self,
        settings: &Settings,
        token: &str,
        preferred_port: Option<u16>,
    ) -> Result<Endpoint, String> {
        let endpoint_path = self.data_dir.join("endpoint.json");
        let _ = fs::remove_file(&endpoint_path);

        fs::create_dir_all(&self.data_dir).map_err(|err| err.to_string())?;
        let log_path = self.data_dir.join("engine.log");
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|err| format!("opening {}: {err}", log_path.display()))?;
        let log_err = log.try_clone().map_err(|err| err.to_string())?;

        let port_arg = preferred_port.unwrap_or(0).to_string();
        // AppImage runtimes export PYTHONHOME/PYTHONPATH for the system Python,
        // which would point our private interpreter at a stdlib that is not
        // there. Pin both to the bundled runtime.
        let python_home = self.python.parent().and_then(|dir| dir.parent());
        let mut command = Command::new(&self.rchatd);
        command
            .arg("--data-dir")
            .arg(&self.data_dir)
            .arg("--port")
            .arg(&port_arg)
            .arg("--supervised")
            .env("RCHAT_TOKEN", token)
            .env("RCHAT_PYTHON", &self.python)
            .env("RCHAT_SIDECAR", &self.sidecar)
            .env("PYTHONPATH", &self.site_packages)
            .env("RCHAT_RNS_AUTO", if settings.auto_discovery { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));

        if let Some(home) = python_home {
            command.env("PYTHONHOME", home);
        }

        let relay = settings.tcp_relay.trim();
        if !relay.is_empty() {
            command.env("RCHAT_RNS_TCP_CLIENT", relay);
        }

        let child = command.spawn().map_err(|err| {
            format!(
                "could not start {}: {err}",
                self.rchatd.display()
            )
        })?;
        let pid = child.id();
        *self.child.lock().unwrap() = Some(child);

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(endpoint) = read_endpoint(&endpoint_path) {
                if endpoint.pid == pid && health_ok(&endpoint) {
                    *self.endpoint.lock().unwrap() = Some(endpoint.clone());
                    return Ok(endpoint);
                }
            }
            if let Some(status) = self
                .child
                .lock()
                .unwrap()
                .as_mut()
                .and_then(|child| child.try_wait().ok())
                .flatten()
            {
                let tail = log_tail(&log_path);
                return Err(format!(
                    "the rchat engine exited during startup ({status}).{}",
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!(" Last output: {tail}")
                    }
                ));
            }
            if Instant::now() > deadline {
                let tail = log_tail(&log_path);
                return Err(format!(
                    "the rchat engine did not start in time.{}",
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!(" Last output: {tail}")
                    }
                ));
            }
            std::thread::sleep(Duration::from_millis(80));
        }
    }

    fn reusable_endpoint(&self) -> Option<Endpoint> {
        let endpoint = read_endpoint(&self.data_dir.join("endpoint.json"))?;
        if health_ok(&endpoint) {
            Some(endpoint)
        } else {
            let _ = fs::remove_file(self.data_dir.join("endpoint.json"));
            None
        }
    }

    /// Terminate the engine cleanly with SIGTERM, then wait for it to exit.
    pub fn stop(&self) {
        let mut child = self.child.lock().unwrap().take();
        let pid = child
            .as_ref()
            .map(|child| child.id())
            .or_else(|| {
                self.endpoint
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|endpoint| endpoint.pid)
            });

        if let Some(pid) = pid {
            signal(pid, libc::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(5);
            while process_alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(60));
            }
            if process_alive(pid) {
                signal(pid, libc::SIGKILL);
            }
        }
        if let Some(child) = child.as_mut() {
            let _ = child.wait();
        }
        self.endpoint.lock().unwrap().take();
        let _ = fs::remove_file(self.data_dir.join("endpoint.json"));
    }

    pub fn base(&self) -> Option<String> {
        self.endpoint
            .lock()
            .unwrap()
            .as_ref()
            .map(Endpoint::base)
    }

    pub fn token(&self) -> String {
        self.token.lock().unwrap().clone()
    }
}

fn ensure_token(data_dir: &Path) -> Result<String, String> {
    let path = data_dir.join("token");
    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if trimmed.len() >= 16 {
            return Ok(trimmed.to_string());
        }
    }
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|err| format!("generating a token: {err}"))?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    fs::create_dir_all(data_dir).map_err(|err| err.to_string())?;
    fs::write(&path, &token).map_err(|err| format!("writing {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

fn read_endpoint(path: &Path) -> Option<Endpoint> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn health_ok(endpoint: &Endpoint) -> bool {
    let Some(port) = endpoint.port() else {
        return false;
    };
    let Ok(addr) = format!("127.0.0.1:{port}").parse() else {
        return false;
    };
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(350)) {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let request = format!(
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.contains("\"ok\":true")
}

fn log_tail(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().rev().take(4).collect();
    lines.reverse();
    lines.join(" | ")
}

fn signal(pid: u32, signal: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, signal);
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
    }
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        PathBuf::from(format!("/proc/{pid}")).exists()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn find_asset(app: &AppHandle, relative: &str) -> Option<PathBuf> {
    let dir = app.path().resource_dir().ok()?;
    let candidates = [
        dir.join(relative),
        dir.join("_up_").join(relative),
    ];
    candidates.into_iter().find(|path| path.exists())
}

fn dev_path(relative: &str) -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest.join(relative);
    path.exists().then_some(path)
}

fn workspace_path(relative: &str) -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest.join("../..").join(relative);
    path.exists().then_some(path)
}
