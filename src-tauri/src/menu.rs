//! App menu (G12): zoom, find-in-page, reload, print + standard Edit items (copy/paste need them on macOS).

use crate::core::Core;
use crate::webviews;
use tauri::menu::{AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Wry};

pub const ZOOM_MIN: f64 = 0.5;
pub const ZOOM_MAX: f64 = 3.0;
pub const ZOOM_STEP: f64 = 0.1;

pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let reload = MenuItem::with_id(app, "reload", "Tải lại trang", true, Some("CmdOrCtrl+R"))?;
    let zoom_in = MenuItem::with_id(app, "zoom_in", "Phóng to", true, Some("CmdOrCtrl+="))?;
    let zoom_out = MenuItem::with_id(app, "zoom_out", "Thu nhỏ", true, Some("CmdOrCtrl+-"))?;
    let zoom_reset = MenuItem::with_id(app, "zoom_reset", "Cỡ thật", true, Some("CmdOrCtrl+0"))?;
    let find = MenuItem::with_id(app, "find", "Tìm trong trang…", true, Some("CmdOrCtrl+F"))?;
    let find_next = MenuItem::with_id(app, "find_next", "Tìm tiếp", true, Some("CmdOrCtrl+G"))?;
    let print = MenuItem::with_id(app, "print", "In…", true, Some("CmdOrCtrl+P"))?;
    let refresh = MenuItem::with_id(app, "refresh_local", "Làm mới danh sách hub local", true, None::<&str>)?;
    let import = MenuItem::with_id(app, "import", "Thêm hub / Import URL…", true, Some("CmdOrCtrl+Shift+N"))?;
    let sync_all = MenuItem::with_id(app, "sync_all", "Đồng bộ lại các directory", true, None::<&str>)?;
    let manage = MenuItem::with_id(app, "manage", "Quản lý hub…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Cài đặt…", true, Some("CmdOrCtrl+,"))?;
    let switcher = MenuItem::with_id(app, "switcher", "Chuyển nhanh tới hub…", true, Some("CmdOrCtrl+K"))?;
    let prev = MenuItem::with_id(app, "hub_prev", "Hub trước", true, Some("CmdOrCtrl+Shift+["))?;
    let next = MenuItem::with_id(app, "hub_next", "Hub sau", true, Some("CmdOrCtrl+Shift+]"))?;
    let numbered: Vec<MenuItem<Wry>> = (1..=9)
        .map(|n| {
            MenuItem::with_id(app, format!("hub_{n}"), format!("Hub thứ {n}"), true, Some(format!("CmdOrCtrl+{n}")))
        })
        .collect::<tauri::Result<_>>()?;

    let about = PredefinedMenuItem::about(
        app,
        Some("Giới thiệu Cfharness Desktop"),
        Some(AboutMetadata {
            name: Some("Cfharness Desktop".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            ..Default::default()
        }),
    )?;

    let file = Submenu::with_items(
        app,
        "Tệp",
        true,
        &[
            &import,
            &refresh,
            &sync_all,
            &PredefinedMenuItem::separator(app)?,
            &print,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some("Đóng cửa sổ"))?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Sửa",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("Hoàn tác"))?,
            &PredefinedMenuItem::redo(app, Some("Làm lại"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("Cắt"))?,
            &PredefinedMenuItem::copy(app, Some("Sao chép"))?,
            &PredefinedMenuItem::paste(app, Some("Dán"))?,
            &PredefinedMenuItem::select_all(app, Some("Chọn tất cả"))?,
            &PredefinedMenuItem::separator(app)?,
            &find,
            &find_next,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "Xem",
        true,
        &[
            &reload,
            &PredefinedMenuItem::separator(app)?,
            &zoom_in,
            &zoom_out,
            &zoom_reset,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, Some("Toàn màn hình"))?,
        ],
    )?;
    let mut hub_items: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = vec![&switcher, &prev, &next];
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    hub_items.push(&sep1);
    for item in &numbered {
        hub_items.push(item);
    }
    hub_items.push(&sep2);
    hub_items.push(&manage);
    #[cfg(not(target_os = "macos"))]
    hub_items.push(&settings);
    let hub = Submenu::with_items(app, "Hub", true, &hub_items)?;
    let window = Submenu::with_items(
        app,
        "Cửa sổ",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("Thu nhỏ cửa sổ"))?,
            &PredefinedMenuItem::maximize(app, Some("Phóng to cửa sổ"))?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    {
        let app_menu = Submenu::with_items(
            app,
            "Cfharness Desktop",
            true,
            &[
                &about,
                &PredefinedMenuItem::separator(app)?,
                &settings,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, Some("Dịch vụ"))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, Some("Ẩn Cfharness Desktop"))?,
                &PredefinedMenuItem::hide_others(app, Some("Ẩn các ứng dụng khác"))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, Some("Thoát Cfharness Desktop"))?,
            ],
        )?;
        Menu::with_items(app, &[&app_menu, &file, &edit, &view, &hub, &window])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let help = Submenu::with_items(app, "Trợ giúp", true, &[&about])?;
        Menu::with_items(app, &[&file, &edit, &view, &hub, &window, &help])
    }
}

/// Next zoom level, clamped and rounded to one decimal.
pub fn next_zoom(cur: f64, action: &str) -> f64 {
    let z = match action {
        "zoom_in" => cur + ZOOM_STEP,
        "zoom_out" => cur - ZOOM_STEP,
        _ => 1.0,
    };
    (z.clamp(ZOOM_MIN, ZOOM_MAX) * 10.0).round() / 10.0
}

/// `hub_3` → 3 (only 1…9).
pub fn hub_number(id: &str) -> Option<usize> {
    let n: usize = id.strip_prefix("hub_")?.parse().ok()?;
    (1..=9).contains(&n).then_some(n)
}

pub fn handle(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref().to_string();
    let app = app.clone();
    // Menu events arrive on the main thread; run the work off it.
    tauri::async_runtime::spawn(async move { dispatch(&app, &id) });
}

fn dispatch(app: &AppHandle, id: &str) {
    match id {
        "reload" => {
            if let Some(wv) = webviews::active_webview(app) {
                let _ = wv.reload();
            }
        }
        "zoom_in" | "zoom_out" | "zoom_reset" => {
            let Some(hub) = webviews::active_hub_id(app) else { return };
            let z = {
                let core = app.state::<Core>();
                let mut g = core.lock();
                let cur = g.zoom.get(&hub).copied().unwrap_or(1.0);
                let z = next_zoom(cur, id);
                g.zoom.insert(hub.clone(), z);
                z
            };
            if let Some(wv) = webviews::active_webview(app) {
                let _ = wv.set_zoom(z);
            }
        }
        "print" => {
            if let Some(wv) = webviews::active_webview(app) {
                if let Err(e) = wv.print() {
                    log::warn!("in không được hỗ trợ: {e}");
                }
            }
        }
        "find" => crate::commands::set_find_open(app, true),
        "find_next" => {
            let _ = tauri::Emitter::emit_to(
                app,
                tauri::EventTarget::webview(webviews::SHELL_LABEL),
                "shell://find-next",
                (),
            );
        }
        "refresh_local" => crate::commands::refresh_local_inner(app),
        "hub_prev" | "hub_next" => {
            let a = app.clone();
            let off = if id == "hub_prev" { -1 } else { 1 };
            tauri::async_runtime::spawn(async move {
                let _ = webviews::select_relative(a, off).await;
            });
        }
        "switcher" | "import" | "manage" | "settings" => {
            // The shell opens the dialog (and asks to cover the hub webview via `set_overlay`).
            if let Some(shell) = app.get_webview(webviews::SHELL_LABEL) {
                let _ = shell.set_focus();
            }
            let _ =
                tauri::Emitter::emit_to(app, tauri::EventTarget::webview(webviews::SHELL_LABEL), "shell://open", id);
        }
        "sync_all" => {
            let a = app.clone();
            tauri::async_runtime::spawn(async move { crate::remote::sync_all(&a).await });
        }
        other if other.starts_with("tray:") => crate::tray::dispatch(app, other),
        other => {
            if let Some(n) = hub_number(other) {
                let a = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = webviews::select_index(a, n).await;
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_and_clamps() {
        assert_eq!(next_zoom(1.0, "zoom_in"), 1.1);
        assert_eq!(next_zoom(1.0, "zoom_out"), 0.9);
        assert_eq!(next_zoom(1.7, "zoom_reset"), 1.0);
        assert_eq!(next_zoom(3.0, "zoom_in"), 3.0);
        assert_eq!(next_zoom(0.5, "zoom_out"), 0.5);
    }

    #[test]
    fn hub_numbers() {
        assert_eq!(hub_number("hub_1"), Some(1));
        assert_eq!(hub_number("hub_9"), Some(9));
        assert_eq!(hub_number("hub_0"), None);
        assert_eq!(hub_number("hub_10"), None);
        assert_eq!(hub_number("hub_prev"), None);
    }
}
