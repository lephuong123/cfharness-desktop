//! IPC commands for the SHELL UI only (granted to the `shell` webview in `capabilities/shell.json`).
//! Hub webviews never get these: they carry untrusted agent content.

use crate::core::{Core, ShellState};
use crate::remote::{self, ImportPreview, SyncSummary};
use crate::{local, webviews};
use tauri::{AppHandle, Manager};

#[tauri::command]
pub fn get_state(app: AppHandle) -> ShellState {
    let core = app.state::<Core>();
    let s = core.lock().shell_state();
    log::debug!("shell: get_state ({} hub)", s.hubs.len());
    s
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[tauri::command]
pub async fn select_hub(app: AppHandle, id: String) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("id không hợp lệ".into());
    }
    webviews::select(app, id).await
}

/// Retry a hub in error: drop its webview and start again.
#[tauri::command]
pub async fn retry_hub(app: AppHandle, id: String) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("id không hợp lệ".into());
    }
    if let Some(wv) = app.get_webview(&webviews::label_of(&id)) {
        let _ = wv.close();
    }
    {
        let core = app.state::<Core>();
        let mut g = core.lock();
        g.lru.retain(|x| x != &id);
        g.relogin.remove(&id);
        g.bin = local::find_binary();
    }
    webviews::select(app, id).await
}

/// Rediscover local hubs and the `cfharness` binary.
pub fn refresh_local_inner(app: &AppHandle) {
    let homes = local::discover_system();
    let bin = local::find_binary();
    {
        let core = app.state::<Core>();
        let mut g = core.lock();
        g.bin = bin;
        if g.reg.merge_local(&homes) {
            core.persist(&g);
        }
    }
    webviews::emit_state(app);
}

#[tauri::command]
pub async fn refresh_local(app: AppHandle) -> Result<(), String> {
    refresh_local_inner(&app);
    let active = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.active.clone().filter(|a| app.get_webview(&webviews::label_of(a)).is_none())
    };
    if let Some(id) = active {
        webviews::select(app, id).await?;
    }
    Ok(())
}

/// Create/open the default hub `~/.cfharness` (the CLI initialises it on first run).
#[tauri::command]
pub async fn create_default_hub(app: AppHandle) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Không xác định được thư mục người dùng")?.join(".cfharness");
    let id = crate::hubs::local_id(&home.to_string_lossy());
    {
        let core = app.state::<Core>();
        let mut g = core.lock();
        if g.bin.is_none() {
            g.bin = local::find_binary();
        }
        if g.bin.is_none() {
            return Err("Chưa cài cfharness trên máy này".into());
        }
        g.reg.merge_local(&[home]);
        core.persist(&g);
    }
    webviews::select(app, id).await
}

pub fn set_find_open(app: &AppHandle, open: bool) {
    let has_hub = webviews::active_webview(app).is_some();
    {
        let core = app.state::<Core>();
        core.lock().find_open = open && has_hub;
    }
    webviews::layout(app);
    webviews::emit_state(app);
    if open && has_hub {
        if let Some(shell) = app.get_webview(webviews::SHELL_LABEL) {
            let _ = shell.set_focus();
        }
    } else if let Some(wv) = webviews::active_webview(app) {
        let _ = wv.set_focus();
    }
}

/// JS for `window.find` in the hub page; the query is embedded as a JSON string literal.
pub fn find_script(query: &str, backwards: bool) -> String {
    let q = serde_json::to_string(query).unwrap_or_else(|_| "\"\"".into());
    format!("(function(){{try{{window.find({q},false,{backwards},true,false,false,false);}}catch(e){{}}}})();")
}

#[tauri::command]
pub fn find_in_page(app: AppHandle, query: String, backwards: bool) -> Result<(), String> {
    if query.chars().count() > 500 {
        return Err("Chuỗi tìm quá dài".into());
    }
    if query.is_empty() {
        return Ok(());
    }
    if let Some(wv) = webviews::active_webview(&app) {
        wv.eval(find_script(&query, backwards)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn find_close(app: AppHandle) {
    if let Some(wv) = webviews::active_webview(&app) {
        let _ = wv.eval("try{window.getSelection().removeAllRanges()}catch(e){}");
    }
    set_find_open(&app, false);
}

// ------------------------------------------------------------------------------------------- cloud hubs (P2)

/// "Thêm hub / Import URL" → step 1: inspect the URL (C2 hub or C5 directory), nothing is written yet.
#[tauri::command]
pub async fn inspect_url(app: AppHandle, url: String) -> Result<ImportPreview, String> {
    if url.len() > 4096 {
        return Err("URL quá dài".into());
    }
    remote::inspect(&app, &url).await
}

/// Step 2: apply what `inspect_url` found (`nonce` from its result).
#[tauri::command]
pub async fn confirm_import(app: AppHandle, nonce: u64, name: Option<String>) -> Result<SyncSummary, String> {
    let summary = remote::confirm(&app, nonce, name).await?;
    if let Some(id) = summary.select.clone() {
        webviews::set_overlay(&app, false);
        let _ = webviews::select(app, id).await;
    }
    Ok(summary)
}

#[tauri::command]
pub async fn sync_directory(app: AppHandle, id: String) -> Result<SyncSummary, String> {
    if !valid_id(&id) {
        return Err("id không hợp lệ".into());
    }
    remote::sync_directory(&app, &id).await
}

#[tauri::command]
pub async fn remove_directory(app: AppHandle, id: String, remove_hubs: bool, wipe: bool) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("id không hợp lệ".into());
    }
    remote::remove_directory(&app, &id, remove_hubs, wipe).await
}

/// Answer an origin change proposed by a directory (`to` must match the pending proposal).
#[tauri::command]
pub fn resolve_origin_change(app: AppHandle, hub: String, to: String, accept: bool) -> Result<(), String> {
    if !valid_id(&hub) {
        return Err("id không hợp lệ".into());
    }
    remote::resolve_origin(&app, &hub, &to, accept)
}

// ------------------------------------------------------------------------------------------- hub management

fn edit_registry(
    app: &AppHandle,
    f: impl FnOnce(&mut crate::hubs::Registry) -> Result<(), String>,
) -> Result<(), String> {
    {
        let core = app.state::<Core>();
        let mut g = core.lock();
        if g.read_only {
            return Err("hubs.json do bản app mới hơn ghi — bản này chỉ đọc".into());
        }
        f(&mut g.reg)?;
        core.persist(&g);
    }
    webviews::emit_state(app);
    Ok(())
}

#[tauri::command]
pub fn rename_hub(app: AppHandle, id: String, name: String) -> Result<(), String> {
    if !valid_id(&id) || name.len() > 1024 {
        return Err("Dữ liệu không hợp lệ".into());
    }
    edit_registry(&app, |r| r.rename(&id, &name))
}

#[tauri::command]
pub fn set_hub_color(app: AppHandle, id: String, color: String) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("id không hợp lệ".into());
    }
    edit_registry(&app, |r| r.set_color(&id, &color))
}

#[tauri::command]
pub async fn set_hub_hidden(app: AppHandle, id: String, hidden: bool) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("id không hợp lệ".into());
    }
    let was_active = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.active.as_deref() == Some(id.as_str())
    };
    edit_registry(&app, |r| r.set_hidden(&id, hidden))?;
    if hidden && was_active {
        if let Some(wv) = app.get_webview(&webviews::label_of(&id)) {
            let _ = wv.hide();
        }
        let next = {
            let core = app.state::<Core>();
            let g = core.lock();
            g.reg.active.clone()
        };
        match next {
            Some(n) => webviews::select(app, n).await?,
            None => webviews::layout(&app),
        }
    }
    Ok(())
}

#[tauri::command]
pub fn move_hub(app: AppHandle, id: String, delta: i32) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("id không hợp lệ".into());
    }
    edit_registry(&app, |r| r.move_in_group(&id, delta).map(|_| ()))
}

/// Remove a remote hub; `wipe` also deletes its cookies/session (webview data store).
#[tauri::command]
pub async fn remove_hub(app: AppHandle, id: String, wipe: bool) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("id không hợp lệ".into());
    }
    remote::remove_hub(&app, &id, wipe).await
}

// ------------------------------------------------------------------------------------------- switching

#[tauri::command]
pub fn set_overlay(app: AppHandle, open: bool) {
    webviews::set_overlay(&app, open);
}

#[tauri::command]
pub async fn select_relative(app: AppHandle, offset: i64) -> Result<(), String> {
    webviews::select_relative(app, offset.clamp(-1, 1)).await
}

// ------------------------------------------------------------------------------------------- install (P3)

/// Onboarding: what the automatic install would do (latest tag, uv/git/claude found?). Runs nothing.
#[tauri::command]
pub async fn install_plan(app: AppHandle) -> Result<crate::installer::Plan, String> {
    crate::installer::plan(&app).await
}

/// The user clicked "Cài": run the plan identified by `nonce`.
#[tauri::command]
pub async fn install_run(app: AppHandle, nonce: u64) -> Result<(), String> {
    crate::installer::run(app, nonce).await
}

// ------------------------------------------------------------------------------------------- settings (P3)

#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    live_webviews: u64,
    notify: bool,
    badge_poll_s: u64,
    autostart: bool,
) -> Result<(), String> {
    let (autostart_changed, limit, active) = {
        let core = app.state::<Core>();
        let mut g = core.lock();
        if g.read_only {
            return Err("hubs.json do bản app mới hơn ghi — bản này chỉ đọc".into());
        }
        let before = g.reg.settings.autostart;
        g.reg.settings.update(live_webviews, notify, badge_poll_s, autostart)?;
        core.persist(&g);
        (before != autostart, g.reg.settings.live_limit(), g.reg.active.clone())
    };
    if autostart_changed {
        use tauri_plugin_autostart::ManagerExt;
        let al = app.autolaunch();
        let r = if autostart { al.enable() } else { al.disable() };
        if let Err(e) = r {
            log::warn!("không đổi được tự khởi động: {e}");
            let core = app.state::<Core>();
            let mut g = core.lock();
            g.reg.settings.autostart = !autostart;
            core.persist(&g);
            drop(g);
            webviews::emit_state(&app);
            return Err(format!("Không đổi được tự khởi động cùng hệ điều hành: {e}"));
        }
    }
    // Fewer live webviews: put the least recently used ones to sleep now.
    let evict = {
        let core = app.state::<Core>();
        let mut g = core.lock();
        match active.filter(|a| g.lru.contains(a)) {
            Some(a) => g.touch_lru(&a, limit),
            None => {
                let mut v = vec![];
                while g.lru.len() > limit {
                    v.push(g.lru.remove(0));
                }
                v
            }
        }
    };
    webviews::sleep_hubs(&app, evict);
    webviews::emit_state(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert!(valid_id("loc-1a2b3c4d"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
        assert!(!valid_id("a b"));
    }

    #[test]
    fn find_script_escapes_query() {
        let s = find_script("a\");alert(1);//\u{2028}", true);
        assert!(s.contains(r#"window.find("a\");alert(1);//"#));
        assert!(s.contains(",false,true,true,"));
        assert!(!s.contains("\");alert(1);//\","));
    }
}
