//! JNI boundary between the Kotlin shells and the Rust engine.
//!
//! Android owns the process: Kotlin starts the foreground service, the service
//! calls [`Java_network_rchat_mobile_EngineBridge_nativeStart`], and the
//! webview later invokes the Tauri commands in `lib.rs`. The pieces that must
//! cross the boundary are small and deliberately one-directional per concern:
//!
//! * Kotlin → Rust: engine start/stop, display name.
//! * Rust → Kotlin: logging, message notifications, the visibility check, the
//!   battery settings page.
//!
//! App classes cannot be looked up with `FindClass` from an attached native
//! thread (it only sees the boot class path), so `JNI_OnLoad` caches a global
//! reference to `EngineBridge` while the app loader is still current.

#[cfg(target_os = "android")]
use jni::objects::JValueOwned;
use jni::objects::{GlobalRef, JClass, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::OnceLock;

static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();
static ENGINE_BRIDGE: OnceLock<GlobalRef> = OnceLock::new();

#[no_mangle]
pub extern "system" fn JNI_OnLoad(
    vm: *mut jni::sys::JavaVM,
    _reserved: *mut c_void,
) -> jni::sys::jint {
    let Ok(vm) = (unsafe { JavaVM::from_raw(vm) }) else {
        return jni::sys::JNI_VERSION_1_6;
    };
    if let Ok(mut env) = vm.get_env() {
        // Called on the thread that loaded the library, so the app class
        // loader is in effect and this lookup works.
        if let Ok(class) = env.find_class("network/rchat/mobile/EngineBridge") {
            if let Ok(class) = env.new_global_ref(class) {
                let _ = ENGINE_BRIDGE.set(class);
            }
        }
    }
    let _ = JAVA_VM.set(vm);
    jni::sys::JNI_VERSION_1_6
}

#[no_mangle]
pub extern "system" fn Java_network_rchat_mobile_EngineBridge_nativeStart<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    data_dir: JString<'local>,
    config_dir: JString<'local>,
) -> jni::sys::jint {
    let dir: String = match env.get_string(&data_dir) {
        Ok(dir) => dir.to_str().unwrap_or_default().to_string(),
        Err(err) => {
            crate::record_boot_error(&format!("invalid engine data directory: {err}"));
            return -1;
        }
    };
    let config: String = match env.get_string(&config_dir) {
        Ok(dir) => dir.to_str().unwrap_or_default().to_string(),
        Err(err) => {
            crate::record_boot_error(&format!("invalid bridge config directory: {err}"));
            return -1;
        }
    };
    match crate::start_engine(PathBuf::from(dir), PathBuf::from(config)) {
        Ok(port) => port as jni::sys::jint,
        Err(err) => {
            crate::record_boot_error(&err);
            -1
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_network_rchat_mobile_EngineBridge_nativeStop<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    crate::stop_engine();
}

#[no_mangle]
pub extern "system" fn Java_network_rchat_mobile_EngineBridge_nativeDisplayName<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jni::sys::jstring {
    match env.new_string(crate::display_name()) {
        Ok(name) => name.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// The user can see the app again, so nothing accumulated is worth a
/// notification. Cancelling what is already in the shade happens in Kotlin.
#[no_mangle]
pub extern "system" fn Java_network_rchat_mobile_EngineBridge_nativeForgetThreads<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    crate::notify::clear();
}

// ---- Rust -> Kotlin -------------------------------------------------------

/// Post the conversation threads, replacing whatever is in the shade.
///
/// One call per event: the payload is the whole snapshot, so Kotlin renders it
/// without holding state of its own beyond the ids it has already posted.
pub fn notify_threads(payload: &str) -> Result<(), String> {
    with_engine_bridge(|env, class| {
        let payload = env.new_string(payload).map_err(|err| err.to_string())?;
        match env.call_static_method(
            class,
            "notifyThreads",
            "(Ljava/lang/String;)V",
            &[JValue::Object(payload.as_ref())],
        ) {
            Ok(_) => Ok(()),
            Err(err) => {
                let _ = env.exception_clear();
                Err(err.to_string())
            }
        }
    })
}

/// Whether the user can actually see the app right now.
///
/// Kotlin owns the definition — one activity resumed, display interactive,
/// keyguard unlocked — and both call sites ask it here rather than re-deriving
/// it, so the notification policy cannot drift.
///
/// Fails closed: off Android nothing is posted, so nothing should accumulate,
/// and on Android a bridge that cannot answer is treated as visible. Message
/// text is never posted on a guess; the next event asks again.
pub fn user_can_see_app() -> bool {
    #[cfg(not(target_os = "android"))]
    {
        true
    }
    #[cfg(target_os = "android")]
    {
        let asked = with_engine_bridge(|env, class| {
            match env.call_static_method(class, "userCanSeeApp", "()Z", &[]) {
                Ok(jni::objects::JValueGen::Bool(visible)) => Ok(visible != 0),
                Ok(_) => Err("userCanSeeApp did not return a boolean".to_string()),
                Err(err) => {
                    let _ = env.exception_clear();
                    Err(err.to_string())
                }
            }
        });
        match asked {
            Ok(visible) => visible,
            Err(err) => {
                log_error(&format!(
                    "could not ask whether the user can see the app: {err}"
                ));
                true
            }
        }
    }
}

pub fn clear_activity() -> Result<(), String> {
    call_engine_bridge("clearActivity", "()V", &[])
}

pub fn open_background_settings() -> Result<(), String> {
    call_engine_bridge("openBackgroundSettings", "()V", &[])
}

/// Write one attachment into the system media store.
///
/// Kotlin answers with an empty string on success and the platform's reason on
/// failure, so a denied permission or a full disk can be shown as it is. Off
/// Android there is no media store, and this is a phone-only command.
pub fn save_attachment(name: &str, bytes: &[u8]) -> Result<bool, String> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (name, bytes);
        Err("Saving to the media store is only available on Android.".to_string())
    }
    #[cfg(target_os = "android")]
    {
        let reason = with_engine_bridge(|env, class| {
            let name = env.new_string(name).map_err(|err| err.to_string())?;
            let payload = env
                .byte_array_from_slice(bytes)
                .map_err(|err| err.to_string())?;
            let answer = env.call_static_method(
                class,
                "saveAttachment",
                "(Ljava/lang/String;[B)Ljava/lang/String;",
                &[
                    JValue::Object(name.as_ref()),
                    JValue::Object(payload.as_ref()),
                ],
            );
            match answer {
                Ok(JValueOwned::Object(value)) => {
                    let text: String = env
                        .get_string(&JString::from(value))
                        .map(|text| text.into())
                        .unwrap_or_default();
                    Ok(text)
                }
                Ok(_) => Err("saveAttachment did not return a string".to_string()),
                Err(err) => {
                    let _ = env.exception_clear();
                    Err(err.to_string())
                }
            }
        })?;
        if reason.is_empty() {
            Ok(true)
        } else {
            Err(reason)
        }
    }
}

fn with_engine_bridge<T>(
    call: impl FnOnce(&mut JNIEnv, &GlobalRef) -> Result<T, String>,
) -> Result<T, String> {
    let vm = JAVA_VM.get().ok_or("the JVM is not available")?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|err| format!("attaching to the JVM: {err}"))?;
    let class = ENGINE_BRIDGE
        .get()
        .ok_or("EngineBridge has not been loaded")?;
    call(&mut env, class)
}

fn call_engine_bridge(method: &str, signature: &str, args: &[JValue]) -> Result<(), String> {
    with_engine_bridge(
        |env, class| match env.call_static_method(class, method, signature, args) {
            Ok(_) => Ok(()),
            Err(err) => {
                // Leave no pending exception behind on this thread.
                let _ = env.exception_clear();
                Err(err.to_string())
            }
        },
    )
}

// ---- logging --------------------------------------------------------------

pub fn log_info(message: &str) {
    log('i', message);
}

pub fn log_error(message: &str) {
    log('e', message);
}

#[cfg(target_os = "android")]
fn log(level: char, message: &str) {
    // Write through liblog directly rather than android.util.Log: the JNI path
    // would re-enter this function (jni logs thread attachments at debug level),
    // which recurses through the tracing subscriber.
    use std::ffi::CString;

    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(prio: i32, tag: *const u8, text: *const u8) -> i32;
    }

    const ANDROID_LOG_DEBUG: i32 = 3;
    const ANDROID_LOG_INFO: i32 = 4;
    const ANDROID_LOG_WARN: i32 = 5;
    const ANDROID_LOG_ERROR: i32 = 6;

    let priority = match level {
        'e' => ANDROID_LOG_ERROR,
        'w' => ANDROID_LOG_WARN,
        'd' => ANDROID_LOG_DEBUG,
        _ => ANDROID_LOG_INFO,
    };
    let Ok(text) = CString::new(message.replace('\0', " ")) else {
        return;
    };
    static TAG: &[u8] = b"rchat\0";
    unsafe {
        __android_log_write(priority, TAG.as_ptr(), text.as_ptr());
    }
}

#[cfg(not(target_os = "android"))]
fn log(_level: char, _message: &str) {
    // The desktop `cargo run` path exists only for quick UI checks.
}

/// Route the engine's `tracing` output (and ours) to logcat on Android.
pub fn init_logging() {
    #[cfg(target_os = "android")]
    {
        use std::io::Write;
        use tracing_subscriber::fmt::MakeWriter;

        struct Logcat;
        impl Write for Logcat {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let text = String::from_utf8_lossy(buf);
                let text = text.trim_end();
                if !text.is_empty() {
                    log_info(text);
                }
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Logcat {
            type Writer = Logcat;
            fn make_writer(&'a self) -> Self::Writer {
                Logcat
            }
        }

        let _ = tracing_subscriber::fmt()
            .with_writer(Logcat)
            .with_ansi(false)
            .with_target(false)
            .without_time()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();
        // The jni crate logs every thread attachment at debug level. Those lines
        // are noise and would otherwise be forwarded by the tracing/log bridge.
        log::set_max_level(log::LevelFilter::Info);
    }
}
