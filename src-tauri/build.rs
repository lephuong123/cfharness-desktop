fn main() {
    // Declaring the app commands makes them permission-gated: only capabilities that list `allow-<cmd>`
    // (the `shell` webview) can call them. Hub webviews have no capability at all.
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
    ])))
    .expect("failed to run tauri-build");
}
