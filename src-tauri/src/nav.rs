//! Navigation lock (PLAN §5.1, G4): a hub webview only loads its own origin (+ `about:blank`).
//! Other http(s)/mailto targets are handed to the system browser; everything else is blocked.

use url::Url;

/// A web origin `scheme://host:port` (host lowercased, default port filled in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl Origin {
    /// Parse an origin string. Only `http`/`https`, no userinfo, path must be empty or `/`, no query/fragment.
    pub fn parse(s: &str) -> Option<Origin> {
        let u = Url::parse(s.trim()).ok()?;
        if !u.username().is_empty() || u.password().is_some() {
            return None;
        }
        if !(u.path().is_empty() || u.path() == "/") || u.query().is_some() || u.fragment().is_some() {
            return None;
        }
        Origin::of(&u)
    }

    /// Origin of an arbitrary http(s) URL.
    pub fn of(u: &Url) -> Option<Origin> {
        let scheme = u.scheme().to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return None;
        }
        let host = u.host_str()?.to_ascii_lowercase();
        let port = u.port_or_known_default()?;
        Some(Origin { scheme, host, port })
    }

    pub fn matches(&self, u: &Url) -> bool {
        Origin::of(u).as_ref() == Some(self)
    }

    /// `scheme://host[:port]` without a trailing slash (default ports omitted).
    pub fn as_string(&self) -> String {
        let default = (self.scheme == "http" && self.port == 80) || (self.scheme == "https" && self.port == 443);
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if default {
            format!("{}://{}", self.scheme, host)
        } else {
            format!("{}://{}:{}", self.scheme, host, self.port)
        }
    }

    /// Join a hub-relative path (`/manager?w=1`) onto the origin. Rejects anything that would leave the origin.
    pub fn join(&self, path: &str) -> Option<Url> {
        if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') {
            return None;
        }
        let u = Url::parse(&format!("{}{}", self.as_string(), path)).ok()?;
        if self.matches(&u) {
            Some(u)
        } else {
            None
        }
    }

    /// Loopback-only host check used for local hubs (`cfharness.localhost`, `*.localhost`, 127.0.0.1, ::1).
    pub fn is_loopback(&self) -> bool {
        let h = self.host.trim_start_matches('[').trim_end_matches(']');
        h == "localhost"
            || h.ends_with(".localhost")
            || h == "::1"
            || h.parse::<std::net::Ipv4Addr>().map(|ip| ip.is_loopback()).unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDecision {
    /// Load inside the hub webview.
    Allow,
    /// Cancel and open in the system browser.
    External,
    /// Cancel silently (`javascript:`, `file:`, `data:` …).
    Block,
}

/// Decide what to do with a navigation of the hub webview whose origin is `hub`.
pub fn decide(hub: &Origin, target: &Url) -> NavDecision {
    match target.scheme() {
        "about" => {
            if target.as_str() == "about:blank" || target.as_str() == "about:srcdoc" {
                NavDecision::Allow
            } else {
                NavDecision::Block
            }
        }
        "http" | "https" => {
            if !target.username().is_empty() || target.password().is_some() {
                return NavDecision::Block;
            }
            if hub.matches(target) {
                NavDecision::Allow
            } else {
                NavDecision::External
            }
        }
        // blob: URLs created by the hub page itself (client-side downloads) carry the creator origin.
        "blob" => match Url::parse(target.path()) {
            Ok(inner) if hub.matches(&inner) => NavDecision::Allow,
            _ => NavDecision::Block,
        },
        "mailto" => NavDecision::External,
        _ => NavDecision::Block,
    }
}

/// The hub's login page without a one-time token: the local session expired (or the user logged out).
pub fn is_login_page(hub: &Origin, u: &Url) -> bool {
    hub.matches(u) && u.path() == "/login" && !u.query_pairs().any(|(k, _)| k == "t")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn origin_parse_rules() {
        let o = Origin::parse("http://cfharness.localhost:8711").unwrap();
        assert_eq!(o, Origin { scheme: "http".into(), host: "cfharness.localhost".into(), port: 8711 });
        assert_eq!(Origin::parse("https://An.Example.com/").unwrap().as_string(), "https://an.example.com");
        assert_eq!(Origin::parse("https://an.example.com:443").unwrap().port, 443);
        assert!(Origin::parse("https://an.example.com/path").is_none());
        assert!(Origin::parse("https://u:p@an.example.com").is_none());
        assert!(Origin::parse("ftp://an.example.com").is_none());
        assert!(Origin::parse("https://an.example.com/?x=1").is_none());
        assert!(Origin::parse("not a url").is_none());
    }

    #[test]
    fn same_origin_is_allowed() {
        let o = Origin::parse("http://cfharness.localhost:8711").unwrap();
        assert_eq!(decide(&o, &u("http://cfharness.localhost:8711/manager?w=1")), NavDecision::Allow);
        assert_eq!(decide(&o, &u("http://CFHARNESS.localhost:8711/")), NavDecision::Allow);
        assert_eq!(decide(&o, &u("about:blank")), NavDecision::Allow);
    }

    #[test]
    fn other_origins_go_external() {
        let o = Origin::parse("http://cfharness.localhost:8711").unwrap();
        // different port, scheme or host are all different origins
        assert_eq!(decide(&o, &u("http://cfharness.localhost:8712/")), NavDecision::External);
        assert_eq!(decide(&o, &u("https://cfharness.localhost:8711/")), NavDecision::External);
        assert_eq!(decide(&o, &u("http://127.0.0.1:8711/")), NavDecision::External);
        assert_eq!(decide(&o, &u("https://github.com/x")), NavDecision::External);
        assert_eq!(decide(&o, &u("http://cfharness.localhost.evil.com:8711/")), NavDecision::External);
        assert_eq!(decide(&o, &u("mailto:a@b.c")), NavDecision::External);
        let r = Origin::parse("https://an.example.com").unwrap();
        assert_eq!(decide(&r, &u("https://an.example.com:443/approvals")), NavDecision::Allow);
        assert_eq!(decide(&r, &u("http://an.example.com/")), NavDecision::External);
        assert_eq!(decide(&r, &u("https://evil.an.example.com/")), NavDecision::External);
    }

    #[test]
    fn dangerous_schemes_blocked() {
        let o = Origin::parse("http://cfharness.localhost:8711").unwrap();
        for s in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,<b>x</b>",
            "about:config",
            "ftp://x/y",
            "cfharness://hub/x",
            "http://user:pw@cfharness.localhost:8711/",
        ] {
            assert_eq!(decide(&o, &u(s)), NavDecision::Block, "{s}");
        }
        assert_eq!(decide(&o, &u("blob:http://cfharness.localhost:8711/1234-5678")), NavDecision::Allow);
        assert_eq!(decide(&o, &u("blob:https://evil.com/1234")), NavDecision::Block);
    }

    #[test]
    fn join_stays_in_origin() {
        let o = Origin::parse("http://cfharness.localhost:8711").unwrap();
        assert_eq!(o.join("/manager?w=2").unwrap().as_str(), "http://cfharness.localhost:8711/manager?w=2");
        assert!(o.join("//evil.com/x").is_none());
        assert!(o.join("https://evil.com").is_none());
        assert!(o.join("manager").is_none());
        assert!(o.join("/\\evil.com").is_none());
        assert!(o.join("/@evil.com").is_some()); // path segment, still same origin
    }

    #[test]
    fn login_page_detection() {
        let o = Origin::parse("http://cfharness.localhost:8711").unwrap();
        assert!(is_login_page(&o, &u("http://cfharness.localhost:8711/login")));
        assert!(is_login_page(&o, &u("http://cfharness.localhost:8711/login?next=/x")));
        assert!(!is_login_page(&o, &u("http://cfharness.localhost:8711/login?t=abc")));
        assert!(!is_login_page(&o, &u("http://other.localhost:8711/login")));
        assert!(!is_login_page(&o, &u("http://cfharness.localhost:8711/")));
    }

    #[test]
    fn loopback_hosts() {
        assert!(Origin::parse("http://cfharness.localhost:8711").unwrap().is_loopback());
        assert!(Origin::parse("http://127.0.0.1:8711").unwrap().is_loopback());
        assert!(Origin::parse("http://[::1]:8711").unwrap().is_loopback());
        assert!(!Origin::parse("http://192.168.1.2:8711").unwrap().is_loopback());
        assert!(!Origin::parse("https://an.example.com").unwrap().is_loopback());
    }
}
