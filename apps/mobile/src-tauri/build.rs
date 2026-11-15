fn main() {
    // Android 15+ devices may boot with 16 KB pages. The NDK linker still
    // defaults to 4 KB segment alignment, which Android 17 reports as an
    // incompatible app. Ask for 16 KB explicitly; it is harmless on 4 KB
    // devices and required on 16 KB ones.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-arg=-Wl,-z,max-page-size=16384");
    }
    tauri_build::build()
}
