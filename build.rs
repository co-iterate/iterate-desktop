fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=dist");
    println!("cargo:rerun-if-changed=dist/index.html");
    println!("cargo:rerun-if-changed=mobile.html");
    println!("cargo:rerun-if-env-changed=TARGET");

    if cargo_target_is_macos() {
        compile_macos_speech_bridge();
    }

    if cargo_target_is_android() {
        link_android_cpp_runtime();
    }

    tauri_build::build()
}

fn cargo_target_is_macos() -> bool {
    std::env::var("TARGET")
        .map(|target| target.contains("apple-darwin"))
        .unwrap_or(false)
}

fn cargo_target_is_android() -> bool {
    std::env::var("TARGET")
        .map(|target| target.contains("linux-android"))
        .unwrap_or(false)
}

fn link_android_cpp_runtime() {
    println!("cargo:rustc-link-lib=c++_shared");
}

fn compile_macos_speech_bridge() {
    println!("cargo:rerun-if-changed=src/rust/native_speech/macos_speech_bridge.m");
    println!("cargo:rerun-if-changed=src/rust/native_speech/macos_speech_abi.h");

    cc::Build::new()
        .file("src/rust/native_speech/macos_speech_bridge.m")
        .flag("-fobjc-arc")
        .compile("macos_speech_bridge");

    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=Speech");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=ApplicationServices");
    println!("cargo:rustc-link-lib=framework=AppKit");
}
