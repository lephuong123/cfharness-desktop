//! Deep links `cfharness://hub/<id>/<path>` (PLAN P3): open hub `<id>` (registry id like `loc-3f2a` / `rem-…`, or the
//! hub's own `hub_id` hex) at a RELATIVE path. Anything else is rejected: other hosts, missing id, absolute or
//! scheme-relative paths, backslashes, `/login` (one-time link), fragments (`#k=` session key), userinfo, ports.

use crate::core::Core;
use crate::webviews;
use tauri::{AppHandle, Manager};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepLink {
    pub hub: String,
    /// Starts with `/`, may carry a query; never a fragment.
    pub path: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn parse(s: &str) -> Option<DeepLink> {
    if s.len() > 2048 {
        return None;
    }
    // dot segments are normalised away by the URL parser: refuse them on the raw text
    let low = s.to_ascii_lowercase();
    if low.contains("/.") || low.contains("%2e") {
        return None;
    }
    let u = Url::parse(s.trim()).ok()?;
    if u.scheme() != "cfharness" || u.host_str() != Some("hub") {
        return None;
    }
    if !u.username().is_empty() || u.password().is_some() || u.port().is_some() || u.fragment().is_some() {
        return None;
    }
    // raw path: "/<id>[/rest]"
    let raw = u.path();
    let rest = raw.strip_prefix('/')?;
    let (id, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if !valid_id(id) {
        return None;
    }
    let tail = if tail.is_empty() { "/" } else { tail };
    if !tail.starts_with('/')
        || tail.starts_with("//")
        || tail.contains('\\')
        || tail.contains("%5C")
        || tail.contains("%5c")
    {
        return None;
    }
    let mut path = tail.to_string();
    if path == "/login" || path.starts_with("/login/") || path.starts_with("/login?") {
        return None;
    }
    if let Some(q) = u.query() {
        if path.starts_with("/login") {
            return None;
        }
        path.push('?');
        path.push_str(q);
    }
    Some(DeepLink { hub: id.to_string(), path })
}

/// Resolve the link's hub to a registry id (registry id first, then the hub's own `hub_id`).
pub fn resolve(reg: &crate::hubs::Registry, hub: &str) -> Option<String> {
    if reg.get(hub).is_some() {
        return Some(hub.to_string());
    }
    let lower = hub.to_ascii_lowercase();
    reg.hubs.iter().find(|h| h.hub_id.as_deref() == Some(lower.as_str())).map(|h| h.id.clone())
}

pub fn handle_urls(app: &AppHandle, urls: Vec<String>) {
    for s in urls {
        let Some(link) = parse(&s) else {
            log::warn!("deep link bị từ chối (sai dạng cfharness://hub/<id>/<đường dẫn>)");
            continue;
        };
        let id = {
            let core = app.state::<Core>();
            let g = core.lock();
            resolve(&g.reg, &link.hub)
        };
        let Some(id) = id else {
            log::warn!("deep link: không có hub {}", link.hub);
            webviews::show_main(app);
            continue;
        };
        let a = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = webviews::open_path(a, id.clone(), link.path).await {
                log::warn!("deep link: không mở được hub {id}: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<(String, String)> {
        parse(s).map(|d| (d.hub, d.path))
    }

    #[test]
    fn accepts_relative_paths() {
        assert_eq!(p("cfharness://hub/loc-3f2a/approvals"), Some(("loc-3f2a".into(), "/approvals".into())));
        assert_eq!(p("cfharness://hub/loc-3f2a"), Some(("loc-3f2a".into(), "/".into())));
        assert_eq!(p("cfharness://hub/loc-3f2a/"), Some(("loc-3f2a".into(), "/".into())));
        assert_eq!(p("cfharness://hub/a1b2c3d4/manager?w=3"), Some(("a1b2c3d4".into(), "/manager?w=3".into())));
        assert_eq!(p("CFHARNESS://hub/x/inbox"), Some(("x".into(), "/inbox".into())));
    }

    #[test]
    fn rejects_everything_else() {
        for s in [
            "cfharness://other/loc/approvals",
            "https://hub/loc/approvals",
            "cfharness://hub/",
            "cfharness://hub//evil.com/x",
            "cfharness://hub/loc//evil.com",
            "cfharness://hub/loc/a\\b",
            "cfharness://hub/loc/%5C%5Cevil",
            "cfharness://hub/loc/login?t=abc",
            "cfharness://hub/loc/login",
            "cfharness://hub/loc/x#k=secret",
            "cfharness://user@hub/loc/x",
            "cfharness://hub:99/loc/x",
            "cfharness://hub/lo c/x",
            "cfharness://hub/../x",
            "javascript:alert(1)",
        ] {
            assert_eq!(parse(s), None, "should reject {s}");
        }
    }

    #[test]
    fn resolves_registry_or_hub_id() {
        let mut reg = crate::hubs::Registry::default();
        let mut h = crate::hubs::Hub::new_remote("rem-1", "An", "https://an.example.com");
        h.hub_id = Some("a1b2c3d4".into());
        reg.hubs.push(h);
        assert_eq!(resolve(&reg, "rem-1").as_deref(), Some("rem-1"));
        assert_eq!(resolve(&reg, "A1B2C3D4").as_deref(), Some("rem-1"));
        assert_eq!(resolve(&reg, "nope"), None);
    }
}
