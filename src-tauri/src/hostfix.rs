//! G6: local hub origins are `http://cfharness.localhost:<port>` (the hub requires that Host — WT-1, host-only
//! cookies; never switch to 127.0.0.1). WKWebView resolves names with the system resolver, and some macOS
//! versions/configs (seen on GitHub's macOS 15 runner) do not map `*.localhost` to loopback → the page never loads.
//!
//! Before loading a local hub the app resolves its host with the system resolver (getaddrinfo) and requires
//! loopback-only answers (cached per host for the session). If that fails the shell shows a screen offering ONE
//! privileged action (`fix`): after an explicit confirmation, `osascript … with administrator privileges` appends
//! `127.0.0.1 <host>` / `::1 <host>` to /etc/hosts when missing and flushes the DNS cache. This is the only place the
//! app ever asks for admin rights. The host is strictly validated and the script is built from constants + that
//! validated host only (no quotes/backslashes reach AppleScript).
//!
//! WebView2 (Windows) resolves `localhost` / `*.localhost` to loopback inside Chromium's own resolver
//! (`net::IsLocalHostname` → `ResolveLocalHostname`, RFC 6761) without asking the OS, while the Windows system
//! resolver does not map arbitrary `*.localhost` names — so the check is skipped on Windows.

use serde::Serialize;
use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Hosts file the fix edits.
pub const HOSTS_PATH: &str = "/etc/hosts";
/// Loopback addresses written for the host, in this order.
const LOOPBACK_IPS: [&str; 2] = ["127.0.0.1", "::1"];
/// Upper bound for one resolver call (an mDNS fallback for `.localhost` can hang).
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Debug builds only: pretend the host does not resolve (to exercise the screen without touching /etc/hosts).
pub const FORCE_ENV: &str = "CFHARNESS_DESKTOP_FORCE_UNRESOLVED";

/// `[a-z0-9.-]+\.localhost`, with sane DNS labels (1–63 chars, no leading/trailing `-`), ≤ 253 chars. Plain
/// `localhost` is not accepted (it is always in /etc/hosts and never needs a fix).
pub fn valid_host(host: &str) -> bool {
    let Some(prefix) = host.strip_suffix(".localhost") else { return false };
    if prefix.is_empty() || host.len() > 253 {
        return false;
    }
    prefix.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// Outcome of resolving a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Resolved, and every address is loopback.
    Loopback,
    /// Resolved to at least one non-loopback address (never load: the hub would be someone else).
    NotLoopback(Vec<IpAddr>),
    /// No answer (NXDOMAIN, resolver error, timeout).
    Unresolved,
}

impl Resolution {
    pub fn ok(&self) -> bool {
        *self == Resolution::Loopback
    }
}

fn is_loopback(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|m| m.is_loopback()),
    }
}

/// Classify resolver results: loopback only if there is at least one address and all are loopback.
pub fn classify(result: Option<Vec<IpAddr>>) -> Resolution {
    match result {
        Some(addrs) if addrs.is_empty() => Resolution::Unresolved,
        Some(addrs) => {
            if addrs.iter().all(is_loopback) {
                Resolution::Loopback
            } else {
                Resolution::NotLoopback(addrs.into_iter().filter(|a| !is_loopback(a)).collect())
            }
        }
        None => Resolution::Unresolved,
    }
}

/// System resolver (getaddrinfo via `ToSocketAddrs`). Blocking.
pub fn system_resolve(host: &str) -> Option<Vec<IpAddr>> {
    (host, 80u16).to_socket_addrs().ok().map(|it| it.map(|sa| sa.ip()).collect())
}

fn cache() -> &'static Mutex<HashMap<String, Resolution>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Resolution>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forget the cached result for `host` (retry, after the fix).
pub fn invalidate(host: &str) {
    cache().lock().unwrap_or_else(|p| p.into_inner()).remove(host);
}

fn forced() -> bool {
    cfg!(debug_assertions) && std::env::var(FORCE_ENV).is_ok_and(|v| v == "1")
}

/// Does `host` (of a local hub origin) resolve to loopback only? Cached per host for the session. Skipped on
/// Windows (see module doc). Blocking — call from `spawn_blocking`.
pub fn check(host: &str) -> Resolution {
    if cfg!(windows) {
        return Resolution::Loopback;
    }
    if let Some(r) = cache().lock().unwrap_or_else(|p| p.into_inner()).get(host) {
        return r.clone();
    }
    let r = if forced() {
        log::warn!("{FORCE_ENV}=1 (bản debug): giả lập {host} không phân giải được");
        Resolution::Unresolved
    } else {
        classify(system_resolve(host))
    };
    match &r {
        Resolution::Loopback => log::info!("G6: {host} phân giải về loopback"),
        Resolution::NotLoopback(a) => log::warn!("G6: {host} phân giải ra địa chỉ KHÔNG phải loopback: {a:?}"),
        Resolution::Unresolved => log::warn!("G6: {host} không phân giải được bằng resolver hệ thống"),
    }
    cache().lock().unwrap_or_else(|p| p.into_inner()).insert(host.to_string(), r.clone());
    r
}

/// Async wrapper with a timeout (timeout = unresolved, not cached).
pub async fn check_async(host: String) -> Resolution {
    let h = host.clone();
    let task = tauri::async_runtime::spawn_blocking(move || check(&h));
    match tokio::time::timeout(RESOLVE_TIMEOUT, task).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Resolution::Unresolved,
        Err(_) => {
            log::warn!("G6: phân giải {host} quá {RESOLVE_TIMEOUT:?}");
            Resolution::Unresolved
        }
    }
}

/// The hosts lines the fix needs for `host`, in order.
pub fn wanted_lines(host: &str) -> Vec<String> {
    LOOPBACK_IPS.iter().map(|ip| format!("{ip} {host}")).collect()
}

/// Is there a hosts entry mapping `ip` to `host` (comments ignored, any whitespace, aliases, case-insensitive host)?
pub fn has_entry(contents: &str, ip: &str, host: &str) -> bool {
    let Ok(want) = ip.parse::<IpAddr>() else { return false };
    contents.lines().any(|line| {
        let line = line.split('#').next().unwrap_or("");
        let mut it = line.split_whitespace();
        let Some(first) = it.next() else { return false };
        first.parse::<IpAddr>().is_ok_and(|a| a == want) && it.any(|name| name.eq_ignore_ascii_case(host))
    })
}

/// Lines of `wanted_lines(host)` not already present in `contents`.
pub fn missing_lines(contents: &str, host: &str) -> Vec<String> {
    LOOPBACK_IPS
        .iter()
        .filter(|ip| !has_entry(contents, ip, host))
        .map(|ip| format!("{ip} {host}"))
        .collect()
}

/// Shell script run as root. Appends only the given lines (each still guarded by an exact-line grep, in case the
/// file changed since it was read), adding a newline first if the file does not end with one, then flushes the DNS
/// cache. Contains no `"` and no `\` (it is embedded in an AppleScript string literal). `None` if the host is invalid
/// or a line is not one of `wanted_lines(host)`.
pub fn fix_script(host: &str, lines: &[String]) -> Option<String> {
    if !valid_host(host) {
        return None;
    }
    let wanted = wanted_lines(host);
    if lines.iter().any(|l| !wanted.contains(l)) {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if !lines.is_empty() {
        parts.push(format!("test $(/usr/bin/tail -c1 {HOSTS_PATH} | /usr/bin/wc -l) -eq 1 || echo >> {HOSTS_PATH}"));
    }
    for l in wanted.iter().filter(|w| lines.contains(w)) {
        parts.push(format!("/usr/bin/grep -qxF '{l}' {HOSTS_PATH} || echo '{l}' >> {HOSTS_PATH}"));
    }
    parts.push("/usr/bin/dscacheutil -flushcache".into());
    parts.push("/usr/bin/killall -HUP mDNSResponder || true".into());
    let s = parts.join("; ");
    debug_assert!(!s.contains('"') && !s.contains('\\'));
    if s.contains('"') || s.contains('\\') {
        return None;
    }
    Some(s)
}

/// Prompt shown by macOS in the admin password dialog (constant, no quotes/backslashes).
pub const ADMIN_PROMPT: &str =
    "Cfharness Desktop cần quyền quản trị để thêm cfharness.localhost vào /etc/hosts (một lần) để mở hub trên máy này.";

/// The AppleScript passed to `osascript -e`.
pub fn applescript(script: &str) -> String {
    format!("do shell script \"{script}\" with administrator privileges with prompt \"{ADMIN_PROMPT}\"")
}

/// Command a user can paste into Terminal to do the same by hand.
pub fn manual_command(host: &str) -> String {
    let lines = wanted_lines(host);
    format!(
        "sudo sh -c 'grep -qxF \"{a}\" /etc/hosts || echo \"{a}\" >> /etc/hosts; grep -qxF \"{b}\" /etc/hosts || echo \"{b}\" >> /etc/hosts' && sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder",
        a = lines[0],
        b = lines[1]
    )
}

/// Data for the shell screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub host: String,
    /// Lines that will be appended (empty: entries already there, the fix only flushes the DNS cache).
    pub lines: Vec<String>,
    pub manual: String,
    /// The automatic fix is available (macOS + valid host).
    pub can_fix: bool,
}

pub fn plan_for(host: &str, hosts_contents: &str) -> Plan {
    Plan {
        host: host.to_string(),
        lines: missing_lines(hosts_contents, host),
        manual: manual_command(host),
        can_fix: cfg!(target_os = "macos") && valid_host(host),
    }
}

pub fn read_hosts() -> String {
    std::fs::read_to_string(HOSTS_PATH).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixError {
    /// The user dismissed the admin prompt.
    Cancelled,
    Failed(String),
}

/// Run the privileged fix (macOS). Blocking.
pub fn run_fix(host: &str, lines: &[String]) -> Result<(), FixError> {
    if !cfg!(target_os = "macos") {
        return Err(FixError::Failed("Chỉ hỗ trợ trên macOS".into()));
    }
    let script = fix_script(host, lines).ok_or_else(|| FixError::Failed("Tên máy không hợp lệ".into()))?;
    log::info!("G6: chạy sửa /etc/hosts cho {host} ({} dòng thêm) bằng quyền quản trị", lines.len());
    let out = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(applescript(&script))
        .output()
        .map_err(|e| FixError::Failed(format!("Không chạy được osascript: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if is_cancel(&err) {
        log::info!("G6: người dùng huỷ hộp thoại quyền quản trị");
        return Err(FixError::Cancelled);
    }
    log::warn!("G6: osascript lỗi: {}", err.trim());
    Err(FixError::Failed(crate::local::one_line(err.trim(), 200)))
}

/// osascript reports a dismissed admin prompt as error -128 ("User canceled.").
pub fn is_cancel(stderr: &str) -> bool {
    stderr.contains("(-128)") || stderr.contains("User canceled") || stderr.contains("User cancelled")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn host_validation() {
        for ok in ["cfharness.localhost", "a.b-c.localhost", "x1.localhost", "hub-2.cfharness.localhost"] {
            assert!(valid_host(ok), "{ok}");
        }
        for bad in [
            "localhost",
            ".localhost",
            "cfharness.localhost.",
            "CFHARNESS.localhost",
            "cfharness.localhost.evil.com",
            "evil.com",
            "a..localhost",
            "-a.localhost",
            "a-.localhost",
            "a b.localhost",
            "a'.localhost",
            "a\".localhost",
            "a;rm.localhost",
            "a$(x).localhost",
            "a\\.localhost",
            "a_b.localhost",
            "ả.localhost",
            "127.0.0.1",
            "",
        ] {
            assert!(!valid_host(bad), "{bad:?}");
        }
        let long = format!("{}.localhost", "a".repeat(64));
        assert!(!valid_host(&long));
        let long_total = format!("{}localhost", "abcdefghi.".repeat(25));
        assert!(long_total.len() > 253 && !valid_host(&long_total));
    }

    #[test]
    fn loopback_only() {
        let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        let other = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));
        let mapped = IpAddr::V6(Ipv4Addr::new(127, 0, 0, 1).to_ipv6_mapped());
        assert_eq!(classify(Some(vec![v4, v6])), Resolution::Loopback);
        assert_eq!(classify(Some(vec![IpAddr::V4(Ipv4Addr::new(127, 1, 2, 3))])), Resolution::Loopback);
        assert_eq!(classify(Some(vec![mapped])), Resolution::Loopback);
        assert_eq!(classify(Some(vec![v4, other])), Resolution::NotLoopback(vec![other]));
        assert_eq!(classify(Some(vec![])), Resolution::Unresolved);
        assert_eq!(classify(None), Resolution::Unresolved);
        assert!(Resolution::Loopback.ok() && !Resolution::Unresolved.ok());
    }

    #[test]
    fn hosts_detection() {
        let h = "cfharness.localhost";
        assert_eq!(missing_lines("", h), vec!["127.0.0.1 cfharness.localhost", "::1 cfharness.localhost"]);
        let sample = "##\n# Host Database\n127.0.0.1\tlocalhost\n255.255.255.255\tbroadcasthost\n::1             localhost\n";
        assert_eq!(missing_lines(sample, h).len(), 2);
        // Already present: tabs, aliases, upper case, trailing comment, no trailing newline.
        let present = format!("{sample}127.0.0.1  foo.localhost  CFHARNESS.localhost # app\n::1\tcfharness.localhost");
        assert!(missing_lines(&present, h).is_empty());
        // Partially present.
        let v4_only = format!("{sample}127.0.0.1 cfharness.localhost\n");
        assert_eq!(missing_lines(&v4_only, h), vec!["::1 cfharness.localhost"]);
        // Commented out / wrong address / substring do not count.
        let tricky = "# 127.0.0.1 cfharness.localhost\n10.0.0.1 cfharness.localhost\n127.0.0.1 xcfharness.localhost\n::1 cfharness.localhostx\n";
        assert_eq!(missing_lines(tricky, h).len(), 2);
        // Equivalent spelling of ::1.
        assert!(has_entry("0:0:0:0:0:0:0:1 cfharness.localhost", "::1", h));
    }

    #[test]
    fn script_exact() {
        let h = "cfharness.localhost";
        let s = fix_script(h, &wanted_lines(h)).unwrap();
        assert_eq!(
            s,
            "test $(/usr/bin/tail -c1 /etc/hosts | /usr/bin/wc -l) -eq 1 || echo >> /etc/hosts; \
             /usr/bin/grep -qxF '127.0.0.1 cfharness.localhost' /etc/hosts || echo '127.0.0.1 cfharness.localhost' >> /etc/hosts; \
             /usr/bin/grep -qxF '::1 cfharness.localhost' /etc/hosts || echo '::1 cfharness.localhost' >> /etc/hosts; \
             /usr/bin/dscacheutil -flushcache; /usr/bin/killall -HUP mDNSResponder || true"
        );
        let only6 = fix_script(h, &["::1 cfharness.localhost".to_string()]).unwrap();
        assert_eq!(
            only6,
            "test $(/usr/bin/tail -c1 /etc/hosts | /usr/bin/wc -l) -eq 1 || echo >> /etc/hosts; \
             /usr/bin/grep -qxF '::1 cfharness.localhost' /etc/hosts || echo '::1 cfharness.localhost' >> /etc/hosts; \
             /usr/bin/dscacheutil -flushcache; /usr/bin/killall -HUP mDNSResponder || true"
        );
        // Nothing missing: only the DNS cache flush, the file is not touched.
        assert_eq!(fix_script(h, &[]).unwrap(), "/usr/bin/dscacheutil -flushcache; /usr/bin/killall -HUP mDNSResponder || true");
        assert_eq!(
            applescript("X"),
            format!("do shell script \"X\" with administrator privileges with prompt \"{ADMIN_PROMPT}\"")
        );
        assert!(!ADMIN_PROMPT.contains('"') && !ADMIN_PROMPT.contains('\\'));
    }

    #[test]
    fn script_rejects_injection() {
        assert_eq!(fix_script("a'; rm -rf /; '.localhost", &[]), None);
        assert_eq!(fix_script("evil.com", &[]), None);
        // Lines must be exactly the wanted ones for this host.
        let h = "cfharness.localhost";
        assert_eq!(fix_script(h, &["10.0.0.1 cfharness.localhost".into()]), None);
        assert_eq!(fix_script(h, &["127.0.0.1 cfharness.localhost' >> /x; echo '".into()]), None);
        assert_eq!(fix_script(h, &["127.0.0.1 other.localhost".into()]), None);
    }

    #[test]
    fn manual_and_plan() {
        let h = "cfharness.localhost";
        assert_eq!(
            manual_command(h),
            "sudo sh -c 'grep -qxF \"127.0.0.1 cfharness.localhost\" /etc/hosts || echo \"127.0.0.1 cfharness.localhost\" >> /etc/hosts; grep -qxF \"::1 cfharness.localhost\" /etc/hosts || echo \"::1 cfharness.localhost\" >> /etc/hosts' && sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder"
        );
        let p = plan_for(h, "127.0.0.1 cfharness.localhost\n");
        assert_eq!(p.lines, vec!["::1 cfharness.localhost"]);
        assert_eq!(p.can_fix, cfg!(target_os = "macos"));
        assert!(!plan_for("evil.com", "").can_fix);
    }

    #[test]
    fn cancel_detection() {
        assert!(is_cancel("0:94: execution error: User canceled. (-128)\n"));
        assert!(!is_cancel("0:94: execution error: sh: foo: command not found (127)"));
    }

    #[test]
    fn cache_and_invalidate() {
        // `localhost` resolves everywhere CI runs; result is cached then invalidated.
        let h = "localhost";
        invalidate(h);
        let r = check(h);
        if !cfg!(windows) {
            assert!(cache().lock().unwrap().contains_key(h));
        }
        invalidate(h);
        assert!(!cache().lock().unwrap().contains_key(h));
        let _ = r;
    }
}
