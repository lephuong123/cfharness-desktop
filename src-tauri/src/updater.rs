//! App self-update (PLAN P4): `tauri-plugin-updater`, bundles signed with ed25519 (minisign) and published on the
//! GitHub Releases of the APP repo (`latest.json`). Independent of OS code signing (builds are unsigned / ad-hoc).
//!
//! - The public key + endpoint live in `tauri.conf.json` → `plugins.updater`. The repo ships PLACEHOLDERS; `release.yml`
//!   injects the real pubkey (`vars.TAURI_UPDATER_PUBKEY`) and `https://github.com/<owner>/<repo>/…` with `--config`.
//!   A build with placeholders reports "chưa cấu hình" and never contacts anything.
//! - Channels: `stable` = `releases/latest/download/latest.json`; `beta` = the rolling `beta` release
//!   (`releases/download/beta/latest.json`) of the same repo — derived from the configured stable endpoint (pure, tested).
//! - Nothing is installed without the user pressing "Cập nhật" in the shell; the plugin refuses a bundle whose signature
//!   does not match the embedded pubkey.
//! - Only the SHELL webview can call `update_*` (declared in `build.rs`, listed in `capabilities/shell.json`). The plugin's own
//!   JS commands are not granted to any capability.

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, EventTarget, Manager};

pub const PUBKEY_PLACEHOLDER: &str = "CFHARNESS_UPDATER_PUBKEY_PLACEHOLDER";
pub const REPO_PLACEHOLDER: &str = "OWNER/REPO";
const STABLE_SUFFIX: &str = "/releases/latest/download/latest.json";
const BETA_SUFFIX: &str = "/releases/download/beta/latest.json";
const NOTES_MAX: usize = 4000;
/// First automatic check after start, then once a day.
const FIRST_CHECK: Duration = Duration::from_secs(90);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn parse(s: &str) -> Option<Channel> {
        match s {
            "stable" => Some(Channel::Stable),
            "beta" => Some(Channel::Beta),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }
}

/// Updater config read from `plugins.updater` (pubkey + the stable endpoint).
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub stable_endpoint: String,
}

/// Why this build cannot self-update (shown in Settings; never an error dialog).
pub fn setup_from_config(cfg: Option<&Value>) -> Result<Setup, &'static str> {
    let cfg = cfg.ok_or("bản build này không có cấu hình cập nhật")?;
    let pubkey = cfg.get("pubkey").and_then(Value::as_str).unwrap_or("");
    if pubkey.is_empty() || pubkey.contains(PUBKEY_PLACEHOLDER) || !valid_pubkey(pubkey) {
        return Err("bản build này chưa có khoá công khai cập nhật (bản dev / tự build)");
    }
    let ep = cfg
        .get("endpoints")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .ok_or("bản build này chưa có địa chỉ cập nhật")?;
    if ep.contains(REPO_PLACEHOLDER) || repo_of(ep).is_none() {
        return Err("bản build này chưa có địa chỉ cập nhật (repo GitHub)");
    }
    Ok(Setup { stable_endpoint: ep.to_string() })
}

/// A Tauri updater public key: base64 of a minisign public key file ("untrusted comment: …\nRW…").
pub fn valid_pubkey(b64: &str) -> bool {
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return false;
    };
    let Ok(text) = String::from_utf8(raw) else {
        return false;
    };
    let mut lines = text.lines();
    let comment = lines.next().unwrap_or("");
    let key = lines.next().unwrap_or("").trim();
    comment.starts_with("untrusted comment:")
        && key.len() == 56
        && base64::engine::general_purpose::STANDARD.decode(key).map(|k| k.len() == 42).unwrap_or(false)
}

/// `owner/repo` of `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`, else None.
pub fn repo_of(endpoint: &str) -> Option<String> {
    let u = url::Url::parse(endpoint).ok()?;
    if u.scheme() != "https" || u.host_str() != Some("github.com") || u.port().is_some() || !u.username().is_empty() {
        return None;
    }
    if u.query().is_some() || u.fragment().is_some() {
        return None;
    }
    let rest = u.path().strip_suffix(STABLE_SUFFIX)?;
    let mut parts = rest.trim_start_matches('/').split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s != "."
            && s != ".."
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    };
    (ok(owner) && ok(repo)).then(|| format!("{owner}/{repo}"))
}

/// Endpoint for a channel, derived from the configured stable endpoint (same repo, never another host).
pub fn endpoint_for(setup: &Setup, ch: Channel) -> Option<String> {
    let repo = repo_of(&setup.stable_endpoint)?;
    Some(match ch {
        Channel::Stable => format!("https://github.com/{repo}{STABLE_SUFFIX}"),
        Channel::Beta => format!("https://github.com/{repo}{BETA_SUFFIX}"),
    })
}

/// `X.Y.Z` or `X.Y.Z-pre.1` (what `latest.json` may carry); anything else is shown as "?".
pub fn valid_version(v: &str) -> bool {
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let nums: Vec<&str> = core.split('.').collect();
    nums.len() == 3
        && nums.iter().all(|n| !n.is_empty() && n.len() <= 6 && n.chars().all(|c| c.is_ascii_digit()))
        && pre.is_none_or(|p| !p.is_empty() && p.len() <= 32 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'))
}

/// Release notes for display: keep newlines, drop other control/bidi/invisible chars, cap length.
pub fn clean_notes(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines().take(200) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&crate::directory::clean_line(line, NOTES_MAX));
        if out.chars().count() >= NOTES_MAX {
            break;
        }
    }
    let mut t: String = out.chars().take(NOTES_MAX).collect();
    if out.chars().count() > NOTES_MAX {
        t.push('…');
    }
    t
}

// ------------------------------------------------------------------------------------------------ runtime state

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Available {
    pub version: String,
    /// Release date (unix seconds) from `latest.json` `pub_date`.
    pub date: Option<i64>,
    pub notes: String,
}

/// What the shell shows (event `shell://update` + `update_status`).
#[derive(Serialize, Clone, Debug)]
pub struct UpdateDto {
    pub configured: bool,
    /// Why not configured (Vietnamese), when `configured` is false.
    pub reason: Option<String>,
    pub repo: Option<String>,
    pub channel: Channel,
    pub auto_check: bool,
    pub current: String,
    pub checking: bool,
    pub available: Option<Available>,
    pub downloading: bool,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Downloaded + installed; waiting for the restart.
    pub ready: bool,
    pub error: Option<String>,
    pub last_checked: Option<u64>,
}

#[derive(Default)]
struct State {
    checking: bool,
    available: Option<Available>,
    pending: Option<tauri_plugin_updater::Update>,
    downloading: bool,
    downloaded: u64,
    total: Option<u64>,
    ready: bool,
    error: Option<String>,
    last_checked: Option<u64>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn setup(app: &AppHandle) -> Result<Setup, &'static str> {
    setup_from_config(app.config().plugins.0.get("updater"))
}

fn prefs(app: &AppHandle) -> (Channel, bool) {
    let core = app.state::<crate::core::Core>();
    let g = core.lock();
    (Channel::parse(&g.reg.settings.update_channel).unwrap_or(Channel::Stable), g.reg.settings.auto_update_check)
}

pub fn status(app: &AppHandle) -> UpdateDto {
    let s = setup(app);
    let (channel, auto_check) = prefs(app);
    with_state(|st| UpdateDto {
        configured: s.is_ok(),
        reason: s.as_ref().err().map(|r| r.to_string()),
        repo: s.as_ref().ok().and_then(|s| repo_of(&s.stable_endpoint)),
        channel,
        auto_check,
        current: app.package_info().version.to_string(),
        checking: st.checking,
        available: st.available.clone(),
        downloading: st.downloading,
        downloaded: st.downloaded,
        total: st.total,
        ready: st.ready,
        error: st.error.clone(),
        last_checked: st.last_checked,
    })
}

pub fn emit(app: &AppHandle) {
    let _ = app.emit_to(EventTarget::webview(crate::webviews::SHELL_LABEL), "shell://update", status(app));
}

/// Ask the channel's `latest.json` whether a newer version exists. Never downloads.
pub async fn check(app: &AppHandle) -> Result<UpdateDto, String> {
    use tauri_plugin_updater::UpdaterExt;
    let s = setup(app).map_err(|r| format!("Không kiểm tra được cập nhật: {r}"))?;
    let (channel, _) = prefs(app);
    let busy = with_state(|st| {
        let b = st.checking || st.downloading;
        if !b {
            st.checking = true;
            st.error = None;
        }
        b
    });
    if busy {
        return Ok(status(app));
    }
    emit(app);
    let endpoint = endpoint_for(&s, channel).ok_or("địa chỉ cập nhật không hợp lệ")?;
    log::info!("kiểm tra cập nhật kênh {} ({endpoint})", channel.as_str());
    let result = async {
        let url = url::Url::parse(&endpoint).map_err(|e| e.to_string())?;
        let updater = app
            .updater_builder()
            .endpoints(vec![url])
            .map_err(|e| e.to_string())?
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        updater.check().await.map_err(|e| e.to_string())
    }
    .await;
    with_state(|st| {
        st.checking = false;
        st.last_checked = Some(now_s());
        match result {
            Ok(Some(u)) => {
                let version = if valid_version(&u.version) { u.version.clone() } else { "?".into() };
                st.available = Some(Available {
                    version,
                    date: u.date.map(|d| d.unix_timestamp()),
                    notes: clean_notes(u.body.as_deref().unwrap_or("")),
                });
                st.pending = Some(u);
            }
            Ok(None) => {
                st.available = None;
                st.pending = None;
            }
            Err(e) => {
                log::warn!("kiểm tra cập nhật lỗi: {e}");
                st.error = Some(format!("Không kiểm tra được cập nhật: {e}"));
            }
        }
    });
    emit(app);
    Ok(status(app))
}

/// Download + verify (ed25519) + install the pending update, then restart. Only after the user pressed "Cập nhật".
pub async fn install(app: &AppHandle, version: &str) -> Result<(), String> {
    let pending = with_state(|st| {
        if st.downloading {
            return Err("Đang tải bản cập nhật".to_string());
        }
        match &st.pending {
            Some(u) if u.version == version => {
                st.downloading = true;
                st.downloaded = 0;
                st.total = None;
                st.error = None;
                Ok(u.clone())
            }
            _ => Err("Không có bản cập nhật này — bấm “Kiểm tra cập nhật” lại".to_string()),
        }
    })?;
    emit(app);
    let a2 = app.clone();
    let mut last_emit = 0u64;
    let r = pending
        .download_and_install(
            move |chunk, total| {
                let n = with_state(|st| {
                    st.downloaded += chunk as u64;
                    st.total = total;
                    st.downloaded
                });
                if n - last_emit > 256 * 1024 {
                    last_emit = n;
                    emit(&a2);
                }
            },
            || {},
        )
        .await;
    match r {
        Ok(()) => {
            with_state(|st| {
                st.downloading = false;
                st.ready = true;
            });
            emit(app);
            log::info!("đã cài bản cập nhật {version}; khởi động lại");
            crate::speech::stop_unless_active(None);
            app.restart();
        }
        Err(e) => {
            log::warn!("cài bản cập nhật lỗi: {e}");
            let msg = format!("Không cài được bản cập nhật: {e}");
            with_state(|st| {
                st.downloading = false;
                st.error = Some(msg.clone());
            });
            emit(app);
            Err(msg)
        }
    }
}

/// Daily background check (only when configured + the user left "tự kiểm tra" on). Only tells the shell; never installs.
pub fn start_background(app: AppHandle) {
    if let Err(r) = setup(&app) {
        log::info!("tự cập nhật tắt: {r}");
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if prefs(&app).1 {
                let _ = check(&app).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // `tauri signer generate` output shape (random test key, NOT used anywhere).
    fn test_pubkey() -> String {
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 42]);
        base64::engine::general_purpose::STANDARD
            .encode(format!("untrusted comment: minisign public key: 0707070707070707\n{key}\n"))
    }

    const EP: &str = "https://github.com/acme/cfharness-desktop/releases/latest/download/latest.json";

    #[test]
    fn placeholders_mean_unconfigured() {
        assert!(setup_from_config(None).is_err());
        let cfg = json!({"pubkey": PUBKEY_PLACEHOLDER, "endpoints": [EP]});
        assert!(setup_from_config(Some(&cfg)).is_err());
        let cfg = json!({"pubkey": test_pubkey(),
            "endpoints": ["https://github.com/OWNER/REPO/releases/latest/download/latest.json"]});
        assert!(setup_from_config(Some(&cfg)).is_err());
        let cfg = json!({"pubkey": test_pubkey(), "endpoints": []});
        assert!(setup_from_config(Some(&cfg)).is_err());
        let cfg = json!({"pubkey": test_pubkey(), "endpoints": [EP]});
        assert_eq!(setup_from_config(Some(&cfg)).unwrap().stable_endpoint, EP);
    }

    #[test]
    fn repo_config_checked_against_placeholder() {
        // The placeholder that ships in tauri.conf.json must stay "unconfigured".
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let up = conf.pointer("/plugins/updater").expect("plugins.updater in tauri.conf.json");
        assert!(setup_from_config(Some(up)).is_err());
        assert_eq!(up["pubkey"], PUBKEY_PLACEHOLDER);
        assert!(conf.pointer("/bundle/createUpdaterArtifacts").is_none_or(|v| v == false));
    }

    #[test]
    fn pubkey_shape() {
        assert!(valid_pubkey(&test_pubkey()));
        // Real `tauri signer generate` output (throwaway key pair; the private half was never kept).
        assert!(valid_pubkey("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEZFNDc0NEQwNzIxMTJGMUEKUldRYUx4RnkwRVJIL2hKQ0FPOVVXSHRaeFlSVmV1WWw0em1yWEJoSjRiU3AvUUxTbHRPdE1sd00K"));
        assert!(!valid_pubkey("bm90IGEga2V5"));
        assert!(!valid_pubkey("%%%"));
        let short = base64::engine::general_purpose::STANDARD.encode("untrusted comment: x\nRWQ=\n");
        assert!(!valid_pubkey(&short));
    }

    #[test]
    fn endpoint_only_github_release_of_one_repo() {
        assert_eq!(repo_of(EP).as_deref(), Some("acme/cfharness-desktop"));
        for bad in [
            "http://github.com/acme/app/releases/latest/download/latest.json",
            "https://evil.com/acme/app/releases/latest/download/latest.json",
            "https://github.com:8443/acme/app/releases/latest/download/latest.json",
            "https://x@github.com/acme/app/releases/latest/download/latest.json",
            "https://github.com/acme/app/releases/latest/download/latest.json?x=1",
            "https://github.com/acme/app/extra/releases/latest/download/latest.json",
            "https://github.com/acme/../releases/latest/download/latest.json",
            "https://github.com/acme/app/releases/download/v1/latest.json",
        ] {
            assert_eq!(repo_of(bad), None, "{bad}");
        }
        let s = Setup { stable_endpoint: EP.into() };
        assert_eq!(endpoint_for(&s, Channel::Stable).unwrap(), EP);
        assert_eq!(
            endpoint_for(&s, Channel::Beta).unwrap(),
            "https://github.com/acme/cfharness-desktop/releases/download/beta/latest.json"
        );
    }

    #[test]
    fn versions_and_channels() {
        for ok in ["0.1.0", "1.20.3", "0.2.0-beta.1"] {
            assert!(valid_version(ok), "{ok}");
        }
        for bad in ["", "1.2", "v1.2.3", "1.2.3-", "1.2.3-<b>", "1.2.3.4", "1.2.x"] {
            assert!(!valid_version(bad), "{bad}");
        }
        assert_eq!(Channel::parse("beta"), Some(Channel::Beta));
        assert_eq!(Channel::parse("nightly"), None);
    }

    #[test]
    fn notes_are_cleaned_and_capped() {
        let n = clean_notes("## v0.2.0\n- Sửa lỗi\u{202e}x\u{0007}\n");
        assert_eq!(n, "## v0.2.0\n- Sửa lỗix");
        let long = "a".repeat(NOTES_MAX + 50);
        assert!(clean_notes(&long).chars().count() <= NOTES_MAX + 1);
    }
}
