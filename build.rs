fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=dist");
    println!("cargo:rerun-if-changed=dist/index.html");
    println!("cargo:rerun-if-changed=mobile.html");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=ITERATE_REQUIRE_ACTIVATION");

    let windows_msvc = std::env::var("TARGET").is_ok_and(|target| target.ends_with("windows-msvc"));
    if windows_msvc {
        // Library tests link GUI dependencies too, but do not inherit the app's
        // resource manifest. Select Common Controls v6 before Windows loads them.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
    }

    if cargo_target_is_macos() {
        compile_macos_speech_bridge();
    }

    if cargo_target_is_android() {
        link_android_cpp_runtime();
    }

    if windows_msvc {
        // The linker embeds this dependency for both applications and lib tests.
        // Avoid embedding a second manifest through Tauri's binary-only resource.
        let attributes = tauri_build::Attributes::new().windows_attributes(
            tauri_build::WindowsAttributes::new_without_app_manifest(),
        );
        tauri_build::try_build(attributes).expect("failed to build Tauri resources");
    } else {
        tauri_build::build()
    }
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
