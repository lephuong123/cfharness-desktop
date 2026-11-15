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
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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

impl Default for Settings {
    fn default() -> Self {
        Settings { live_webviews: 5, notify: true, autostart: false, badge_poll_s: 10, extra: Map::new() }
    }
}

impl Settings {
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

fn is_hex_color(s: &str) -> bool {
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
}
