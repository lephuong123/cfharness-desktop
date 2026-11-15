//! Native speech recognition behind `window.cfhNative.speech` (C4, PLAN G1/G2).
//!
//! - macOS: SFSpeechRecognizer (`macos.rs` + `native/speech_mac.m`), on-device when the locale supports it.
//! - Windows: whisper.cpp (`whisper.rs`) only when built with the `whisper` cargo feature; otherwise `available()` is
//!   false and the hub's voice buttons stay hidden (hub behaviour unchanged).
//!
//! One microphone session at a time, owned by one hub webview. Events go back to that webview only, as a JSON literal
//! passed to the page's `__cfhNativeDispatch` (installed by `inject/hub.js`). The session stops when its hub stops
//! being the active one (`stop_unless_active`, called from `webviews::layout`).

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

// Compiled whenever the feature is on (so `cargo check --features whisper` works on macOS too), used off macOS.
#[cfg(feature = "whisper")]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod whisper;
#[cfg(all(not(target_os = "macos"), feature = "whisper"))]
use whisper as imp;

#[cfg(all(not(target_os = "macos"), not(feature = "whisper")))]
mod imp {
    use super::Sink;
    pub fn available(_lang: &str) -> bool {
        false
    }
    pub fn start(_lang: &str, _interim: bool, _sink: Sink) -> Result<(), String> {
        Err("language-not-supported".into())
    }
    pub fn stop() {}
}

use crate::bridge::SpeechOpts;
use crate::webviews;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Interim (`is_final=false`: the current partial text) or final segment.
    Result {
        text: String,
        is_final: bool,
    },
    End,
    Error(String),
}

pub type Sink = Box<dyn Fn(Event) + Send + Sync>;

static OWNER: Mutex<Option<String>> = Mutex::new(None);
static DATA_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// App data dir (whisper model location). Set once at startup.
pub fn set_data_dir(p: &std::path::Path) {
    let _ = DATA_DIR.set(p.to_path_buf());
}

#[allow(dead_code)]
pub(crate) fn data_dir() -> Option<&'static std::path::Path> {
    DATA_DIR.get().map(|p| p.as_path())
}

fn owner() -> std::sync::MutexGuard<'static, Option<String>> {
    OWNER.lock().unwrap_or_else(|p| p.into_inner())
}

/// Recognized text going into the page: no control/bidi/invisible characters, ≤ 4000 chars.
pub fn clean_text(s: &str) -> String {
    s.chars().filter(|c| !crate::directory::is_unsafe_char(*c) || *c == ' ').take(4000).collect()
}

/// Error codes passed to the page: a short lowercase token (anything else → "native").
pub fn clean_code(s: &str) -> String {
    let ok = !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    if ok {
        s.to_string()
    } else {
        "native".into()
    }
}

#[derive(Serialize)]
struct Wire<'a> {
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(rename = "final", skip_serializing_if = "Option::is_none")]
    is_final: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

/// JS evaluated in the hub page for one event (payload = JSON literal, U+2028/2029 escaped).
pub fn event_js(ev: &Event) -> String {
    let w = match ev {
        Event::Result { text, is_final } => {
            Wire { kind: "result", text: Some(clean_text(text)), is_final: Some(*is_final), code: None }
        }
        Event::End => Wire { kind: "end", text: None, is_final: None, code: None },
        Event::Error(c) => Wire { kind: "error", text: None, is_final: None, code: Some(clean_code(c)) },
    };
    let json = serde_json::to_string(&w)
        .unwrap_or_else(|_| "{}".into())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    format!("window.__cfhNativeDispatch&&window.__cfhNativeDispatch({json});")
}

/// A native backend exists in this build (macOS always; Windows only with the `whisper` feature).
pub fn compiled_in() -> bool {
    cfg!(any(target_os = "macos", feature = "whisper"))
}

pub fn available(_app: &AppHandle) -> bool {
    imp::available("vi-VN")
}

/// Start listening for hub `hub` (caller already checked it is the active hub in the focused window).
pub async fn start(app: AppHandle, hub: String, opts: SpeechOpts) -> Result<(), String> {
    stop_any();
    *owner() = Some(hub.clone());
    let (a, h) = (app.clone(), hub.clone());
    let sink: Sink = Box::new(move |ev: Event| {
        if owner().as_deref() != Some(h.as_str()) {
            return;
        }
        if let Some(wv) = a.get_webview(&webviews::label_of(&h)) {
            let _ = wv.eval(event_js(&ev));
        }
    });
    let res = tauri::async_runtime::spawn_blocking(move || imp::start(&opts.lang, opts.interim, sink))
        .await
        .map_err(|_| "start".to_string())?;
    match res {
        Ok(()) => {
            log::info!("hub {hub}: bắt đầu nhận dạng giọng nói");
            Ok(())
        }
        Err(code) => {
            let mut o = owner();
            if o.as_deref() == Some(hub.as_str()) {
                *o = None;
            }
            log::info!("hub {hub}: không bắt đầu được nhận dạng giọng nói ({})", clean_code(&code));
            Err(clean_code(&code))
        }
    }
}

fn stop_any() {
    let had = owner().take();
    if had.is_some() {
        imp::stop();
    }
}

/// `speech_stop` from hub `hub`: stops only its own session.
pub fn stop_for(hub: &str) {
    let mine = {
        let mut o = owner();
        if o.as_deref() == Some(hub) {
            *o = None;
            true
        } else {
            false
        }
    };
    if mine {
        imp::stop();
    }
}

/// Stop the session when its hub is no longer the active (visible) one.
pub fn stop_unless_active(active: Option<&str>) {
    let stale = {
        let mut o = owner();
        match o.as_deref() {
            Some(h) if Some(h) != active => {
                *o = None;
                true
            }
            _ => false,
        }
    };
    if stale {
        log::info!("dừng nhận dạng giọng nói: hub không còn ở tiền cảnh");
        imp::stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_is_a_json_literal() {
        let js = event_js(&Event::Result { text: "chào\u{2028}\");alert(1);//\u{202e}".into(), is_final: true });
        assert!(js.starts_with("window.__cfhNativeDispatch&&window.__cfhNativeDispatch({"));
        assert!(js.contains(r#""kind":"result""#));
        assert!(js.contains(r#""final":true"#));
        assert!(js.contains(r#"\");alert(1);//"#)); // quote escaped inside the string
        assert!(!js.contains('\u{2028}') && !js.contains('\u{202e}'));
        assert_eq!(
            event_js(&Event::End),
            "window.__cfhNativeDispatch&&window.__cfhNativeDispatch({\"kind\":\"end\"});"
        );
        let e = event_js(&Event::Error("<img src=x>".into()));
        assert!(e.contains(r#""code":"native""#));
        assert!(event_js(&Event::Error("no-speech".into())).contains(r#""code":"no-speech""#));
    }

    #[test]
    fn owner_tracking() {
        *owner() = Some("a".into());
        stop_for("b");
        assert_eq!(owner().as_deref(), Some("a"));
        stop_unless_active(Some("a"));
        assert_eq!(owner().as_deref(), Some("a"));
        stop_unless_active(Some("b"));
        assert_eq!(owner().as_deref(), None);
    }
}
