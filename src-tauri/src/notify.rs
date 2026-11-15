//! Badges + OS notifications (PLAN §3.4, §5.4).
//!
//! The hub page reports its C3 counts through `badge_update` (bridge.rs). Here we keep the last counts per hub (rail
//! badge, tray), and when approvals/questions INCREASE while the hub is not in the foreground we show an OS
//! notification with FIXED text plus the hub name the user sees in the app — never text coming from the hub/agent.
//! Clicking the notification switches to the hub and opens `/approvals` (or `/inbox` for questions).

use crate::bridge::Badge;
use crate::core::Core;
use crate::directory::clean_line;
use crate::webviews;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Approvals,
    Questions,
}

impl Kind {
    /// Hub page opened when the notification is clicked.
    pub fn path(self) -> &'static str {
        match self {
            Kind::Approvals => "/approvals",
            Kind::Questions => "/inbox",
        }
    }
}

/// New items since the previous report. The first report of a hub only sets the baseline.
pub fn increases(prev: Option<&Badge>, next: &Badge) -> Vec<(Kind, u32)> {
    let Some(p) = prev else { return vec![] };
    let mut v = vec![];
    if next.approvals > p.approvals {
        v.push((Kind::Approvals, next.approvals - p.approvals));
    }
    if next.questions > p.questions {
        v.push((Kind::Questions, next.questions - p.questions));
    }
    v
}

/// Fixed notification text; `hub_name` is the app-side name (cleaned to one line).
pub fn message(hub_name: &str, kind: Kind, n: u32) -> String {
    let name = clean_line(hub_name, 60);
    let name = if name.is_empty() { "?".to_string() } else { name };
    match kind {
        Kind::Approvals => format!("Hub {name}: {n} yêu cầu duyệt mới"),
        Kind::Questions => format!("Hub {name}: {n} câu hỏi mới"),
    }
}

/// Rail badge number: what needs the human (approvals + questions).
pub fn rail_count(b: &Badge) -> u32 {
    b.approvals.saturating_add(b.questions)
}

/// Called by `badge_update` (already validated).
pub fn on_badge(app: &AppHandle, id: &str, b: Badge) {
    log::debug!("hub {id}: badge duyệt={} hỏi={} chạy={} estop={}", b.approvals, b.questions, b.running, b.estop);
    let (prev, name, enabled, active) = {
        let core = app.state::<Core>();
        let mut g = core.lock();
        let prev = g.badges.insert(id.to_string(), b.clone());
        let name = g.reg.get(id).map(|h| h.name.clone()).unwrap_or_default();
        (prev, name, g.reg.settings.notify, g.reg.active.clone())
    };
    if prev.as_ref() != Some(&b) {
        webviews::emit_state(app);
    }
    let ups = increases(prev.as_ref(), &b);
    if ups.is_empty() || !enabled {
        return;
    }
    let foreground = active.as_deref() == Some(id)
        && app
            .get_window(webviews::MAIN_WINDOW)
            .map(|w| {
                w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) && !w.is_minimized().unwrap_or(false)
            })
            .unwrap_or(false);
    if foreground {
        return;
    }
    for (kind, n) in ups {
        show(app, id, &message(&name, kind, n), kind);
    }
}

fn on_click(app: &AppHandle, id: &str, kind: Kind) {
    let (app, id) = (app.clone(), id.to_string());
    tauri::async_runtime::spawn(async move {
        if let Err(e) = webviews::open_path(app, id.clone(), kind.path().to_string()).await {
            log::warn!("thông báo: không mở được hub {id}: {e}");
        }
    });
}

#[cfg(target_os = "macos")]
fn show(app: &AppHandle, id: &str, text: &str, kind: Kind) {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Inside the .app bundle notifications belong to the app; an unbundled dev binary falls back to the default.
        let bundled =
            std::env::current_exe().map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/")).unwrap_or(false);
        if bundled {
            let _ = mac_notification_sys::set_application("io.cfharness.desktop");
        }
    });
    let (app, id, text) = (app.clone(), id.to_string(), text.to_string());
    // `wait_for_click` blocks until the notification is clicked or dismissed: own thread.
    std::thread::spawn(move || {
        let res = mac_notification_sys::Notification::new()
            .title("Cfharness Desktop")
            .message(&text)
            .wait_for_click(true)
            .send();
        match res {
            Ok(mac_notification_sys::NotificationResponse::Click) => on_click(&app, &id, kind),
            Ok(_) => {}
            Err(e) => log::warn!("không hiện được thông báo: {e}"),
        }
    });
}

#[cfg(windows)]
fn show(app: &AppHandle, id: &str, text: &str, kind: Kind) {
    use tauri_winrt_notification::Toast;
    // Installed (NSIS registers the AUMID = identifier); a dev build uses the PowerShell AUMID like the Tauri plugin.
    let installed = std::env::current_exe().map(|p| !p.to_string_lossy().contains("\\target\\")).unwrap_or(false);
    let app_id = if installed { "io.cfharness.desktop" } else { Toast::POWERSHELL_APP_ID };
    let (a, i) = (app.clone(), id.to_string());
    let res = Toast::new(app_id)
        .title("Cfharness Desktop")
        .text1(text)
        .on_activated(move |_| {
            on_click(&a, &i, kind);
            Ok(())
        })
        .show();
    if let Err(e) = res {
        log::warn!("không hiện được thông báo: {e}");
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn show(_app: &AppHandle, id: &str, text: &str, _kind: Kind) {
    log::info!("thông báo (hub {id}): {text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(a: u32, q: u32) -> Badge {
        Badge { approvals: a, questions: q, running: 0, estop: false, version: "1".into() }
    }

    #[test]
    fn only_increases_notify() {
        assert!(increases(None, &b(3, 2)).is_empty(), "first report = baseline");
        assert_eq!(increases(Some(&b(1, 0)), &b(3, 0)), vec![(Kind::Approvals, 2)]);
        assert_eq!(increases(Some(&b(1, 0)), &b(1, 1)), vec![(Kind::Questions, 1)]);
        assert!(increases(Some(&b(3, 3)), &b(2, 3)).is_empty());
        assert_eq!(increases(Some(&b(0, 0)), &b(1, 4)), vec![(Kind::Approvals, 1), (Kind::Questions, 4)]);
    }

    #[test]
    fn fixed_text() {
        assert_eq!(message("Local", Kind::Approvals, 2), "Hub Local: 2 yêu cầu duyệt mới");
        assert_eq!(message("An\n\u{202e}x", Kind::Questions, 1), "Hub An x: 1 câu hỏi mới");
        assert_eq!(message("", Kind::Questions, 1), "Hub ?: 1 câu hỏi mới");
        assert_eq!(Kind::Approvals.path(), "/approvals");
        assert_eq!(Kind::Questions.path(), "/inbox");
        assert_eq!(rail_count(&b(2, 3)), 5);
    }
}
