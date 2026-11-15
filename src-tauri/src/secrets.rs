//! Directory tokens live in the OS credential store (macOS Keychain / Windows Credential Manager), never in
//! `hubs.json` (PLAN §2 Q4, §5.5). `hubs.json` only keeps a `token_ref` pointing at the entry.
//!
//! The store is behind the `TokenStore` trait so tests (and `CFHARNESS_DESKTOP_TOKEN_STORE=memory` in dev) use an
//! in-memory fake instead of the real keychain.

use std::collections::HashMap;
use std::sync::Mutex;

/// Keychain service name (= app identifier).
pub const SERVICE: &str = "io.cfharness.desktop";
/// Dev/test only: `memory` → tokens are kept in RAM for the process lifetime.
pub const STORE_ENV: &str = "CFHARNESS_DESKTOP_TOKEN_STORE";

pub trait TokenStore: Send + Sync {
    fn set(&self, key: &str, secret: &str) -> Result<(), String>;
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// Keychain account of a directory token.
pub fn token_key(dir_id: &str) -> String {
    format!("directory:{dir_id}")
}

/// Value stored in `hubs.json` (`directories[].token_ref`).
pub fn token_ref(dir_id: &str) -> String {
    format!("keychain:{SERVICE}/{}", token_key(dir_id))
}

/// Keychain account named by a `token_ref`, only if it is one of ours.
pub fn key_of_ref(token_ref: &str) -> Option<String> {
    let key = token_ref.strip_prefix(&format!("keychain:{SERVICE}/"))?;
    let dir = key.strip_prefix("directory:")?;
    (!dir.is_empty() && dir.len() <= 64 && dir.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| key.to_string())
}

/// Real OS credential store via the `keyring` crate.
pub struct KeyringStore;

impl KeyringStore {
    fn entry(key: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, key).map_err(|e| format!("Không mở được kho bí mật của hệ điều hành: {e}"))
    }
}

impl TokenStore for KeyringStore {
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        Self::entry(key)?
            .set_password(secret)
            .map_err(|e| format!("Không lưu được token vào kho bí mật của hệ điều hành: {e}"))
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        match Self::entry(key)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Không đọc được token từ kho bí mật của hệ điều hành: {e}")),
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Không xoá được token khỏi kho bí mật của hệ điều hành: {e}")),
        }
    }
}

/// In-memory store (tests, dev).
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, String>>,
}

impl TokenStore for MemoryStore {
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.map.lock().unwrap_or_else(|p| p.into_inner()).insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.map.lock().unwrap_or_else(|p| p.into_inner()).get(key).cloned())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.map.lock().unwrap_or_else(|p| p.into_inner()).remove(key);
        Ok(())
    }
}

/// Store selected for this process.
pub fn default_store() -> Box<dyn TokenStore> {
    match std::env::var(STORE_ENV).as_deref() {
        Ok("memory") => {
            log::warn!("{STORE_ENV}=memory: token directory chỉ giữ trong RAM (chế độ dev)");
            Box::new(MemoryStore::default())
        }
        _ => Box::new(KeyringStore),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_round_trip_and_reject_foreign() {
        let r = token_ref("dir-1a2b3c4d");
        assert_eq!(r, "keychain:io.cfharness.desktop/directory:dir-1a2b3c4d");
        assert_eq!(key_of_ref(&r).as_deref(), Some("directory:dir-1a2b3c4d"));
        assert_eq!(key_of_ref("keychain:other.app/directory:dir-1"), None);
        assert_eq!(key_of_ref("keychain:io.cfharness.desktop/login:x"), None);
        assert_eq!(key_of_ref("keychain:io.cfharness.desktop/directory:../x"), None);
        assert_eq!(key_of_ref("file:/tmp/x"), None);
    }

    #[test]
    fn memory_store_behaves_like_a_keychain() {
        let s = MemoryStore::default();
        assert_eq!(s.get("k").unwrap(), None);
        s.set("k", "v1").unwrap();
        s.set("k", "v2").unwrap();
        assert_eq!(s.get("k").unwrap().as_deref(), Some("v2"));
        s.delete("k").unwrap();
        s.delete("k").unwrap(); // idempotent
        assert_eq!(s.get("k").unwrap(), None);
    }
}
