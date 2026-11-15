//! Shared app state. Rule: never call a webview/window method while holding `Core::inner` (many of them
//! round-trip to the main thread, which may itself be waiting on the lock inside a navigation callback).

use crate::hubs::{self, HubKind, Registry};
use crate::nav::Origin;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HubStatus {
    /// Not opened in this app session.
    Idle,
    /// Asking `cfharness web --json --ensure` / loading.
    Starting,
    /// A page of the hub is loaded.
    Online,
    /// Needs a login (remote hubs, or local re-login exhausted).
    Login,
    Error {
        message: String,
    },
}

pub struct Inner {
    pub reg: Registry,
    pub read_only: bool,
    pub status: HashMap<String, HubStatus>,
    /// Runtime origin of each hub (local hubs: from C1; may change when the web server restarts on another port).
    pub origins: HashMap<String, Origin>,
    /// Live hub webviews, least recently used first.
    pub lru: Vec<String>,
    pub zoom: HashMap<String, f64>,
    /// Timestamps of automatic re-logins per hub (loop guard).
    pub relogin: HashMap<String, Vec<Instant>>,
    /// Path to restore once the login link has been consumed.
    pub pending_after_login: HashMap<String, String>,
    /// Hubs with a C1 call in flight.
    pub starting: HashSet<String>,
    pub bin: Option<PathBuf>,
    pub find_open: bool,
}

pub struct Core {
    pub reg_path: PathBuf,
    pub data_dir: PathBuf,
    inner: Mutex<Inner>,
}

/// Automatic re-login budget: at most this many within `RELOGIN_WINDOW_S`.
pub const RELOGIN_MAX: usize = 3;
pub const RELOGIN_WINDOW_S: u64 = 120;

impl Core {
    pub fn new(data_dir: PathBuf) -> Core {
        let reg_path = data_dir.join("hubs.json");
        let loaded = hubs::load(&reg_path);
        if let Some(b) = &loaded.backup {
            log::warn!("hubs.json hỏng đã được chuyển sang {}", b.display());
        }
        if loaded.read_only {
            log::warn!("hubs.json do bản app mới hơn ghi (version {}); chỉ đọc", loaded.registry.version);
        }
        Core {
            reg_path,
            data_dir,
            inner: Mutex::new(Inner {
                reg: loaded.registry,
                read_only: loaded.read_only,
                status: HashMap::new(),
                origins: HashMap::new(),
                lru: vec![],
                zoom: HashMap::new(),
                relogin: HashMap::new(),
                pending_after_login: HashMap::new(),
                starting: HashSet::new(),
                bin: None,
                find_open: false,
            }),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Persist the registry (no-op when read-only). Call with the lock held — file I/O only.
    pub fn persist(&self, inner: &Inner) {
        if inner.read_only {
            return;
        }
        if let Err(e) = hubs::save(&self.reg_path, &inner.reg) {
            log::error!("không ghi được hubs.json: {e}");
        }
    }
}

impl Inner {
    pub fn status_of(&self, id: &str) -> HubStatus {
        self.status.get(id).cloned().unwrap_or(HubStatus::Idle)
    }

    /// Consume one automatic re-login slot; false when the budget is exhausted.
    pub fn take_relogin_slot(&mut self, id: &str, now: Instant) -> bool {
        let v = self.relogin.entry(id.to_string()).or_default();
        v.retain(|t| now.duration_since(*t).as_secs() < RELOGIN_WINDOW_S);
        if v.len() >= RELOGIN_MAX {
            return false;
        }
        v.push(now);
        true
    }

    /// Mark `id` as most recently used; returns hub ids to put to sleep to respect `limit`.
    pub fn touch_lru(&mut self, id: &str, limit: usize) -> Vec<String> {
        self.lru.retain(|x| x != id);
        self.lru.push(id.to_string());
        let mut evict = vec![];
        while self.lru.len() > limit.max(1) {
            let victim = self.lru.remove(0);
            evict.push(victim);
        }
        evict
    }

    pub fn is_local(&self, id: &str) -> bool {
        self.reg.get(id).map(|h| h.kind == HubKind::Local).unwrap_or(false)
    }
}

#[derive(Serialize, Clone)]
pub struct HubDto {
    pub id: String,
    pub name: String,
    pub kind: HubKind,
    pub color: String,
    pub initial: String,
    pub status: HubStatus,
    pub badge: Option<u32>,
    pub home: Option<String>,
    pub origin: Option<String>,
    pub hidden: bool,
}

#[derive(Serialize, Clone)]
pub struct ShellState {
    pub hubs: Vec<HubDto>,
    pub active: Option<String>,
    pub cfharness: Option<String>,
    pub read_only: bool,
    pub find_open: bool,
    pub version: String,
}

/// First letter for the rail tile ("Local (hub-thu-2)" → "H", "Local" → "L").
pub fn initial_of(name: &str) -> String {
    let base = name.strip_prefix("Local (").and_then(|r| r.strip_suffix(')')).unwrap_or(name);
    base.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".to_string())
}

impl Inner {
    pub fn shell_state(&self) -> ShellState {
        ShellState {
            hubs: self
                .reg
                .hubs
                .iter()
                .map(|h| HubDto {
                    id: h.id.clone(),
                    name: h.name.clone(),
                    kind: h.kind,
                    color: h.color.clone(),
                    initial: initial_of(&h.name),
                    status: self.status_of(&h.id),
                    badge: None,
                    home: h.home.clone(),
                    origin: self.origins.get(&h.id).map(|o| o.as_string()).or_else(|| h.origin.clone()),
                    hidden: h.hidden,
                })
                .collect(),
            active: self.reg.active.clone(),
            cfharness: self.bin.as_ref().map(|p| p.to_string_lossy().to_string()),
            read_only: self.read_only,
            find_open: self.find_open,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn inner() -> Inner {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::new(dir.path().to_path_buf());
        let g = core.lock();
        Inner {
            reg: g.reg.clone(),
            read_only: false,
            status: HashMap::new(),
            origins: HashMap::new(),
            lru: vec![],
            zoom: HashMap::new(),
            relogin: HashMap::new(),
            pending_after_login: HashMap::new(),
            starting: HashSet::new(),
            bin: None,
            find_open: false,
        }
    }

    #[test]
    fn lru_evicts_oldest() {
        let mut i = inner();
        assert!(i.touch_lru("a", 2).is_empty());
        assert!(i.touch_lru("b", 2).is_empty());
        assert!(i.touch_lru("a", 2).is_empty()); // a becomes most recent
        assert_eq!(i.touch_lru("c", 2), vec!["b".to_string()]);
        assert_eq!(i.lru, vec!["a", "c"]);
        assert_eq!(i.touch_lru("d", 0), vec!["a".to_string(), "c".to_string()]);
        // limit clamps to 1
    }

    #[test]
    fn relogin_budget() {
        let mut i = inner();
        let t0 = Instant::now();
        for _ in 0..RELOGIN_MAX {
            assert!(i.take_relogin_slot("a", t0));
        }
        assert!(!i.take_relogin_slot("a", t0));
        assert!(i.take_relogin_slot("b", t0));
        assert!(i.take_relogin_slot("a", t0 + Duration::from_secs(RELOGIN_WINDOW_S + 1)));
    }

    #[test]
    fn initials() {
        assert_eq!(initial_of("Local"), "L");
        assert_eq!(initial_of("Local (hub-thu-2)"), "H");
        assert_eq!(initial_of("  ánh"), "Á");
        assert_eq!(initial_of("--"), "?");
    }
}
