//! Tray / menu bar icon (PLAN P3): list of hubs (click → show window + switch), show/hide window, quit.
//! ESTOP is never a tray action (it is a page in the hub). Menu ids are prefixed `tray:` and handled in
//! `menu::dispatch` (Tauri delivers every menu event, tray included, to the global handler).

use crate::core::Core;
use crate::notify;
use crate::webviews;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

pub const TRAY_ID: &str = "main";
const MAX_HUBS: usize = 30;

/// Tray label for one hub: app-side name (one line, ≤ 40 chars) + count of items waiting for the human.
pub fn hub_label(name: &str, count: u32) -> String {
    let n = crate::directory::clean_line(name, 40);
    let n = if n.is_empty() { "?".to_string() } else { n };
    if count > 0 {
        format!("{n} ({})", if count > 99 { "99+".to_string() } else { count.to_string() })
    } else {
        n
    }
}

/// What the menu shows; rebuilt only when this changes.
fn signature(app: &AppHandle) -> Vec<(String, String)> {
    let core = app.state::<Core>();
    let g = core.lock();
    g.reg
        .rail_order(false)
        .iter()
        .take(MAX_HUBS)
        .filter_map(|id| g.reg.get(id))
        .map(|h| {
            let count = g.badges.get(&h.id).map(notify::rail_count).unwrap_or(0);
            let mark = if g.reg.active.as_deref() == Some(h.id.as_str()) { "● " } else { "" };
            (h.id.clone(), format!("{mark}{}", hub_label(&h.name, count)))
        })
        .collect()
}

static LAST: Mutex<Option<Vec<(String, String)>>> = Mutex::new(None);

fn build_menu(app: &AppHandle, sig: &[(String, String)]) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;
    if sig.is_empty() {
        menu.append(&MenuItem::with_id(app, "tray:none", "Chưa có hub", false, None::<&str>)?)?;
    }
    for (id, label) in sig {
        menu.append(&MenuItem::with_id(app, format!("tray:hub:{id}"), label, true, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "tray:show", "Hiện cửa sổ", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "tray:hide", "Ẩn cửa sổ", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "tray:quit", "Thoát Cfharness Desktop", true, None::<&str>)?)?;
    Ok(menu)
}

pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let sig = signature(app);
    let menu = build_menu(app, &sig)?;
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = Some(sig);
    let mut b =
        TrayIconBuilder::with_id(TRAY_ID).tooltip("Cfharness Desktop").menu(&menu).show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon().cloned() {
        b = b.icon(icon);
    }
    b.build(app)?;
    Ok(())
}

/// Rebuild the menu when the hub list / names / badges changed. Call off the main-thread callbacks.
pub fn refresh(app: &AppHandle) {
    let sig = signature(app);
    {
        let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
        if last.as_ref() == Some(&sig) {
            return;
        }
        *last = Some(sig.clone());
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    match build_menu(app, &sig) {
        Ok(m) => {
            let _ = tray.set_menu(Some(m));
        }
        Err(e) => log::warn!("không dựng được menu tray: {e}"),
    }
}

pub fn dispatch(app: &AppHandle, id: &str) {
    match id {
        "tray:show" => webviews::show_main(app),
        "tray:hide" => {
            if let Some(w) = app.get_window(webviews::MAIN_WINDOW) {
                let _ = w.hide();
            }
        }
        "tray:quit" => app.exit(0),
        other => {
            if let Some(hub) = other.strip_prefix("tray:hub:") {
                let (a, h) = (app.clone(), hub.to_string());
                webviews::show_main(app);
                tauri::async_runtime::spawn(async move {
                    let _ = webviews::select(a, h).await;
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(hub_label("Local", 0), "Local");
        assert_eq!(hub_label("Local", 3), "Local (3)");
        assert_eq!(hub_label("Local", 120), "Local (99+)");
        assert_eq!(hub_label("a\nb\u{202e}", 0), "a b");
        assert_eq!(hub_label("", 0), "?");
    }
}
