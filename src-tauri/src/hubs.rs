//! App registry `hubs.json` (PLAN §4).
//!
//! Lives in the Tauri app data dir (`io.cfharness.desktop`), written atomically with mode 0600 on unix.
//! Unknown fields are preserved (`extra`) so an older app never drops data written by a newer one; a file with
//! a newer schema `version` is loaded read-only and never overwritten.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Current schema version of `hubs.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// Palette for the rail (picked deterministically from the hub id).
pub const PALETTE: [&str; 8] = ["#d97757", "#5b8def", "#3fa47a", "#b763d1", "#e0a43a", "#4fb3bf", "#e05d7c", "#7a8699"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HubKind {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hub {
    pub id: String,
    pub kind: HubKind,
    #[serde(default)]
    pub name: String,
    /// HUB_HOME (local hubs only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// Web origin (remote hubs; for local hubs it is learned at runtime from `cfharness web --json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default)]
    pub color: String,
    /// Last path (+query) visited inside the hub. Never contains a fragment or a login token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_url: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_id: Option<String>,
    #[serde(default)]
    pub stale: bool,
    /// Name last published by the directory (remote hubs): the display `name` follows it until the user renames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir_name: Option<String>,
    /// OS user owning the hub on the cloud machine (from the directory, display only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// An origin change proposed by the directory that the user refused: not asked again for this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_origin: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Hub {
    /// A remote hub with defaults filled in (color from id, name from origin when empty).
    pub fn new_remote(id: &str, name: &str, origin: &str) -> Hub {
        let mut h = Hub {
            id: id.to_string(),
            kind: HubKind::Remote,
            name: name.to_string(),
            home: None,
            origin: Some(origin.to_string()),
            color: color_for(id).to_string(),
            last_url: None,
            hidden: false,
            directory: None,
            hub_id: None,
            stale: false,
            dir_name: None,
            user: None,
            rejected_origin: None,
            extra: Map::new(),
        };
        if h.name.trim().is_empty() {
            h.name = default_name(&h);
        }
        h
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Directory {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced_at: Option<String>,
    /// Directory hub ids (`hubs[].id` of the directory) the user removed: a resync does not add them back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismissed: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_live")]
    pub live_webviews: usize,
    #[serde(default = "default_true")]
    pub notify: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "default_poll")]
    pub badge_poll_s: u64,
    /// App self-update channel: "stable" | "beta" (`updater::Channel`).
    #[serde(default = "default_channel")]
    pub update_channel: String,
    /// Check for app updates once a day (never installs without the user).
    #[serde(default = "default_true")]
    pub auto_update_check: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_live() -> usize {
    5
}
fn default_true() -> bool {
    true
}
fn default_poll() -> u64 {
    10
}
fn default_channel() -> String {
    "stable".into()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            live_webviews: 5,
            notify: true,
            autostart: false,
            badge_poll_s: 10,
            update_channel: default_channel(),
            auto_update_check: true,
            extra: Map::new(),
        }
    }
}

/// Values accepted from the settings screen (`set_settings`).
pub const LIVE_MIN: usize = 1;
pub const LIVE_MAX: usize = 20;

impl Settings {
    /// Validate + apply the settings screen values (unknown keys in `extra` are kept).
    pub fn update(
        &mut self,
        live_webviews: u64,
        notify: bool,
        badge_poll_s: u64,
        autostart: bool,
    ) -> Result<(), String> {
        if !(LIVE_MIN as u64..=LIVE_MAX as u64).contains(&live_webviews) {
            return Err(format!("Số WebView giữ sống phải từ {LIVE_MIN} đến {LIVE_MAX}"));
        }
        if !(crate::bridge::BADGE_POLL_MIN_S..=crate::bridge::BADGE_POLL_MAX_S).contains(&badge_poll_s) {
            return Err(format!(
                "Chu kỳ cập nhật badge phải từ {} đến {} giây",
                crate::bridge::BADGE_POLL_MIN_S,
                crate::bridge::BADGE_POLL_MAX_S
            ));
        }
        self.live_webviews = live_webviews as usize;
        self.notify = notify;
        self.badge_poll_s = badge_poll_s;
        self.autostart = autostart;
        Ok(())
    }

    /// `live_webviews` clamped to a sane range (at least the active hub stays alive).
    pub fn live_limit(&self) -> usize {
        self.live_webviews.clamp(1, 20)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    #[serde(default)]
    pub hubs: Vec<Hub>,
    #[serde(default)]
    pub directories: Vec<Directory>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            version: SCHEMA_VERSION,
            active: None,
            hubs: vec![],
            directories: vec![],
            settings: Settings::default(),
            extra: Map::new(),
        }
    }
}

/// Outcome of loading the registry file.
#[derive(Debug)]
pub struct Loaded {
    pub registry: Registry,
    /// The file was written by a newer app (schema > ours): keep it untouched.
    pub read_only: bool,
    /// A corrupt file was moved aside to this path.
    pub backup: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    Invalid(String),
}

/// Parse + migrate a registry document. Returns `(registry, read_only)`.
pub fn parse(text: &str) -> Result<(Registry, bool), ParseError> {
    let mut raw: Value = serde_json::from_str(text).map_err(|e| ParseError::Invalid(e.to_string()))?;
    let obj = raw.as_object_mut().ok_or_else(|| ParseError::Invalid("not an object".into()))?;
    let version = obj.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    if version > SCHEMA_VERSION {
        let reg: Registry = serde_json::from_value(raw).map_err(|e| ParseError::Invalid(e.to_string()))?;
        return Ok((reg, true));
    }
    if version == 0 {
        migrate_v0(obj);
    }
    let mut reg: Registry = serde_json::from_value(raw).map_err(|e| ParseError::Invalid(e.to_string()))?;
    reg.version = SCHEMA_VERSION;
    normalize(&mut reg);
    Ok((reg, false))
}

/// v0 (pre-release drafts): `hubs` could be a bare list of HUB_HOME strings and `kind` could be missing.
fn migrate_v0(obj: &mut Map<String, Value>) {
    if let Some(Value::Array(hubs)) = obj.get_mut("hubs") {
        let mut out = Vec::with_capacity(hubs.len());
        for h in hubs.drain(..) {
            match h {
                Value::String(home) => {
                    let id = local_id(&home);
                    out.push(serde_json::json!({"id": id, "kind": "local", "home": home}));
                }
                Value::Object(mut m) => {
                    if !m.contains_key("kind") {
                        let kind = if m.contains_key("origin") && !m.contains_key("home") { "remote" } else { "local" };
                        m.insert("kind".into(), Value::String(kind.into()));
                    }
                    if !m.contains_key("id") {
                        let id = match (m.get("home").and_then(Value::as_str), m.get("origin").and_then(Value::as_str))
                        {
                            (Some(home), _) => local_id(home),
                            (None, Some(origin)) => format!("rem-{}", short_hash(origin)),
                            _ => continue,
                        };
                        m.insert("id".into(), Value::String(id));
                    }
                    out.push(Value::Object(m));
                }
                _ => {}
            }
        }
        *hubs = out;
    }
}

/// Fill derived defaults; drop duplicates (same id).
pub fn normalize(reg: &mut Registry) {
    let mut seen = std::collections::HashSet::new();
    reg.hubs.retain(|h| !h.id.is_empty() && seen.insert(h.id.clone()));
    for h in reg.hubs.iter_mut() {
        if h.color.is_empty() || !is_hex_color(&h.color) {
            h.color = color_for(&h.id).to_string();
        }
        if h.name.trim().is_empty() {
            h.name = default_name(h);
        }
    }
    if let Some(a) = &reg.active {
        if !reg.hubs.iter().any(|h| &h.id == a) {
            reg.active = None;
        }
    }
}

pub fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn default_name(h: &Hub) -> String {
    match (&h.home, &h.origin) {
        (Some(home), _) => local_display_name(home),
        (None, Some(o)) => o.trim_start_matches("https://").trim_start_matches("http://").to_string(),
        _ => h.id.clone(),
    }
}

/// "Local" for the default home, otherwise "Local (<basename>)".
pub fn local_display_name(home: &str) -> String {
    let p = Path::new(home);
    let base = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if base == ".cfharness" || base.is_empty() {
        "Local".to_string()
    } else {
        format!("Local ({base})")
    }
}

pub fn short_hash(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// Stable id of a local hub derived from its HUB_HOME.
pub fn local_id(home: &str) -> String {
    format!("loc-{}", short_hash(home.trim_end_matches(['/', '\\'])))
}

pub fn color_for(id: &str) -> &'static str {
    let d = Sha256::digest(id.as_bytes());
    PALETTE[(d[0] as usize) % PALETTE.len()]
}

/// 16-byte WebKit data store identifier for a hub (macOS 14+): one isolated cookie/storage jar per hub.
pub fn data_store_id(hub_id: &str) -> [u8; 16] {
    let d = Sha256::digest(format!("cfharness-desktop/webview/{hub_id}").as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
}

impl Registry {
    pub fn get(&self, id: &str) -> Option<&Hub> {
        self.hubs.iter().find(|h| h.id == id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Hub> {
        self.hubs.iter_mut().find(|h| h.id == id)
    }

    pub fn directory(&self, id: &str) -> Option<&Directory> {
        self.directories.iter().find(|d| d.id == id)
    }
    pub fn directory_mut(&mut self, id: &str) -> Option<&mut Directory> {
        self.directories.iter_mut().find(|d| d.id == id)
    }

    /// Group key used by the rail: the directory id when that directory still exists, else none.
    pub fn group_of(&self, h: &Hub) -> Option<String> {
        h.directory.as_ref().filter(|d| self.directory(d).is_some()).cloned()
    }

    /// Rail order: hubs without a directory first (registry order), then each directory in `directories` order.
    /// `include_hidden=false` gives the visible order used for Cmd/Ctrl+1…9 and previous/next.
    pub fn rail_order(&self, include_hidden: bool) -> Vec<String> {
        let keep = |h: &&Hub| include_hidden || !h.hidden;
        let mut out: Vec<String> =
            self.hubs.iter().filter(keep).filter(|h| self.group_of(h).is_none()).map(|h| h.id.clone()).collect();
        for d in &self.directories {
            out.extend(
                self.hubs.iter().filter(keep).filter(|h| h.directory.as_deref() == Some(&d.id)).map(|h| h.id.clone()),
            );
        }
        out
    }

    /// Hub `offset` steps away from `from` in the visible rail order (wraps around).
    pub fn neighbour(&self, from: Option<&str>, offset: i64) -> Option<String> {
        let order = self.rail_order(false);
        if order.is_empty() {
            return None;
        }
        let n = order.len() as i64;
        let idx = from.and_then(|f| order.iter().position(|x| x == f)).map(|i| i as i64).unwrap_or(if offset > 0 {
            -1
        } else {
            0
        });
        Some(order[(idx + offset).rem_euclid(n) as usize].clone())
    }

    /// Rename (one line, ≤ 60 chars, control/bidi characters removed). Empty → back to the default name.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        let clean = crate::directory::clean_line(name, crate::directory::NAME_MAX);
        let h = self.get_mut(id).ok_or("Không tìm thấy hub")?;
        h.name = if clean.is_empty() { h.dir_name.clone().unwrap_or_else(|| default_name(h)) } else { clean };
        Ok(())
    }

    pub fn set_color(&mut self, id: &str, color: &str) -> Result<(), String> {
        if !is_hex_color(color) {
            return Err("Màu không hợp lệ".into());
        }
        self.get_mut(id).ok_or("Không tìm thấy hub")?.color = color.to_ascii_lowercase();
        Ok(())
    }

    /// Hide/show. Hiding the active hub moves the selection to the next visible hub.
    pub fn set_hidden(&mut self, id: &str, hidden: bool) -> Result<(), String> {
        self.get_mut(id).ok_or("Không tìm thấy hub")?.hidden = hidden;
        if hidden && self.active.as_deref() == Some(id) {
            self.active = self.rail_order(false).into_iter().next();
        }
        Ok(())
    }

    /// Remove a REMOTE hub (local hubs are rediscovered automatically — they can only be hidden). A hub that came
    /// from a directory is remembered in `dismissed` so a resync does not bring it back.
    pub fn remove(&mut self, id: &str) -> Result<Hub, String> {
        let pos = self.hubs.iter().position(|h| h.id == id).ok_or("Không tìm thấy hub")?;
        if self.hubs[pos].kind == HubKind::Local {
            return Err("Hub local được phát hiện tự động — chỉ ẩn được, không xoá được".into());
        }
        let h = self.hubs.remove(pos);
        if let (Some(dir), Some(hid)) = (h.directory.clone(), h.hub_id.clone()) {
            if let Some(d) = self.directory_mut(&dir) {
                if !d.dismissed.contains(&hid) {
                    d.dismissed.push(hid);
                }
            }
        }
        if self.active.as_deref() == Some(id) {
            self.active = self.rail_order(false).into_iter().next();
        }
        Ok(h)
    }

    /// Move a hub one step up (`delta < 0`) or down within its rail group (swap with the neighbour of the same group).
    /// Returns false when it is already at the edge.
    pub fn move_in_group(&mut self, id: &str, delta: i32) -> Result<bool, String> {
        let h = self.get(id).ok_or("Không tìm thấy hub")?;
        let group = self.group_of(h);
        let same: Vec<usize> = (0..self.hubs.len()).filter(|&i| self.group_of(&self.hubs[i]) == group).collect();
        let me = same.iter().position(|&i| self.hubs[i].id == id).ok_or("Không tìm thấy hub")?;
        let other = if delta < 0 { me.checked_sub(1) } else { Some(me + 1).filter(|&j| j < same.len()) };
        match other {
            Some(j) => {
                self.hubs.swap(same[me], same[j]);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Merge discovered local homes: add new ones, keep user customisations of existing ones.
    /// Returns true if something changed.
    pub fn merge_local(&mut self, homes: &[PathBuf]) -> bool {
        let mut changed = false;
        for home in homes {
            let home_s = home.to_string_lossy().to_string();
            let id = local_id(&home_s);
            if self.get(&id).is_some() {
                continue;
            }
            let mut hub = Hub {
                id: id.clone(),
                kind: HubKind::Local,
                name: String::new(),
                home: Some(home_s),
                origin: None,
                color: String::new(),
                last_url: None,
                hidden: false,
                directory: None,
                hub_id: None,
                stale: false,
                dir_name: None,
                user: None,
                rejected_origin: None,
                extra: Map::new(),
            };
            hub.color = color_for(&id).to_string();
            hub.name = default_name(&hub);
            self.hubs.push(hub);
            changed = true;
        }
        if self.active.is_none() {
            if let Some(h) = self.hubs.iter().find(|h| !h.hidden) {
                self.active = Some(h.id.clone());
                changed = true;
            }
        }
        changed
    }
}

/// Load `hubs.json`; a missing file yields an empty registry, a corrupt one is moved aside.
pub fn load(path: &Path) -> Loaded {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Loaded { registry: Registry::default(), read_only: false, backup: None },
    };
    match parse(&text) {
        Ok((registry, read_only)) => Loaded { registry, read_only, backup: None },
        Err(ParseError::Invalid(e)) => {
            log::warn!("hubs.json hỏng ({e}); chuyển sang bản sao lưu và tạo mới");
            let ts =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let bak = path.with_file_name(format!("hubs.json.corrupt-{ts}"));
            let backup = fs::rename(path, &bak).ok().map(|_| bak);
            Loaded { registry: Registry::default(), read_only: false, backup }
        }
    }
}

/// Atomic write (temp file in the same dir + fsync + rename), 0600 on unix, parent dir 0700.
pub fn save(path: &Path, reg: &Registry) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent dir"))?;
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let data = serde_json::to_vec_pretty(reg).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!(".hubs.json.{}.tmp", std::process::id()));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&data)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Clean a URL into a `last_url` value: path + query only, never a fragment (`#k=` session key) and never a
/// login link (`/login?t=` one-time token). Returns `None` when the URL must not be remembered.
pub fn last_url_of(url: &url::Url) -> Option<String> {
    if url.path() == "/login" || url.path().starts_with("/login/") || url.path() == "/enroll" {
        return None;
    }
    if url.query_pairs().any(|(k, _)| k == "t") {
        return None;
    }
    let mut s = url.path().to_string();
    if let Some(q) = url.query() {
        s.push('?');
        s.push_str(q);
    }
    if s.len() > 2048 {
        return None;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_example() {
        let text = r##"{
          "version": 1, "active": "loc-3f2a",
          "hubs": [
            {"id": "loc-3f2a", "kind": "local", "name": "Local", "home": "/Users/x/.cfharness", "color": "#d97757",
             "last_url": "/manager", "hidden": false},
            {"id": "rem-a1b2c3d4", "kind": "remote", "name": "An", "origin": "https://an.example.com",
             "directory": "dir-01", "hub_id": "a1b2c3d4", "last_url": "/", "stale": false}
          ],
          "directories": [{"id": "dir-01", "name": "Mac mini", "url": "https://admin.example.com/x",
            "token_ref": "keychain:cfharness-desktop/dir-01", "etag": "e", "synced_at": "t"}],
          "settings": {"live_webviews": 3, "notify": true, "autostart": false, "badge_poll_s": 10}
        }"##;
        let (reg, ro) = parse(text).unwrap();
        assert!(!ro);
        assert_eq!(reg.hubs.len(), 2);
        assert_eq!(reg.hubs[1].kind, HubKind::Remote);
        assert_eq!(reg.settings.live_webviews, 3);
        assert_eq!(reg.active.as_deref(), Some("loc-3f2a"));
        assert_eq!(reg.directories[0].token_ref.as_deref(), Some("keychain:cfharness-desktop/dir-01"));
    }

    #[test]
    fn migrate_v0_bare_list() {
        let (reg, ro) = parse(r#"{"hubs": ["/Users/x/.cfharness", {"origin": "https://a.example.com"}]}"#).unwrap();
        assert!(!ro);
        assert_eq!(reg.version, SCHEMA_VERSION);
        assert_eq!(reg.hubs.len(), 2);
        assert_eq!(reg.hubs[0].kind, HubKind::Local);
        assert_eq!(reg.hubs[0].id, local_id("/Users/x/.cfharness"));
        assert_eq!(reg.hubs[0].name, "Local");
        assert_eq!(reg.hubs[1].kind, HubKind::Remote);
        assert!(reg.hubs[1].id.starts_with("rem-"));
        assert!(is_hex_color(&reg.hubs[0].color));
        assert_eq!(reg.settings, Settings::default());
    }

    #[test]
    fn newer_schema_is_read_only_and_keeps_unknown_fields() {
        let text = r#"{"version": 9, "future": {"x": 1}, "hubs": [{"id": "loc-1", "kind": "local", "zzz": true}]}"#;
        let (reg, ro) = parse(text).unwrap();
        assert!(ro);
        assert_eq!(reg.version, 9);
        let out = serde_json::to_value(&reg).unwrap();
        assert_eq!(out["future"]["x"], 1);
        assert_eq!(out["hubs"][0]["zzz"], true);
    }

    #[test]
    fn unknown_fields_round_trip_v1() {
        let text = r##"{"version": 1, "x_new": 5, "hubs": [{"id": "loc-1", "kind": "local", "name": "A", "color": "#000000", "pin": 1}]}"##;
        let (reg, _) = parse(text).unwrap();
        let out = serde_json::to_value(&reg).unwrap();
        assert_eq!(out["x_new"], 5);
        assert_eq!(out["hubs"][0]["pin"], 1);
    }

    #[test]
    fn normalize_drops_duplicates_and_bad_active() {
        let text = r#"{"version": 1, "active": "gone", "hubs": [
            {"id": "a", "kind": "local", "home": "/h/a"}, {"id": "a", "kind": "local", "home": "/h/b"}]}"#;
        let (reg, _) = parse(text).unwrap();
        assert_eq!(reg.hubs.len(), 1);
        assert_eq!(reg.active, None);
        assert_eq!(reg.hubs[0].name, "Local (a)");
    }

    #[test]
    fn invalid_documents() {
        assert!(parse("[]").is_err());
        assert!(parse("{nope").is_err());
        assert!(parse(r#"{"version":1,"hubs":[{"id":"x","kind":"weird"}]}"#).is_err());
    }

    #[test]
    fn save_load_round_trip_atomic_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("hubs.json");
        let mut reg = Registry::default();
        assert!(reg.merge_local(&[PathBuf::from("/Users/x/.cfharness"), PathBuf::from("/Users/x/hub2")]));
        assert!(!reg.merge_local(&[PathBuf::from("/Users/x/.cfharness")]));
        save(&path, &reg).unwrap();
        let loaded = load(&path);
        assert!(!loaded.read_only);
        assert_eq!(loaded.registry, reg);
        assert_eq!(loaded.registry.active, Some(local_id("/Users/x/.cfharness")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // no temp files left behind
        let names: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hubs.json");
        fs::write(&path, "{broken").unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.registry, Registry::default());
        assert!(loaded.backup.as_ref().unwrap().exists());
        assert!(!path.exists());
    }

    #[test]
    fn last_url_never_keeps_tokens_or_fragments() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert_eq!(
            last_url_of(&u("http://cfharness.localhost:8711/manager?w=3#k=secret")).as_deref(),
            Some("/manager?w=3")
        );
        assert_eq!(last_url_of(&u("http://cfharness.localhost:8711/login?t=abc")), None);
        assert_eq!(last_url_of(&u("http://cfharness.localhost:8711/login")), None);
        assert_eq!(last_url_of(&u("http://cfharness.localhost:8711/x?t=abc")), None);
        assert_eq!(last_url_of(&u("http://cfharness.localhost:8711/")).as_deref(), Some("/"));
    }

    #[test]
    fn ids_and_store_ids_are_stable() {
        assert_eq!(local_id("/a/b/"), local_id("/a/b"));
        assert_ne!(data_store_id("loc-1"), data_store_id("loc-2"));
        assert_eq!(data_store_id("loc-1"), data_store_id("loc-1"));
        assert_eq!(local_display_name("/Users/x/hub-thu-2"), "Local (hub-thu-2)");
    }

    fn mixed() -> Registry {
        let mut reg = Registry::default();
        reg.merge_local(&[PathBuf::from("/Users/x/.cfharness"), PathBuf::from("/Users/x/hub2")]);
        reg.directories.push(Directory {
            id: "dir-1".into(),
            name: "D".into(),
            url: "https://admin.example.com/d".into(),
            token_ref: None,
            etag: None,
            synced_at: None,
            dismissed: vec![],
            extra: Map::new(),
        });
        let mut a = Hub::new_remote("rem-1-a", "A", "https://a.example.com");
        a.directory = Some("dir-1".into());
        a.hub_id = Some("a".into());
        let mut b = Hub::new_remote("rem-1-b", "B", "https://b.example.com");
        b.directory = Some("dir-1".into());
        b.hub_id = Some("b".into());
        // interleaved on purpose: the rail still groups them
        reg.hubs.insert(1, a);
        reg.hubs.push(b);
        reg.hubs.push(Hub::new_remote("rem-solo", "", "https://solo.example.com"));
        reg
    }

    #[test]
    fn rail_groups_and_neighbours() {
        let reg = mixed();
        let l1 = local_id("/Users/x/.cfharness");
        let l2 = local_id("/Users/x/hub2");
        assert_eq!(
            reg.rail_order(false),
            vec![l1.clone(), l2.clone(), "rem-solo".into(), "rem-1-a".into(), "rem-1-b".into()]
        );
        assert_eq!(reg.get("rem-solo").unwrap().name, "solo.example.com");
        assert_eq!(reg.neighbour(Some(&l1), 1).as_deref(), Some(l2.as_str()));
        assert_eq!(reg.neighbour(Some(&l1), -1).as_deref(), Some("rem-1-b")); // wraps
        assert_eq!(reg.neighbour(Some("rem-1-b"), 1).as_deref(), Some(l1.as_str()));
        assert_eq!(reg.neighbour(None, 1).as_deref(), Some(l1.as_str()));
        // a directory that no longer exists → its hubs are shown ungrouped
        let mut r2 = reg.clone();
        r2.directories.clear();
        assert_eq!(r2.group_of(r2.get("rem-1-a").unwrap()), None);
    }

    #[test]
    fn rename_color_hide() {
        let mut reg = mixed();
        reg.rename("rem-solo", "  Hub\n của\u{202E} tôi  ").unwrap();
        assert_eq!(reg.get("rem-solo").unwrap().name, "Hub của tôi");
        reg.rename("rem-solo", "").unwrap();
        assert_eq!(reg.get("rem-solo").unwrap().name, "solo.example.com");
        assert!(reg.rename("nope", "x").is_err());
        reg.set_color("rem-solo", "#ABCDEF").unwrap();
        assert_eq!(reg.get("rem-solo").unwrap().color, "#abcdef");
        assert!(reg.set_color("rem-solo", "red").is_err());
        assert!(reg.set_color("rem-solo", "#12345").is_err());
        let l1 = local_id("/Users/x/.cfharness");
        assert_eq!(reg.active.as_deref(), Some(l1.as_str()));
        reg.set_hidden(&l1, true).unwrap();
        assert_ne!(reg.active.as_deref(), Some(l1.as_str()));
        assert!(!reg.rail_order(false).contains(&l1));
        assert!(reg.rail_order(true).contains(&l1));
        reg.set_hidden(&l1, false).unwrap();
        assert!(reg.rail_order(false).contains(&l1));
    }

    #[test]
    fn remove_rules() {
        let mut reg = mixed();
        let l1 = local_id("/Users/x/.cfharness");
        assert!(reg.remove(&l1).is_err()); // local hubs can only be hidden
        reg.remove("rem-1-a").unwrap();
        assert_eq!(reg.directory("dir-1").unwrap().dismissed, vec!["a".to_string()]);
        reg.remove("rem-solo").unwrap();
        assert!(reg.get("rem-solo").is_none());
        assert!(reg.remove("rem-solo").is_err());
    }

    #[test]
    fn move_within_group_only() {
        let mut reg = mixed();
        let l1 = local_id("/Users/x/.cfharness");
        let l2 = local_id("/Users/x/hub2");
        assert!(!reg.move_in_group(&l1, -1).unwrap()); // already first
        assert!(reg.move_in_group(&l1, 1).unwrap());
        assert_eq!(reg.rail_order(false)[..3], [l2.clone(), l1.clone(), "rem-solo".to_string()]);
        assert!(reg.move_in_group("rem-1-b", -1).unwrap());
        assert_eq!(reg.rail_order(false)[3..], ["rem-1-b".to_string(), "rem-1-a".to_string()]);
        assert!(!reg.move_in_group("rem-1-b", -1).unwrap()); // first of its group even if not first overall
                                                             // round trip through save/load keeps the order
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hubs.json");
        save(&p, &reg).unwrap();
        assert_eq!(load(&p).registry.rail_order(true), reg.rail_order(true));
    }
}
