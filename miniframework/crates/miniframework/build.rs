//! `mf_dual_core` on chips with a second core (ESP32, ESP32-S3), so the
//! board runner can name `Core::Core1` only where it exists.
fn main() {
    println!("cargo:rustc-check-cfg=cfg(mf_dual_core)");
    println!("cargo:rerun-if-env-changed=TARGET");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("xtensa-esp32s3-") || target.starts_with("xtensa-esp32-") {
        println!("cargo:rustc-cfg=mf_dual_core");
    }
}
