//! Loads Media Foundation (MP4 export) only when first used, so the tool
//! still starts on Windows N editions, which lack it until the Media
//! Feature Pack is installed; MP4 export then says so
//! (`guiderecorder_core::export::motion::mp4`).

fn main() {
    let target = |key: &str| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "windows" && target("CARGO_CFG_TARGET_ENV") == "msvc" {
        for dll in ["mfplat.dll", "mfreadwrite.dll"] {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
