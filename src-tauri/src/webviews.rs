//! One child webview per hub inside the main window, right of the rail (PLAN §3.3, §5).
//!
//! - separate data store per hub (`data_store_identifier` on macOS 14+, `data_directory` on Windows);
//! - navigation lock to the hub origin, other origins → system browser, new windows blocked;
//! - downloads → native save dialog;
//! - web permission requests (microphone, camera, geolocation, notifications, clipboard read, …) are DENIED
//!   (`permission_decision`): without a handler WKWebView grants getUserMedia silently. Voice goes through the
//!   native bridge (`speech_start`, active hub only);
//! - Tauri's drag-drop handler is disabled so HTML5 drag-and-drop (file attachments in the chat) works on Windows;
//! - LRU: at most `settings.live_webviews` alive, the rest sleep (webview closed, data store kept);
//! - `last_url` remembered per hub (path + query only);
//! - User-Agent suffix ` CfharnessDesktop/<ver>` (C7);
//! - local hubs log in through the one-time link from C1 and re-login when they land on `/login`.
//!
//! Hub webviews get NO shell command (see `capabilities/shell.json`, which targets the `shell` webview only). Each hub
//! webview gets the bridge init script (`inject/hub.js`) and a runtime capability for the six bridge commands,
//! limited to that hub's origin (`bridge::grant`, P3).

use crate::core::{Core, HubStatus};
use crate::hubs::{self, HubKind};
use crate::local;
use crate::nav::{self, NavDecision, Origin};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tauri::webview::{
    DownloadEvent, NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, WebviewBuilder,
};
use tauri::{AppHandle, Emitter, EventTarget, LogicalPosition, LogicalSize, Manager, Rect, WebviewUrl};
use tauri_plugin_opener::OpenerExt;
use url::Url;

pub const MAIN_WINDOW: &str = "main";
pub const SHELL_LABEL: &str = "shell";
/// Width of the hub rail (logical px) — must match `--rail-w` in shell-ui.
pub const RAIL_W: f64 = 72.0;
/// Height of the find bar drawn by the shell above the hub webview — must match `--find-h`.
pub const FIND_H: f64 = 44.0;

const C1_TIMEOUT: Duration = Duration::from_secs(60);

pub fn user_agent_suffix() -> String {
    format!(" CfharnessDesktop/{}", env!("CARGO_PKG_VERSION"))
}

/// macOS: the WKWebView default UA is frozen to this string; we append the app token (C7).
#[cfg(target_os = "macos")]
fn macos_user_agent() -> String {
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko){}",
        user_agent_suffix()
    )
}

pub fn label_of(hub_id: &str) -> String {
    format!("hub-{hub_id}")
}

fn hub_id_of_label(label: &str) -> Option<&str> {
    label.strip_prefix("hub-")
}

/// Push the full shell state to the shell UI.
pub fn emit_state(app: &AppHandle) {
    let core = app.state::<Core>();
    let state = core.lock().shell_state();
    let _ = app.emit_to(EventTarget::webview(SHELL_LABEL), "shell://state", state);
    let a = app.clone();
    tauri::async_runtime::spawn(async move { crate::tray::refresh(&a) });
}

/// Show, unminimize and focus the main window (tray, notification click, deep link, second instance).
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_window(MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Switch to hub `id` and open `path` (relative, already validated) in it.
pub async fn open_path(app: AppHandle, id: String, path: String) -> Result<(), String> {
    if !path.starts_with('/') || path.starts_with("//") {
        return Err("Đường dẫn không hợp lệ".into());
    }
    show_main(&app);
    let live = app.get_webview(&label_of(&id)).is_some();
    if !live {
        let core = app.state::<Core>();
        core.lock().pending_path.insert(id.clone(), path.clone());
    }
    select(app.clone(), id.clone()).await?;
    if live {
        if let (Some(o), Some(wv)) = (current_origin(&app, &id), app.get_webview(&label_of(&id))) {
            if let Some(u) = o.join(&path) {
                let _ = wv.navigate(u);
            }
        }
    }
    Ok(())
}

fn set_status(app: &AppHandle, id: &str, st: HubStatus) {
    {
        let core = app.state::<Core>();
        let mut g = core.lock();
        g.status.insert(id.to_string(), st);
    }
    emit_state(app);
}

/// Logical size of the main window's content area.
fn content_size(app: &AppHandle) -> Option<(f64, f64)> {
    let w = app.get_window(MAIN_WINDOW)?;
    let scale = w.scale_factor().ok()?;
    let s = w.inner_size().ok()?.to_logical::<f64>(scale);
    Some((s.width, s.height))
}

/// Position the shell (full window) and the active hub webview (right of the rail); hide the others.
pub fn layout(app: &AppHandle) {
    let Some((w, h)) = content_size(app) else { return };
    let (active, find_open, live) = {
        let core = app.state::<Core>();
        let g = core.lock();
        // A shell dialog (import, switcher, hub settings…) covers the content area: hide the hub meanwhile.
        let active_ok = g
            .reg
            .active
            .clone()
            .filter(|_| !g.overlay)
            .filter(|a| matches!(g.status_of(a), HubStatus::Online | HubStatus::Login | HubStatus::Starting));
        (active_ok, g.find_open, g.lru.clone())
    };
    {
        // The microphone belongs to the hub on screen only.
        let core = app.state::<Core>();
        let act = core.lock().reg.active.clone();
        crate::speech::stop_unless_active(act.as_deref());
    }
    if let Some(shell) = app.get_webview(SHELL_LABEL) {
        let _ = shell
            .set_bounds(Rect { position: LogicalPosition::new(0.0, 0.0).into(), size: LogicalSize::new(w, h).into() });
    }
    let top = if find_open { FIND_H } else { 0.0 };
    for id in live {
        let Some(wv) = app.get_webview(&label_of(&id)) else { continue };
        if Some(&id) == active.as_ref() {
            let _ = wv.set_bounds(Rect {
                position: LogicalPosition::new(RAIL_W, top).into(),
                size: LogicalSize::new((w - RAIL_W).max(1.0), (h - top).max(1.0)).into(),
            });
            let _ = wv.show();
        } else {
            let _ = wv.hide();
        }
    }
}

/// Select a hub: show its live webview or start it.
pub async fn select(app: AppHandle, id: String) -> Result<(), String> {
    let (live, limit) = {
        let core = app.state::<Core>();
        let mut g = core.lock();
        if g.reg.get(&id).is_none() {
            return Err("Không tìm thấy hub".into());
        }
        g.reg.active = Some(id.clone());
        core.persist(&g);
        let live = app.get_webview(&label_of(&id)).is_some();
        (live, g.reg.settings.live_limit())
    };
    if live {
        let evict = {
            let core = app.state::<Core>();
            let mut g = core.lock();
            g.touch_lru(&id, limit)
        };
        sleep_hubs(&app, evict);
        layout(&app);
        if let Some(wv) = app.get_webview(&label_of(&id)) {
            let _ = wv.set_focus();
        }
        emit_state(&app);
        return Ok(());
    }
    emit_state(&app);
    open(app, id).await
}

/// Start a hub that has no webview yet.
pub async fn open(app: AppHandle, id: String) -> Result<(), String> {
    let (kind, home, origin_s, last_url, bin) = {
        let core = app.state::<Core>();
        let mut g = core.lock();
        let Some(h) = g.reg.get(&id).cloned() else { return Err("Không tìm thấy hub".into()) };
        if !g.starting.insert(id.clone()) {
            return Ok(()); // already starting
        }
        g.status.insert(id.clone(), HubStatus::Starting);
        let start = g.pending_path.remove(&id).or_else(|| h.last_url.clone());
        (h.kind, h.home.clone(), h.origin.clone(), start, g.bin.clone())
    };
    emit_state(&app);
    layout(&app);
    let result = match kind {
        HubKind::Local => open_local(&app, &id, home, last_url, bin).await,
        HubKind::Remote => open_remote(&app, &id, origin_s, last_url).await,
    };
    {
        let core = app.state::<Core>();
        core.lock().starting.remove(&id);
    }
    if let Err(msg) = &result {
        set_status(&app, &id, HubStatus::Error { message: msg.clone() });
        layout(&app);
    }
    result
}

async fn c1(bin: PathBuf, home: PathBuf) -> Result<local::WebInfo, String> {
    tauri::async_runtime::spawn_blocking(move || local::ensure_web(&bin, &home, C1_TIMEOUT))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            log::warn!("cfharness web --json --ensure lỗi: {e:?}");
            e.user_message()
        })
}

async fn open_local(
    app: &AppHandle,
    id: &str,
    home: Option<String>,
    last_url: Option<String>,
    bin: Option<PathBuf>,
) -> Result<(), String> {
    let bin = bin.ok_or_else(|| "Chưa cài cfharness trên máy này".to_string())?;
    let home = home.ok_or_else(|| "Hub local thiếu đường dẫn HUB_HOME".to_string())?;
    let info = c1(bin, PathBuf::from(&home)).await?;
    log::info!("hub {id}: web {} (phiên bản {}, started={})", info.origin.as_string(), info.version, info.started);
    {
        let core = app.state::<Core>();
        let mut g = core.lock();
        g.origins.insert(id.to_string(), info.origin.clone());
        if let Some(lu) = last_url.filter(|u| u != "/") {
            g.pending_after_login.insert(id.to_string(), lu);
        }
        if let Some(h) = g.reg.get_mut(id) {
            if h.hub_id.as_deref() != Some(info.hub_id.as_str()) {
                h.hub_id = Some(info.hub_id.clone());
            }
        }
        core.persist(&g);
    }
    create(app, id, info.login_url)
}

async fn open_remote(
    app: &AppHandle,
    id: &str,
    origin_s: Option<String>,
    last_url: Option<String>,
) -> Result<(), String> {
    // Same rules as the import (https, no IP/localhost/path) — hubs.json is user-editable.
    let origin = crate::directory::validate_origin(origin_s.as_deref().unwrap_or_default())
        .map_err(|e| format!("Origin của hub không hợp lệ: {}", e.message()))?;
    // Probe first: a TLS/network problem is shown in the shell (the webview would show a bare error page).
    // The hub's own /login (TOTP) runs inside its webview — the app stores no credential for cloud hubs.
    let reach = {
        let core = app.state::<Core>();
        let r = core.fetcher.probe(&origin).await;
        core.lock().reach.insert(id.to_string(), r);
        r
    };
    match reach {
        crate::fetch::Reach::Tls => return Err(crate::fetch::FetchError::Tls.message()),
        crate::fetch::Reach::Offline => return Err(crate::fetch::FetchError::Offline.message()),
        // Bad = answers but without C2 (older hub): still usable.
        crate::fetch::Reach::Online | crate::fetch::Reach::Bad => {}
    }
    let start = origin.join(last_url.as_deref().unwrap_or("/")).unwrap_or_else(|| origin.join("/").expect("root path"));
    {
        let core = app.state::<Core>();
        core.lock().origins.insert(id.to_string(), origin);
    }
    create(app, id, start)
}

/// Re-login a local hub whose session expired: mint a fresh one-time link and load it.
pub fn relogin(app: &AppHandle, id: &str) {
    let app = app.clone();
    let id = id.to_string();
    tauri::async_runtime::spawn(async move {
        let (home, bin, last) = {
            let core = app.state::<Core>();
            let mut g = core.lock();
            let Some(h) = g.reg.get(&id).cloned() else { return };
            if !g.starting.insert(id.clone()) {
                return;
            }
            g.status.insert(id.clone(), HubStatus::Starting);
            (h.home.clone(), g.bin.clone(), h.last_url.clone())
        };
        emit_state(&app);
        let res = match (bin, home) {
            (Some(b), Some(h)) => c1(b, PathBuf::from(h)).await,
            _ => Err("Chưa cài cfharness trên máy này".to_string()),
        };
        {
            let core = app.state::<Core>();
            core.lock().starting.remove(&id);
        }
        match res {
            Ok(info) => {
                let moved = {
                    let core = app.state::<Core>();
                    let mut g = core.lock();
                    let old = g.origins.insert(id.clone(), info.origin.clone());
                    if let Some(lu) = last.filter(|u| u != "/") {
                        g.pending_after_login.entry(id.clone()).or_insert(lu);
                    }
                    old.as_ref() != Some(&info.origin)
                };
                if moved {
                    // New port: the bridge script and capability are bound to the old origin → new webview.
                    log::info!("hub {id}: web đổi địa chỉ, tạo lại WebView");
                    if let Err(e) = create(&app, &id, info.login_url) {
                        set_status(&app, &id, HubStatus::Error { message: e });
                    }
                } else if let Some(wv) = app.get_webview(&label_of(&id)) {
                    log::info!("hub {id}: đăng nhập lại tự động");
                    let _ = wv.navigate(info.login_url);
                }
            }
            Err(msg) => {
                set_status(&app, &id, HubStatus::Error { message: msg });
                layout(&app);
            }
        }
    });
}

fn current_origin(app: &AppHandle, id: &str) -> Option<Origin> {
    let core = app.state::<Core>();
    let g = core.lock();
    g.origins.get(id).cloned()
}

/// Navigation-lock callback (runs on the main thread: keep it short, no webview calls under the lock).
fn on_navigate(app: &AppHandle, id: &str, url: &Url) -> bool {
    let Some(origin) = current_origin(app, id) else { return url.as_str() == "about:blank" };
    let decision = nav::decide(&origin, url);
    log::debug!("hub {id}: điều hướng {} → {decision:?}", redact_url(url));
    match decision {
        NavDecision::Allow => {
            if nav::is_login_page(&origin, url) {
                let core = app.state::<Core>();
                let mut g = core.lock();
                if g.is_local(id) && g.take_relogin_slot(id, Instant::now()) {
                    drop(g);
                    relogin(app, id);
                    return false;
                }
            }
            true
        }
        NavDecision::External => {
            log::info!("hub {id}: mở ngoài trình duyệt hệ thống: {}", url.origin().ascii_serialization());
            let _ = app.opener().open_url(url.as_str(), None::<&str>);
            false
        }
        NavDecision::Block => {
            log::warn!("hub {id}: chặn điều hướng scheme {}", url.scheme());
            false
        }
    }
}

/// URL for logs: never the query (one-time login token) or the fragment (`#k=` session key).
pub fn redact_url(u: &Url) -> String {
    match u.scheme() {
        "http" | "https" => format!("{}{}", u.origin().ascii_serialization(), u.path()),
        s => format!("{s}:…"),
    }
}

fn on_loaded(app: &AppHandle, id: &str, url: &Url) {
    log::debug!("hub {id}: tải xong {}", redact_url(url));
    let Some(origin) = current_origin(app, id) else { return };
    if !origin.matches(url) {
        return;
    }
    let mut follow: Option<Url> = None;
    let status;
    {
        let core = app.state::<Core>();
        let mut g = core.lock();
        if nav::is_login_page(&origin, url) {
            status = HubStatus::Login;
        } else if url.path() == "/login" {
            // The one-time link is being consumed (redirect follows); keep "starting".
            status = HubStatus::Starting;
        } else {
            status = HubStatus::Online;
            if let Some(p) = g.pending_after_login.remove(id) {
                if hubs::last_url_of(url).as_deref() != Some(p.as_str()) {
                    follow = origin.join(&p);
                }
            }
            if follow.is_none() {
                if let Some(lu) = hubs::last_url_of(url) {
                    if let Some(h) = g.reg.get_mut(id) {
                        if h.last_url.as_deref() != Some(lu.as_str()) {
                            h.last_url = Some(lu);
                            core.persist(&g);
                        }
                    }
                }
            }
        }
        g.status.insert(id.to_string(), status.clone());
    }
    // Runs on the main thread inside a webview callback: defer webview/window calls.
    let id = id.to_string();
    defer(app, move |app| {
        emit_state(app);
        layout(app);
        if let Some(u) = follow {
            if let Some(wv) = app.get_webview(&label_of(&id)) {
                let _ = wv.navigate(u);
            }
        }
    });
}

/// Run `f` off the current (main-thread callback) stack.
fn defer<F: FnOnce(&AppHandle) + Send + 'static>(app: &AppHandle, f: F) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { f(&app) });
}

fn suggested_name(dest: &std::path::Path, url: &Url) -> String {
    if let Some(n) = dest.file_name().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.is_empty()) {
        return n;
    }
    url.path_segments()
        .and_then(|mut s| s.next_back().map(str::to_string))
        .map(|s| percent_decode(&s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "tai-ve".to_string())
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes())
        .next()
        .map(|(_, v)| v.replace(['/', '\\'], "_"))
        .unwrap_or_else(|| s.to_string())
}

fn on_download(app: &AppHandle, id: &str, event: DownloadEvent<'_>) -> bool {
    match event {
        DownloadEvent::Requested { url, destination } => {
            let origin = current_origin(app, id);
            let ok_src = match url.scheme() {
                "blob" | "data" => true,
                "http" | "https" => origin.as_ref().map(|o| o.matches(&url)).unwrap_or(false),
                _ => false,
            };
            if !ok_src {
                log::warn!("hub {id}: từ chối tải từ nguồn ngoài hub");
                return false;
            }
            let name = suggested_name(destination, &url);
            let mut dlg = rfd::FileDialog::new().set_title("Lưu tệp").set_file_name(&name);
            if let Some(d) = dirs::download_dir() {
                dlg = dlg.set_directory(d);
            }
            match dlg.save_file() {
                Some(path) => {
                    *destination = path;
                    true
                }
                None => false,
            }
        }
        DownloadEvent::Finished { success, .. } => {
            log::info!("hub {id}: tải tệp xong (success={success})");
            true
        }
        _ => true,
    }
}

/// Decision for a web permission request from a hub page (PLAN §5.2 / G2). Hub pages are untrusted; the only
/// sensitive capability they need (voice) goes through the native bridge with its active-hub check, so every
/// privacy-relevant request is denied. Only harmless UI behaviours keep the platform default.
pub fn permission_decision(kind: PermissionKind) -> PermissionResponse {
    match kind {
        PermissionKind::Autoplay | PermissionKind::PointerLock => PermissionResponse::Default,
        _ => PermissionResponse::Deny,
    }
}

/// Create the hub webview (hidden until laid out) and load `start`.
fn create(app: &AppHandle, id: &str, start: Url) -> Result<(), String> {
    let window = app.get_window(MAIN_WINDOW).ok_or("Không thấy cửa sổ chính")?;
    let label = label_of(id);
    if let Some(old) = app.get_webview(&label) {
        let _ = old.close();
    }
    let origin = current_origin(app, id).ok_or("Hub chưa có địa chỉ")?;
    let poll = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.settings.badge_poll_s
    };
    // Bridge (P3): capability for exactly this origin, granted before the first page can call it.
    crate::bridge::grant(app, &label, &origin);
    let (a1, a2, a3, a4) = (app.clone(), app.clone(), app.clone(), app.clone());
    let (i1, i2, i3, i4) = (id.to_string(), id.to_string(), id.to_string(), id.to_string());
    // Windows sets the UA through WebView2 settings before the first real navigation (see below).
    let initial = if cfg!(windows) { Url::parse("about:blank").expect("about:blank") } else { start.clone() };
    let mut builder = WebviewBuilder::new(&label, WebviewUrl::External(initial))
        .initialization_script(crate::bridge::init_script(&origin, poll))
        .on_navigation(move |u| on_navigate(&a1, &i1, u))
        .on_page_load(move |_wv, p| {
            if p.event() == PageLoadEvent::Finished {
                on_loaded(&a2, &i2, p.url());
            }
        })
        .on_new_window(move |u, _features| {
            // Never open a second window: same origin → this webview, else the system browser.
            if let Some(o) = current_origin(&a3, &i3) {
                match nav::decide(&o, &u) {
                    NavDecision::Allow => {
                        let id = i3.clone();
                        defer(&a3, move |app| {
                            if let Some(wv) = app.get_webview(&label_of(&id)) {
                                let _ = wv.navigate(u);
                            }
                        });
                    }
                    NavDecision::External => {
                        let _ = a3.opener().open_url(u.as_str(), None::<&str>);
                    }
                    NavDecision::Block => {}
                }
            }
            NewWindowResponse::Deny
        })
        .on_download(move |_wv, ev| on_download(&a4, &i4, ev))
        .on_permission_request(move |_wv, kind| {
            let d = permission_decision(kind);
            if d == PermissionResponse::Deny {
                log::warn!("hub webview: từ chối quyền trình duyệt {kind}");
            }
            d
        })
        // Required for HTML5 drag-and-drop on Windows (Tauri otherwise replaces WebView2's drop handler); the app
        // does not use Tauri's DragDropEvent.
        .disable_drag_drop_handler();

    #[cfg(target_os = "macos")]
    {
        builder = builder.data_store_identifier(hubs::data_store_id(id)).user_agent(&macos_user_agent());
    }
    #[cfg(windows)]
    {
        let core = app.state::<Core>();
        builder = builder.data_directory(core.data_dir.join("webviews").join(id));
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let core = app.state::<Core>();
        builder = builder.data_directory(core.data_dir.join("webviews").join(id));
    }

    let zoom = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.zoom.get(id).copied()
    };
    let wv = window
        .add_child(builder, LogicalPosition::new(RAIL_W, 0.0), LogicalSize::new(800.0, 600.0))
        .map_err(|e| format!("Không tạo được WebView: {e}"))?;
    let _ = wv.hide();
    if let Some(z) = zoom {
        let _ = wv.set_zoom(z);
    }

    #[cfg(windows)]
    {
        let suffix = user_agent_suffix();
        let wv2 = wv.clone();
        let target = start.clone();
        let _ = wv.with_webview(move |pw| {
            platform_windows::append_user_agent(&pw, &suffix);
            let _ = wv2.navigate(target);
        });
    }

    let evict = {
        let core = app.state::<Core>();
        let mut g = core.lock();
        let limit = g.reg.settings.live_limit();
        g.touch_lru(id, limit)
    };
    sleep_hubs(app, evict);
    layout(app);
    Ok(())
}

/// Put hubs to sleep: remember their URL, close the webview (the data store stays on disk).
pub fn sleep_hubs(app: &AppHandle, ids: Vec<String>) {
    let active = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.active.clone()
    };
    for id in ids {
        if Some(&id) == active.as_ref() {
            // never evict the hub being shown; put it back as most recent
            let core = app.state::<Core>();
            core.lock().lru.push(id);
            continue;
        }
        if let Some(wv) = app.get_webview(&label_of(&id)) {
            let url = wv.url().ok();
            let _ = wv.close();
            let core = app.state::<Core>();
            let mut g = core.lock();
            if let Some(lu) = url.as_ref().and_then(hubs::last_url_of) {
                if let Some(h) = g.reg.get_mut(&id) {
                    h.last_url = Some(lu);
                }
                core.persist(&g);
            }
            g.status.insert(id.clone(), HubStatus::Idle);
            log::info!("hub {id}: ngủ (vượt giới hạn WebView sống)");
        }
    }
    emit_state(app);
}

/// The active hub's webview, if it is alive.
pub fn active_webview(app: &AppHandle) -> Option<tauri::Webview> {
    let id = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.active.clone()?
    };
    app.get_webview(&label_of(&id))
}

/// Select the hub `offset` steps away in the visible rail order (Cmd/Ctrl+Shift+[ / ]).
pub async fn select_relative(app: AppHandle, offset: i64) -> Result<(), String> {
    let next = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.neighbour(g.reg.active.as_deref(), offset)
    };
    match next {
        Some(id) => select(app, id).await,
        None => Ok(()),
    }
}

/// Select the n-th (1-based) visible hub (Cmd/Ctrl+1…9).
pub async fn select_index(app: AppHandle, n: usize) -> Result<(), String> {
    let id = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.rail_order(false).get(n.wrapping_sub(1)).cloned()
    };
    match id {
        Some(id) => select(app, id).await,
        None => Ok(()),
    }
}

/// Show/hide a shell dialog over the content area (the active hub webview is hidden while it is open).
pub fn set_overlay(app: &AppHandle, open: bool) {
    {
        let core = app.state::<Core>();
        core.lock().overlay = open;
    }
    layout(app);
    if open {
        if let Some(shell) = app.get_webview(SHELL_LABEL) {
            let _ = shell.set_focus();
        }
    } else if let Some(wv) = active_webview(app) {
        let _ = wv.set_focus();
    }
}

/// Label → hub id, used by menu actions.
pub fn active_hub_id(app: &AppHandle) -> Option<String> {
    active_webview(app).and_then(|w| hub_id_of_label(w.label()).map(str::to_string))
}

#[cfg(windows)]
mod platform_windows {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings2;
    use windows::core::{Interface, HSTRING, PWSTR};

    /// Append the C7 token to WebView2's default UA (keeps the real Edge/Chromium version).
    pub fn append_user_agent(pw: &tauri::webview::PlatformWebview, suffix: &str) {
        unsafe {
            let Ok(core) = pw.controller().CoreWebView2() else { return };
            let Ok(settings) = core.Settings() else { return };
            let Ok(s2) = settings.cast::<ICoreWebView2Settings2>() else { return };
            let mut ua = PWSTR::null();
            if s2.UserAgent(&mut ua).is_err() || ua.is_null() {
                return;
            }
            let cur = ua.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(ua.0 as *const _));
            if !cur.contains("CfharnessDesktop/") {
                let _ = s2.SetUserAgent(&HSTRING::from(format!("{cur}{suffix}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_pages_get_no_sensitive_permission() {
        for k in [
            PermissionKind::Microphone,
            PermissionKind::Camera,
            PermissionKind::Geolocation,
            PermissionKind::Notifications,
            PermissionKind::ClipboardRead,
            PermissionKind::DisplayCapture,
            PermissionKind::Midi,
            PermissionKind::Sensors,
            PermissionKind::MediaKeySystemAccess,
            PermissionKind::LocalFonts,
            PermissionKind::WindowManagement,
            PermissionKind::AutomaticDownloads,
            PermissionKind::FileSystemAccess,
            PermissionKind::Other,
        ] {
            assert_eq!(permission_decision(k), PermissionResponse::Deny, "{k}");
        }
        // never Allow: a page can never get a grant without the platform asking
        assert_eq!(permission_decision(PermissionKind::Autoplay), PermissionResponse::Default);
        assert_eq!(permission_decision(PermissionKind::PointerLock), PermissionResponse::Default);
    }
}
