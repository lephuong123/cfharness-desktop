//! Cfharness Desktop — native shell around each hub's own web UI (docs/PLAN.md).

pub mod bridge;
pub mod commands;
pub mod core;
pub mod deeplink;
pub mod directory;
pub mod fetch;
pub mod hubs;
pub mod installer;
pub mod local;
pub mod menu;
pub mod nav;
pub mod notify;
pub mod remote;
pub mod secrets;
pub mod speech;
pub mod tray;
pub mod updater;
pub mod webviews;

use crate::core::Core;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::window::WindowBuilder;
use tauri::{LogicalPosition, LogicalSize, Manager, RunEvent, WebviewBuilder, WebviewUrl, WindowEvent};
use tauri_plugin_deep_link::DeepLinkExt;

/// Launched by the OS login item (`tauri-plugin-autostart` passes this flag): start in the tray, window hidden.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Set when the user really quits (tray/menu): the window close button only hides the window otherwise.
static QUITTING: AtomicBool = AtomicBool::new(false);

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,cfharness_desktop_lib=info"))
        .init();

    tauri::Builder::default()
        // Must be first: a second launch (deep link on Windows, double click) focuses the running app instead.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            webviews::show_main(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::select_hub,
            commands::retry_hub,
            commands::refresh_local,
            commands::create_default_hub,
            commands::find_in_page,
            commands::find_close,
            commands::inspect_url,
            commands::confirm_import,
            commands::sync_directory,
            commands::remove_directory,
            commands::resolve_origin_change,
            commands::rename_hub,
            commands::set_hub_color,
            commands::set_hub_hidden,
            commands::move_hub,
            commands::remove_hub,
            commands::set_overlay,
            commands::select_relative,
            commands::install_plan,
            commands::install_run,
            commands::set_settings,
            commands::update_status,
            commands::update_check,
            commands::update_install,
            commands::set_update_prefs,
            bridge::badge_update,
            bridge::speech_available,
            bridge::speech_start,
            bridge::speech_stop,
            bridge::clipboard_write,
            bridge::download_save,
        ])
        .setup(|app| {
            // `CFHARNESS_DESKTOP_DATA_DIR` (dev/test only) isolates a run from the real app data dir.
            let data_dir = match std::env::var_os("CFHARNESS_DESKTOP_DATA_DIR") {
                Some(d) => std::path::PathBuf::from(d),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700));
            }
            log::info!("thư mục dữ liệu app: {}", data_dir.display());
            speech::set_data_dir(&data_dir);
            let core = Core::new(data_dir);
            {
                let mut g = core.lock();
                g.bin = local::find_binary();
                match &g.bin {
                    Some(b) => log::info!("cfharness: {}", b.display()),
                    None => log::warn!("không tìm thấy cfharness (PATH, ~/.local/bin, thư mục uv tool)"),
                }
                let homes = local::discover_system();
                log::info!("hub local tìm thấy: {}", homes.len());
                if g.reg.merge_local(&homes) {
                    core.persist(&g);
                }
            }
            app.manage(core);

            let handle = app.handle().clone();
            let menu = menu::build(&handle)?;
            app.set_menu(menu)?;
            app.on_menu_event(menu::handle);

            let hidden = std::env::args().any(|a| a == AUTOSTART_ARG);
            let window = WindowBuilder::new(app, webviews::MAIN_WINDOW)
                .title("Cfharness Desktop")
                .inner_size(1280.0, 820.0)
                .min_inner_size(900.0, 600.0)
                .visible(!hidden)
                .build()?;
            let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
            window.add_child(
                WebviewBuilder::new(webviews::SHELL_LABEL, WebviewUrl::App("index.html".into())),
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(size.width, size.height),
            )?;
            let h2 = handle.clone();
            let w2 = window.clone();
            window.on_window_event(move |e| match e {
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    let app = h2.clone();
                    tauri::async_runtime::spawn(async move { webviews::layout(&app) });
                }
                // Closing the window keeps the app in the tray (notifications keep working); quit = tray/menu.
                WindowEvent::CloseRequested { api, .. } if !QUITTING.load(Ordering::SeqCst) => {
                    api.prevent_close();
                    let _ = w2.hide();
                    crate::speech::stop_unless_active(None);
                }
                _ => {}
            });

            if let Err(e) = tray::init(&handle) {
                log::warn!("không tạo được biểu tượng tray: {e}");
            }

            // Deep links: cfharness://hub/<id>/<path> (validated in deeplink.rs).
            #[cfg(windows)]
            {
                // NSIS registers the scheme; this also covers a portable/dev run (HKCU only, no admin).
                if let Err(e) = app.deep_link().register_all() {
                    log::warn!("không đăng ký được scheme cfharness://: {e}");
                }
            }
            let h4 = handle.clone();
            app.deep_link().on_open_url(move |ev| {
                let urls: Vec<String> = ev.urls().iter().map(|u| u.to_string()).collect();
                deeplink::handle_urls(&h4, urls);
            });
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                deeplink::handle_urls(&handle, urls.iter().map(|u| u.to_string()).collect());
            }

            // Open the last active hub (local hubs need cfharness; cloud hubs do not) at its last URL.
            let active = {
                let core = handle.state::<Core>();
                let g = core.lock();
                g.reg.active.clone().filter(|a| g.bin.is_some() || !g.is_local(a))
            };
            if let Some(id) = active {
                let h3 = handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = webviews::select(h3, id.clone()).await {
                        log::warn!("không mở được hub {id}: {e}");
                    }
                });
            }
            remote::start_background(handle.clone());
            updater::start_background(handle.clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("lỗi khi chạy Cfharness Desktop")
        .run(|app, ev| match ev {
            RunEvent::ExitRequested { .. } => {
                QUITTING.store(true, Ordering::SeqCst);
                crate::speech::stop_unless_active(None);
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => webviews::show_main(app),
            _ => {
                let _ = app;
            }
        });
}
