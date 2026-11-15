fn main() {
    // Declaring the app commands makes them permission-gated: only capabilities that list `allow-<cmd>` can call them.
    // Shell commands → `capabilities/shell.json` (the `shell` webview only). The six bridge commands at the end are
    // granted at runtime to each hub webview for its registered origin only (`src/bridge.rs`, PLAN §5.2).
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "get_state",
        "select_hub",
        "retry_hub",
        "refresh_local",
        "create_default_hub",
        "find_in_page",
        "find_close",
        "inspect_url",
        "confirm_import",
        "sync_directory",
        "remove_directory",
        "resolve_origin_change",
        "rename_hub",
        "set_hub_color",
        "set_hub_hidden",
        "move_hub",
        "remove_hub",
        "set_overlay",
        "select_relative",
        "install_plan",
        "install_run",
        "set_settings",
        "update_status",
        "update_check",
        "update_install",
        "set_update_prefs",
        // hub bridge (never in shell.json)
        "badge_update",
        "speech_start",
        "speech_stop",
        "speech_available",
        "clipboard_write",
        "download_save",
    ])))
    .expect("failed to run tauri-build");

    // macOS speech recognition (C4): small Objective-C bridge over SFSpeechRecognizer + AVAudioEngine.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=native/speech_mac.m");
        cc::Build::new()
            .file("native/speech_mac.m")
            .flag("-fobjc-arc")
            .flag("-fmodules")
            .flag("-Wno-unguarded-availability-new")
            .compile("cfh_speech");
        for fw in ["Foundation", "AVFoundation", "Speech"] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
    }
}
