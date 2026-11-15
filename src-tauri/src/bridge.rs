//! IPC surface for HUB webviews (PLAN §5.2, contract C3/C4).
//!
//! A hub page is untrusted data (it renders agent text). It gets exactly the commands in [`HUB_COMMANDS`], through a
//! capability added at runtime per hub webview and limited to the hub's registered origin (`grant`). Tauri's
//! isolation pattern is NOT used: it injects a `<iframe src="isolation://…">` into the page, which the hub's own CSP
//! (`default-src 'self'`, no `frame-src` for custom schemes) blocks, and the pattern is app-wide (it would also wrap
//! the shell). Instead every payload is validated strictly here (pure `validate_*` functions with tests) and every
//! call re-checks that the calling webview is a hub webview currently showing its registered origin (`check_caller`).
//!
//! Extra rules beyond the payload shape:
//! - `speech_start`, `clipboard_write`, `download_save` only for the ACTIVE hub while the main window is focused
//!   (a background hub cannot open the microphone or touch the clipboard);
//! - `download_save` always goes through the native Save dialog (the user picks the file);
//! - `badge_update` only carries ints/bools/version; notification text is fixed (`notify.rs`).

use crate::core::Core;
use crate::nav::Origin;
use crate::{notify, speech, webviews};
use base64::Engine as _;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::ipc::CapabilityBuilder;
use tauri::{AppHandle, Manager, Webview};
use url::Url;

/// The only commands a hub page may call (also declared in `build.rs`; never granted to the shell).
pub const HUB_COMMANDS: [&str; 6] =
    ["badge_update", "speech_start", "speech_stop", "speech_available", "clipboard_write", "download_save"];

pub const CLIPBOARD_MAX: usize = 1024 * 1024;
/// base64 text of a `download_save` payload (≈ 48 MB of file data).
pub const DOWNLOAD_B64_MAX: usize = 64 * 1024 * 1024;
pub const COUNT_MAX: i64 = 1_000_000;
pub const BADGE_POLL_MIN_S: u64 = 5;
pub const BADGE_POLL_MAX_S: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// Not an object / unknown or missing field / wrong type / out of range.
    Payload(&'static str),
    /// Caller is not a registered hub webview on its origin.
    Caller,
    /// Allowed only for the active hub in the focused window.
    NotActive,
}

impl BridgeError {
    pub fn message(&self) -> String {
        match self {
            BridgeError::Payload(m) => format!("payload không hợp lệ: {m}"),
            BridgeError::Caller => "lệnh chỉ dành cho trang của hub đã đăng ký".into(),
            BridgeError::NotActive => "chỉ hub đang mở ở cửa sổ đang dùng mới được gọi lệnh này".into(),
        }
    }
}

// ------------------------------------------------------------------------------------------------ validation

fn object<'a>(v: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>, BridgeError> {
    let m = v.as_object().ok_or(BridgeError::Payload("không phải object"))?;
    if m.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(BridgeError::Payload("có trường lạ"));
    }
    Ok(m)
}

fn count(m: &Map<String, Value>, k: &'static str) -> Result<u32, BridgeError> {
    let n = m.get(k).and_then(Value::as_i64).ok_or(BridgeError::Payload(k))?;
    // as_i64 rejects floats like 1.5 and strings; also reject 1.0 written as float
    if m.get(k).map(|v| v.is_f64()).unwrap_or(false) || !(0..=COUNT_MAX).contains(&n) {
        return Err(BridgeError::Payload(k));
    }
    Ok(n as u32)
}

/// C3 counts as reported by the page (copied from `/app/badge`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Badge {
    pub approvals: u32,
    pub questions: u32,
    pub running: u32,
    pub estop: bool,
    pub version: String,
}

pub fn valid_version(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

pub fn validate_badge(v: &Value) -> Result<Badge, BridgeError> {
    let m = object(v, &["approvals", "questions", "running", "estop", "version"])?;
    let estop = m.get("estop").and_then(Value::as_bool).ok_or(BridgeError::Payload("estop"))?;
    let version = m.get("version").and_then(Value::as_str).ok_or(BridgeError::Payload("version"))?;
    if !valid_version(version) {
        return Err(BridgeError::Payload("version"));
    }
    Ok(Badge {
        approvals: count(m, "approvals")?,
        questions: count(m, "questions")?,
        running: count(m, "running")?,
        estop,
        version: version.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechOpts {
    pub lang: String,
    pub interim: bool,
}

/// Languages the hub's `voice.js` uses (`vi-VN` default, `en-US`).
pub const SPEECH_LANGS: [&str; 2] = ["vi-VN", "en-US"];

pub fn validate_speech_opts(v: &Value) -> Result<SpeechOpts, BridgeError> {
    if v.is_null() {
        return Ok(SpeechOpts { lang: "vi-VN".into(), interim: true });
    }
    let m = object(v, &["lang", "interim"])?;
    let lang = match m.get("lang") {
        None => "vi-VN",
        Some(l) => l.as_str().ok_or(BridgeError::Payload("lang"))?,
    };
    if !SPEECH_LANGS.contains(&lang) {
        return Err(BridgeError::Payload("lang"));
    }
    let interim = match m.get("interim") {
        None => true,
        Some(b) => b.as_bool().ok_or(BridgeError::Payload("interim"))?,
    };
    Ok(SpeechOpts { lang: lang.to_string(), interim })
}

pub fn validate_clipboard(v: &Value) -> Result<String, BridgeError> {
    let s = v.as_str().ok_or(BridgeError::Payload("text"))?;
    if s.len() > CLIPBOARD_MAX {
        return Err(BridgeError::Payload("text quá 1 MB"));
    }
    Ok(s.to_string())
}

/// File name for the Save dialog: no path separators, control/bidi chars, leading dots; ≤ 120 chars.
pub fn safe_file_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .filter(|c| !crate::directory::is_unsafe_char(*c))
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    let out: String = trimmed.chars().take(120).collect();
    if out.is_empty() {
        "tai-ve".to_string()
    } else {
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadReq {
    pub name: String,
    pub data: Vec<u8>,
}

pub fn validate_download(v: &Value) -> Result<DownloadReq, BridgeError> {
    let m = object(v, &["name", "data"])?;
    let name = m.get("name").and_then(Value::as_str).ok_or(BridgeError::Payload("name"))?;
    if name.len() > 1024 {
        return Err(BridgeError::Payload("name"));
    }
    let data = m.get("data").and_then(Value::as_str).ok_or(BridgeError::Payload("data"))?;
    if data.len() > DOWNLOAD_B64_MAX {
        return Err(BridgeError::Payload("data quá lớn"));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| BridgeError::Payload("data"))?;
    Ok(DownloadReq { name: safe_file_name(name), data: bytes })
}

/// The calling webview must be `hub-<id>` of a registered hub, currently on that hub's registered origin.
pub fn check_caller(label: &str, url: Option<&Url>, origins: &HashMap<String, Origin>) -> Result<String, BridgeError> {
    let id = label.strip_prefix("hub-").ok_or(BridgeError::Caller)?;
    let origin = origins.get(id).ok_or(BridgeError::Caller)?;
    match url {
        Some(u) if origin.matches(u) => Ok(id.to_string()),
        _ => Err(BridgeError::Caller),
    }
}

/// URLPattern for the capability `remote.urls` of a hub origin.
pub fn remote_pattern(origin: &Origin) -> String {
    format!("{}/*", origin.as_string())
}

/// Badge poll interval sent back to the page.
pub fn poll_interval(setting: u64) -> u64 {
    setting.clamp(BADGE_POLL_MIN_S, BADGE_POLL_MAX_S)
}

// ------------------------------------------------------------------------------------------------ inject script

const INJECT_JS: &str = include_str!("../../inject/hub.js");

/// Initialization script for a hub webview: the template with the registered origin and the poll interval baked
/// in as JSON literals. The script only runs on that origin, top frame.
pub fn init_script(origin: &Origin, poll_s: u64) -> String {
    let o = serde_json::to_string(&origin.as_string()).unwrap_or_else(|_| "\"\"".into());
    INJECT_JS.replace("__CFH_ORIGIN__", &o).replace("__CFH_POLL_S__", &poll_interval(poll_s).to_string())
}

// ------------------------------------------------------------------------------------------------ capability

/// (label, origin) pairs already granted — capabilities can only be added, so grant each pair once.
static GRANTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Grant the hub commands to webview `label` for pages of `origin` only.
pub fn grant(app: &AppHandle, label: &str, origin: &Origin) {
    let key = (label.to_string(), origin.as_string());
    {
        let mut g = GRANTED.lock().unwrap_or_else(|p| p.into_inner());
        if g.contains(&key) {
            return;
        }
        g.push(key.clone());
    }
    let ident = format!("hub-bridge-{}", crate::hubs::short_hash(&format!("{}|{}", key.0, key.1)));
    let mut cap = CapabilityBuilder::new(ident).webview(label).remote(remote_pattern(origin)).local(false);
    for c in HUB_COMMANDS {
        cap = cap.permission(format!("allow-{}", c.replace('_', "-")));
    }
    if let Err(e) = app.add_capability(cap) {
        log::error!("không cấp được cầu nối cho {label}: {e}");
    }
}

// ------------------------------------------------------------------------------------------------ commands

fn caller(app: &AppHandle, wv: &Webview) -> Result<String, String> {
    let url = wv.url().ok();
    let core = app.state::<Core>();
    let g = core.lock();
    check_caller(wv.label(), url.as_ref(), &g.origins).map_err(|e| {
        log::warn!("cầu nối: từ chối {} ({})", wv.label(), e.message());
        e.message()
    })
}

/// Caller must be the active hub and the main window focused.
fn caller_active(app: &AppHandle, wv: &Webview) -> Result<String, String> {
    let id = caller(app, wv)?;
    let active = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.active.clone()
    };
    let focused = app.get_window(webviews::MAIN_WINDOW).and_then(|w| w.is_focused().ok()).unwrap_or(false);
    if active.as_deref() != Some(id.as_str()) || !focused {
        return Err(BridgeError::NotActive.message());
    }
    Ok(id)
}

#[derive(Serialize)]
pub struct BadgeAck {
    /// Seconds until the next poll (settings.badge_poll_s, clamped).
    pub next_s: u64,
}

#[tauri::command]
pub fn badge_update(app: AppHandle, webview: Webview, badge: Value) -> Result<BadgeAck, String> {
    let id = caller(&app, &webview)?;
    let b = validate_badge(&badge).map_err(|e| e.message())?;
    notify::on_badge(&app, &id, b);
    let next_s = {
        let core = app.state::<Core>();
        let g = core.lock();
        poll_interval(g.reg.settings.badge_poll_s)
    };
    Ok(BadgeAck { next_s })
}

#[tauri::command]
pub fn speech_available(app: AppHandle, webview: Webview) -> Result<bool, String> {
    caller(&app, &webview)?;
    Ok(speech::available(&app))
}

#[tauri::command]
pub async fn speech_start(app: AppHandle, webview: Webview, opts: Option<Value>) -> Result<(), String> {
    let id = caller_active(&app, &webview)?;
    let o = validate_speech_opts(&opts.unwrap_or(Value::Null)).map_err(|e| e.message())?;
    speech::start(app, id, o).await.map_err(|code| code.to_string())
}

#[tauri::command]
pub async fn speech_stop(app: AppHandle, webview: Webview) -> Result<(), String> {
    let id = caller(&app, &webview)?;
    speech::stop_for(&id);
    Ok(())
}

#[tauri::command]
pub fn clipboard_write(app: AppHandle, webview: Webview, text: Value) -> Result<(), String> {
    caller_active(&app, &webview)?;
    let t = validate_clipboard(&text).map_err(|e| e.message())?;
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("không mở được clipboard: {e}"))?;
    cb.set_text(t).map_err(|e| format!("không ghi được clipboard: {e}"))
}

#[tauri::command]
pub async fn download_save(app: AppHandle, webview: Webview, file: Value) -> Result<bool, String> {
    let id = caller_active(&app, &webview)?;
    let req = validate_download(&file).map_err(|e| e.message())?;
    let mut dlg = rfd::AsyncFileDialog::new().set_title("Lưu tệp").set_file_name(&req.name);
    if let Some(d) = dirs::download_dir() {
        dlg = dlg.set_directory(d);
    }
    let Some(handle) = dlg.save_file().await else { return Ok(false) };
    let path = handle.path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&path, &req.data))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("không ghi được tệp: {e}"))?;
    log::info!("hub {id}: đã lưu tệp từ cầu nối");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn badge_ok() {
        let b =
            validate_badge(&json!({"approvals":2,"questions":0,"running":5,"estop":false,"version":"0.2.10"})).unwrap();
        assert_eq!(b.approvals, 2);
        assert_eq!(b.running, 5);
        assert_eq!(b.version, "0.2.10");
    }

    #[test]
    fn badge_rejects_bad_input() {
        let ok = json!({"approvals":1,"questions":1,"running":1,"estop":false,"version":"1.0"});
        assert!(validate_badge(&ok).is_ok());
        let bad = [
            json!(null),
            json!([1, 2]),
            json!("x"),
            // extra field (free text smuggling)
            json!({"approvals":1,"questions":1,"running":1,"estop":false,"version":"1.0","title":"<b>x</b>"}),
            // missing field
            json!({"approvals":1,"questions":1,"running":1,"estop":false}),
            // wrong types
            json!({"approvals":"1","questions":1,"running":1,"estop":false,"version":"1.0"}),
            json!({"approvals":1.5,"questions":1,"running":1,"estop":false,"version":"1.0"}),
            json!({"approvals":1.0,"questions":1,"running":1,"estop":false,"version":"1.0"}),
            json!({"approvals":1,"questions":1,"running":1,"estop":"no","version":"1.0"}),
            // range
            json!({"approvals":-1,"questions":1,"running":1,"estop":false,"version":"1.0"}),
            json!({"approvals":1,"questions":1_000_001,"running":1,"estop":false,"version":"1.0"}),
            // version must be a short token
            json!({"approvals":1,"questions":1,"running":1,"estop":false,"version":"1.0 <script>"}),
            json!({"approvals":1,"questions":1,"running":1,"estop":false,"version":""}),
            json!({"approvals":1,"questions":1,"running":1,"estop":false,"version":"1".repeat(33)}),
        ];
        for b in bad {
            assert!(validate_badge(&b).is_err(), "should reject {b}");
        }
    }

    #[test]
    fn speech_opts() {
        assert_eq!(
            validate_speech_opts(&json!({"lang":"vi-VN","interim":true})).unwrap(),
            SpeechOpts { lang: "vi-VN".into(), interim: true }
        );
        assert_eq!(validate_speech_opts(&json!(null)).unwrap().lang, "vi-VN");
        assert_eq!(validate_speech_opts(&json!({"lang":"en-US"})).unwrap().lang, "en-US");
        for b in [
            json!({"lang":"fr-FR"}),
            json!({"lang":1}),
            json!({"interim":"yes"}),
            json!({"lang":"vi-VN","continuous":true}),
            json!([]),
            json!("vi-VN"),
        ] {
            assert!(validate_speech_opts(&b).is_err(), "should reject {b}");
        }
    }

    #[test]
    fn clipboard_limits() {
        assert_eq!(validate_clipboard(&json!("xin chào")).unwrap(), "xin chào");
        assert!(validate_clipboard(&json!("a".repeat(CLIPBOARD_MAX))).is_ok());
        assert!(validate_clipboard(&json!("a".repeat(CLIPBOARD_MAX + 1))).is_err());
        assert!(validate_clipboard(&json!(5)).is_err());
        assert!(validate_clipboard(&json!({"text":"x"})).is_err());
    }

    #[test]
    fn download_payload() {
        let d = validate_download(&json!({"name":"../../etc/passwd","data":"aGVsbG8="})).unwrap();
        assert_eq!(d.data, b"hello");
        assert!(!d.name.contains('/') && !d.name.starts_with('.'));
        assert_eq!(safe_file_name("..\\x\u{202e}gpj.exe"), "_xgpj.exe");
        assert_eq!(safe_file_name("   "), "tai-ve");
        assert_eq!(safe_file_name("báo cáo.pdf"), "báo cáo.pdf");
        assert!(validate_download(&json!({"name":"a","data":"!!notbase64"})).is_err());
        assert!(validate_download(&json!({"name":"a"})).is_err());
        assert!(validate_download(&json!({"name":"a","data":"","path":"/tmp/x"})).is_err());
        assert!(validate_download(&json!({"name":5,"data":""})).is_err());
    }

    #[test]
    fn caller_must_match_registered_origin() {
        let mut origins = HashMap::new();
        origins.insert("loc-1".to_string(), Origin::parse("http://cfharness.localhost:8711").unwrap());
        let ok = Url::parse("http://cfharness.localhost:8711/manager").unwrap();
        assert_eq!(check_caller("hub-loc-1", Some(&ok), &origins).unwrap(), "loc-1");
        // other port / scheme / host
        for u in
            ["http://cfharness.localhost:8712/", "https://cfharness.localhost:8711/", "http://evil.localhost:8711/"]
        {
            let u = Url::parse(u).unwrap();
            assert_eq!(check_caller("hub-loc-1", Some(&u), &origins), Err(BridgeError::Caller));
        }
        // shell or unknown hub
        assert_eq!(check_caller("shell", Some(&ok), &origins), Err(BridgeError::Caller));
        assert_eq!(check_caller("hub-loc-2", Some(&ok), &origins), Err(BridgeError::Caller));
        assert_eq!(check_caller("hub-loc-1", None, &origins), Err(BridgeError::Caller));
        let blank = Url::parse("about:blank").unwrap();
        assert_eq!(check_caller("hub-loc-1", Some(&blank), &origins), Err(BridgeError::Caller));
    }

    #[test]
    fn patterns_and_script() {
        let o = Origin::parse("https://an.example.com").unwrap();
        assert_eq!(remote_pattern(&o), "https://an.example.com/*");
        let o2 = Origin::parse("http://cfharness.localhost:8711").unwrap();
        assert_eq!(remote_pattern(&o2), "http://cfharness.localhost:8711/*");
        let s = init_script(&o2, 1);
        assert!(s.contains("\"http://cfharness.localhost:8711\""));
        assert!(!s.contains("__CFH_"));
        assert_eq!(poll_interval(0), BADGE_POLL_MIN_S);
        assert_eq!(poll_interval(10_000), BADGE_POLL_MAX_S);
    }

    #[test]
    fn command_list_matches_build_rs() {
        let build = include_str!("../build.rs");
        for c in HUB_COMMANDS {
            assert!(build.contains(&format!("\"{c}\"")), "{c} missing in build.rs");
        }
        let shell = include_str!("../capabilities/shell.json");
        for c in HUB_COMMANDS {
            assert!(!shell.contains(&format!("allow-{}", c.replace('_', "-"))), "{c} must not be granted to the shell");
        }
    }
}
