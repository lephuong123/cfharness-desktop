//! IPC commands for the SHELL UI only (granted to the `shell` webview in `capabilities/shell.json`).
//! Hub webviews never get these: they carry untrusted agent content.

use crate::core::{Core, ShellState};
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
