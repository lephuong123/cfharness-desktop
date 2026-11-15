//! Cfharness Desktop — native shell around each hub's own web UI (docs/PLAN.md).

pub mod commands;
pub mod core;
pub mod directory;
pub mod fetch;
pub mod hubs;
pub mod local;
pub mod menu;
pub mod nav;
pub mod remote;
pub mod secrets;
pub mod webviews;

use crate::core::Core;
use tauri::window::WindowBuilder;
use tauri::{LogicalPosition, LogicalSize, Manager, WebviewBuilder, WebviewUrl, WindowEvent};

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,cfharness_desktop_lib=info"))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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

            let window = WindowBuilder::new(app, webviews::MAIN_WINDOW)
                .title("Cfharness Desktop")
                .inner_size(1280.0, 820.0)
                .min_inner_size(900.0, 600.0)
                .build()?;
            let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
            window.add_child(
                WebviewBuilder::new(webviews::SHELL_LABEL, WebviewUrl::App("index.html".into())),
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(size.width, size.height),
            )?;
            let h2 = handle.clone();
            window.on_window_event(move |e| {
                if let WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } = e {
                    let app = h2.clone();
                    tauri::async_runtime::spawn(async move { webviews::layout(&app) });
                }
            });

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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("lỗi khi chạy Cfharness Desktop");
}
