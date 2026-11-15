//! Local hubs (PLAN §3.1): discovery, locating the `cfharness` binary and the C1 contract
//! `cfharness web --json --ensure` (run with `CFHARNESS_HOME=<home>`).
//!
//! The app never reads hub secrets: the one-time `login_url` comes from the CLI running as the same OS user.

use crate::nav::Origin;
use serde::Deserialize;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Env var to point the app at a specific `cfharness` executable (dev/test).
pub const BIN_ENV: &str = "CFHARNESS_DESKTOP_CFHARNESS";

// ---------------------------------------------------------------------------------------------- discovery

/// Inputs of discovery, separated so tests can run against a fake filesystem layout.
pub struct DiscoverInput<'a> {
    /// The user's home dir (default hub = `<home>/.cfharness`).
    pub user_home: &'a Path,
    /// Contents of `~/.config/cfharness/hubs.json` (registry written by each hub daemon), if any.
    pub registry_text: Option<&'a str>,
    /// Temp roots to exclude (test hubs, demos).
    pub temp_roots: &'a [PathBuf],
}

/// HUB_HOME paths listed by the hub-side registry `{"hubs":[{"home": "..."}]}` (or a bare list).
pub fn registry_homes(text: &str) -> Vec<String> {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let list = match &v {
        serde_json::Value::Object(m) => m.get("hubs").and_then(|h| h.as_array()).cloned().unwrap_or_default(),
        serde_json::Value::Array(a) => a.clone(),
        _ => vec![],
    };
    list.iter()
        .filter_map(|h| match h {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(m) => m.get("home").and_then(|x| x.as_str()).map(str::to_string),
            _ => None,
        })
        .collect()
}

fn canon(p: &Path) -> PathBuf {
    dunce_canonicalize(p).unwrap_or_else(|| p.to_path_buf())
}

/// `fs::canonicalize` without the `\\?\` prefix on Windows.
fn dunce_canonicalize(p: &Path) -> Option<PathBuf> {
    let c = std::fs::canonicalize(p).ok()?;
    #[cfg(windows)]
    {
        let s = c.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if !rest.starts_with("UNC\\") {
                return Some(PathBuf::from(rest));
            }
        }
    }
    Some(c)
}

fn under(p: &Path, root: &Path) -> bool {
    p.starts_with(root)
}

/// Is `p` inside one of the temp roots (raw or canonical forms are both compared)?
pub fn is_temp(p: &Path, temp_roots: &[PathBuf]) -> bool {
    let cp = canon(p);
    temp_roots.iter().any(|r| {
        let cr = canon(r);
        under(p, r) || under(&cp, r) || under(p, &cr) || under(&cp, &cr)
    })
}

/// Discover local hubs: registry homes + the default `~/.cfharness`, keeping only existing dirs that contain a
/// `config.yaml`, are absolute, are not inside a temp dir, deduplicated by canonical path (first one wins).
pub fn discover(input: &DiscoverInput) -> Vec<PathBuf> {
    let mut cands: Vec<PathBuf> = vec![input.user_home.join(".cfharness")];
    if let Some(t) = input.registry_text {
        cands.extend(registry_homes(t).into_iter().map(PathBuf::from));
    }
    let mut seen = HashSet::new();
    let mut out = vec![];
    for c in cands {
        if !c.is_absolute() || !c.is_dir() || !c.join("config.yaml").is_file() {
            continue;
        }
        if is_temp(&c, input.temp_roots) {
            continue;
        }
        let key = canon(&c);
        if seen.insert(key.clone()) {
            out.push(c);
        }
    }
    out
}

/// Temp roots of this OS.
pub fn default_temp_roots() -> Vec<PathBuf> {
    let mut v = vec![std::env::temp_dir()];
    #[cfg(unix)]
    {
        for p in ["/tmp", "/private/tmp", "/var/tmp", "/private/var/tmp", "/var/folders", "/private/var/folders"] {
            v.push(PathBuf::from(p));
        }
    }
    #[cfg(windows)]
    {
        for k in ["TEMP", "TMP"] {
            if let Some(p) = std::env::var_os(k) {
                v.push(PathBuf::from(p));
            }
        }
        if let Some(w) = std::env::var_os("SystemRoot") {
            v.push(PathBuf::from(w).join("Temp"));
        }
    }
    v
}

/// Path of the hub-side registry: `~/.config/cfharness/hubs.json` (same path on Windows under %USERPROFILE%).
pub fn hub_registry_path(user_home: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("CFHARNESS_HUBS_REGISTRY") {
        return PathBuf::from(p);
    }
    user_home.join(".config").join("cfharness").join("hubs.json")
}

/// Env var (dev/test only): explicit list of HUB_HOMEs (OS path-list syntax) replacing discovery.
pub const HOMES_ENV: &str = "CFHARNESS_DESKTOP_HOMES";

/// Discover with the real environment.
pub fn discover_system() -> Vec<PathBuf> {
    if let Some(list) = std::env::var_os(HOMES_ENV) {
        return std::env::split_paths(&list).filter(|p| p.is_absolute() && p.is_dir()).collect();
    }
    let Some(home) = dirs::home_dir() else { return vec![] };
    let text = std::fs::read_to_string(hub_registry_path(&home)).ok();
    let roots = default_temp_roots();
    discover(&DiscoverInput { user_home: &home, registry_text: text.as_deref(), temp_roots: &roots })
}

// ---------------------------------------------------------------------------------------------- binary

#[cfg(windows)]
const EXE_NAMES: &[&str] = &["cfharness.exe", "cfharness.cmd", "cfharness.bat"];
#[cfg(not(windows))]
const EXE_NAMES: &[&str] = &["cfharness"];

/// Directories searched for `cfharness` after PATH (GUI apps on macOS get a minimal PATH).
pub fn extra_bin_dirs(user_home: &Path) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(p) = std::env::var_os("UV_TOOL_BIN_DIR") {
        v.push(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os("XDG_BIN_HOME") {
        v.push(PathBuf::from(p));
    }
    v.push(user_home.join(".local").join("bin"));
    #[cfg(not(windows))]
    {
        v.push(user_home.join(".cargo").join("bin"));
        v.push(PathBuf::from("/opt/homebrew/bin"));
        v.push(PathBuf::from("/usr/local/bin"));
    }
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("APPDATA") {
            v.push(PathBuf::from(p).join("uv").join("tools").join("cfharness").join("Scripts"));
        }
    }
    v
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file() && std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Find `cfharness`: env override → PATH → `~/.local/bin` / uv tool bin dir / common prefixes.
pub fn find_binary_in(
    path_var: Option<&std::ffi::OsStr>,
    user_home: &Path,
    env_override: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = env_override {
        if is_executable(&p) {
            return Some(p);
        }
    }
    let mut dirs_: Vec<PathBuf> = path_var.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    dirs_.extend(extra_bin_dirs(user_home));
    for d in dirs_ {
        if !d.is_absolute() {
            continue;
        }
        for n in EXE_NAMES {
            let c = d.join(n);
            if is_executable(&c) {
                return Some(c);
            }
        }
    }
    None
}

pub fn find_binary() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = std::env::var_os("PATH");
    find_binary_in(path.as_deref(), &home, std::env::var_os(BIN_ENV).map(PathBuf::from))
}

/// PATH for the child: the inherited one plus the user bin dirs (so the CLI finds `uv`, `claude`, …).
pub fn child_path(user_home: &Path) -> Option<std::ffi::OsString> {
    let mut parts: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for d in extra_bin_dirs(user_home) {
        if !parts.contains(&d) {
            parts.push(d);
        }
    }
    std::env::join_paths(parts).ok()
}

// ---------------------------------------------------------------------------------------------- C1

/// Successful C1 answer.
#[derive(Debug, Clone, PartialEq)]
pub struct WebInfo {
    pub origin: Origin,
    pub login_url: url::Url,
    pub hub_id: String,
    pub version: String,
    pub name: String,
    pub started: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WebError {
    /// `{"ok":false,"error":code,"message":vi}` from the CLI.
    Hub { code: String, message: String },
    /// Output did not follow the contract.
    Protocol(String),
    /// Could not run the binary / timeout.
    Spawn(String),
}

impl WebError {
    /// Vietnamese text for the shell UI.
    pub fn user_message(&self) -> String {
        match self {
            WebError::Hub { code, message } if !message.is_empty() => format!("{message} ({code})"),
            WebError::Hub { code, .. } => format!("Hub báo lỗi: {code}"),
            WebError::Protocol(m) => {
                format!("cfharness trả về dữ liệu không hợp lệ: {m}. Có thể bản cfharness quá cũ — hãy cập nhật.")
            }
            WebError::Spawn(m) => format!("Không chạy được cfharness: {m}"),
        }
    }
}

#[derive(Deserialize)]
struct RawC1 {
    ok: bool,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    login_url: Option<String>,
    #[serde(default)]
    hub_id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    started: Option<bool>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Strip control + bidi/zero-width characters, collapse to one line, cap length.
pub fn one_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .filter(|c| {
            !c.is_control()
                && !matches!(*c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2069 | 0xFEFF | 0x061C)
        })
        .collect();
    let trimmed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    trimmed.chars().take(max).collect()
}

/// Parse the C1 output: the last non-empty stdout line must be the JSON object.
pub fn parse_c1(stdout: &str) -> Result<WebInfo, WebError> {
    let line = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| WebError::Protocol("không có output".into()))?;
    let raw: RawC1 = serde_json::from_str(line).map_err(|e| WebError::Protocol(format!("JSON lỗi: {e}")))?;
    if !raw.ok {
        return Err(WebError::Hub {
            code: one_line(raw.error.as_deref().unwrap_or("unknown"), 64),
            message: one_line(raw.message.as_deref().unwrap_or(""), 300),
        });
    }
    let origin_s = raw.origin.ok_or_else(|| WebError::Protocol("thiếu origin".into()))?;
    let origin = Origin::parse(&origin_s).ok_or_else(|| WebError::Protocol("origin không hợp lệ".into()))?;
    if !origin.is_loopback() {
        return Err(WebError::Protocol("origin của hub local phải là loopback".into()));
    }
    let login_s = raw.login_url.ok_or_else(|| WebError::Protocol("thiếu login_url".into()))?;
    let login_url = url::Url::parse(&login_s).map_err(|_| WebError::Protocol("login_url không hợp lệ".into()))?;
    if !origin.matches(&login_url)
        || login_url.path() != "/login"
        || !login_url.query_pairs().any(|(k, v)| k == "t" && !v.is_empty())
    {
        return Err(WebError::Protocol("login_url phải là <origin>/login?t=…".into()));
    }
    let hub_id = raw.hub_id.unwrap_or_default();
    if hub_id.is_empty() || hub_id.len() > 64 || !hub_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(WebError::Protocol("hub_id phải là chuỗi hex".into()));
    }
    Ok(WebInfo {
        origin,
        login_url,
        hub_id: hub_id.to_ascii_lowercase(),
        version: one_line(raw.version.as_deref().unwrap_or(""), 32),
        name: one_line(raw.name.as_deref().unwrap_or(""), 60),
        started: raw.started.unwrap_or(false),
    })
}

/// Run `cfharness web --json --ensure` for `home` and parse the answer. Blocking (call from a worker thread).
///
/// stdout is read line by line on a thread; once the CLI process exits we only wait a short grace period, so a
/// detached web server that (wrongly) kept the pipe open cannot hang the app.
pub fn ensure_web(bin: &Path, home: &Path, timeout: Duration) -> Result<WebInfo, WebError> {
    let user_home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let mut cmd = Command::new(bin);
    cmd.args(["web", "--json", "--ensure"])
        .env("CFHARNESS_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(p) = child_path(&user_home) {
        cmd.env("PATH", p);
    }
    // Never leak the wrapping Claude Code session into the hub CLI.
    for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"] {
        cmd.env_remove(k);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| WebError::Spawn(e.to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| WebError::Spawn("không có stdout".into()))?;
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let start = Instant::now();
    let mut lines: Vec<String> = vec![];
    let status = loop {
        while let Ok(l) = rx.try_recv() {
            lines.push(l);
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {}
            Err(e) => return Err(WebError::Spawn(e.to_string())),
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WebError::Spawn(format!("quá {} giây chưa xong", timeout.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let grace = Instant::now();
    while grace.elapsed() < Duration::from_millis(500) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(l) => lines.push(l),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    let out = lines.join("\n");
    match parse_c1(&out) {
        Ok(info) => Ok(info),
        Err(e @ WebError::Hub { .. }) => Err(e),
        Err(e) if !status.success() && out.trim().is_empty() => {
            Err(WebError::Spawn(format!("cfharness thoát với mã {:?} ({e:?})", status.code())))
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn mkhub(p: &Path) {
        fs::create_dir_all(p).unwrap();
        fs::write(p.join("config.yaml"), "{}\n").unwrap();
    }

    #[test]
    fn registry_homes_formats() {
        assert_eq!(registry_homes(r#"{"hubs":[{"home":"/a"},{"home":"/b"}]}"#), vec!["/a", "/b"]);
        assert_eq!(registry_homes(r#"["/a", {"home": "/b"}, 3]"#), vec!["/a", "/b"]);
        assert!(registry_homes("garbage").is_empty());
        assert!(registry_homes(r#"{"x":1}"#).is_empty());
    }

    #[test]
    fn discovery_filters_temp_missing_and_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        let default = user.join(".cfharness");
        mkhub(&default);
        let second = user.join("hub-thu-2");
        mkhub(&second);
        let no_cfg = user.join("no-config");
        fs::create_dir_all(&no_cfg).unwrap();
        let fake_tmp = root.path().join("tmp");
        let temp_hub = fake_tmp.join("tmp.abc");
        mkhub(&temp_hub);
        let reg = serde_json::json!({"hubs": [
            {"home": default.to_string_lossy()},           // duplicate of default
            {"home": second.to_string_lossy()},
            {"home": no_cfg.to_string_lossy()},
            {"home": temp_hub.to_string_lossy()},
            {"home": user.join("gone").to_string_lossy()}, // missing
            {"home": "relative/path"},
        ]})
        .to_string();
        let roots = vec![fake_tmp.clone()];
        let found = discover(&DiscoverInput { user_home: &user, registry_text: Some(&reg), temp_roots: &roots });
        assert_eq!(found, vec![default.clone(), second.clone()]);
    }

    #[test]
    fn discovery_without_default_home() {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        fs::create_dir_all(&user).unwrap();
        let found = discover(&DiscoverInput { user_home: &user, registry_text: None, temp_roots: &[] });
        assert!(found.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn temp_detection_uses_canonical_paths() {
        let root = tempfile::tempdir().unwrap();
        let real_tmp = root.path().join("real");
        mkhub(&real_tmp.join("h"));
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real_tmp, &link).unwrap();
        assert!(is_temp(&link.join("h"), std::slice::from_ref(&real_tmp)));
        assert!(is_temp(&real_tmp.join("h"), std::slice::from_ref(&link)));
        assert!(!is_temp(&root.path().join("other"), &[real_tmp]));
    }

    #[cfg(unix)]
    #[test]
    fn find_binary_search_order() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        let local_bin = user.join(".local").join("bin");
        fs::create_dir_all(&local_bin).unwrap();
        let b = local_bin.join("cfharness");
        fs::write(&b, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&b, fs::Permissions::from_mode(0o755)).unwrap();
        // not on PATH → found in ~/.local/bin
        let empty = std::ffi::OsString::from(root.path().join("nothing"));
        assert_eq!(find_binary_in(Some(&empty), &user, None), Some(b.clone()));
        // on PATH wins
        let pdir = root.path().join("pathdir");
        fs::create_dir_all(&pdir).unwrap();
        let pb = pdir.join("cfharness");
        fs::write(&pb, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&pb, fs::Permissions::from_mode(0o755)).unwrap();
        let pv = std::env::join_paths([pdir.clone()]).unwrap();
        assert_eq!(find_binary_in(Some(&pv), &user, None), Some(pb.clone()));
        // non-executable file is ignored
        fs::set_permissions(&pb, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_binary_in(Some(&pv), &user, None), Some(b.clone()));
        // override wins
        assert_eq!(find_binary_in(Some(&pv), &user, Some(b.clone())), Some(b));
    }

    const OK: &str = r#"{"ok":true,"origin":"http://cfharness.localhost:8711","login_url":"http://cfharness.localhost:8711/login?t=abc123","hub_id":"9f3a01bc","version":"0.2.10","name":"Local","started":true}"#;

    #[test]
    fn c1_ok() {
        let info = parse_c1(&format!("some warning\n{OK}\n\n")).unwrap();
        assert_eq!(info.origin.as_string(), "http://cfharness.localhost:8711");
        assert_eq!(info.login_url.as_str(), "http://cfharness.localhost:8711/login?t=abc123");
        assert_eq!(info.hub_id, "9f3a01bc");
        assert_eq!(info.version, "0.2.10");
        assert_eq!(info.name, "Local");
        assert!(info.started);
    }

    #[test]
    fn c1_error() {
        let e = parse_c1(r#"{"ok":false,"error":"web_not_running","message":"Web chưa chạy"}"#).unwrap_err();
        assert_eq!(e, WebError::Hub { code: "web_not_running".into(), message: "Web chưa chạy".into() });
        assert!(e.user_message().contains("Web chưa chạy"));
    }

    #[test]
    fn c1_rejects_contract_violations() {
        let cases = [
            "",
            "not json",
            r#"{"ok":true}"#,
            // non-loopback origin for a local hub
            r#"{"ok":true,"origin":"https://evil.com","login_url":"https://evil.com/login?t=a","hub_id":"ab"}"#,
            // login url on another origin
            r#"{"ok":true,"origin":"http://cfharness.localhost:8711","login_url":"http://cfharness.localhost:9999/login?t=a","hub_id":"ab"}"#,
            // login url without token
            r#"{"ok":true,"origin":"http://cfharness.localhost:8711","login_url":"http://cfharness.localhost:8711/login","hub_id":"ab"}"#,
            // login url wrong path
            r#"{"ok":true,"origin":"http://cfharness.localhost:8711","login_url":"http://cfharness.localhost:8711/x?t=a","hub_id":"ab"}"#,
            // hub id not hex
            r#"{"ok":true,"origin":"http://cfharness.localhost:8711","login_url":"http://cfharness.localhost:8711/login?t=a","hub_id":"zz"}"#,
            // origin with a path
            r#"{"ok":true,"origin":"http://cfharness.localhost:8711/x","login_url":"http://cfharness.localhost:8711/login?t=a","hub_id":"ab"}"#,
        ];
        for c in cases {
            assert!(matches!(parse_c1(c), Err(WebError::Protocol(_))), "{c}");
        }
    }

    #[test]
    fn c1_sanitizes_free_text() {
        let s = r#"{"ok":true,"origin":"http://cfharness.localhost:8711","login_url":"http://cfharness.localhost:8711/login?t=a","hub_id":"AB","version":"0.2.10","name":"Hub\u202e\u0007\nthu  2"}"#;
        let info = parse_c1(s).unwrap();
        assert_eq!(info.name, "Hub thu 2");
        assert_eq!(info.hub_id, "ab");
        assert!(!info.started);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_web_runs_fake_cli() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("cfharness");
        // echoes CFHARNESS_HOME through the name field to prove the env is passed
        let script = format!(
            "#!/bin/sh\n[ \"$1 $2 $3\" = \"web --json --ensure\" ] || exit 3\necho 'starting...'\nprintf '%s\\n' '{}' | sed \"s|\\\"Local\\\"|\\\"$(basename \"$CFHARNESS_HOME\")\\\"|\"\n",
            OK
        );
        fs::write(&bin, script).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let info = ensure_web(&bin, Path::new("/x/my-hub"), Duration::from_secs(10)).unwrap();
        assert_eq!(info.name, "my-hub");

        let bad = dir.path().join("bad");
        fs::write(&bad, "#!/bin/sh\necho '{\"ok\":false,\"error\":\"boom\",\"message\":\"Hỏng\"}'\nexit 1\n").unwrap();
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(ensure_web(&bad, Path::new("/x"), Duration::from_secs(10)), Err(WebError::Hub { .. })));

        let slow = dir.path().join("slow");
        fs::write(&slow, "#!/bin/sh\nsleep 5\n").unwrap();
        fs::set_permissions(&slow, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(ensure_web(&slow, Path::new("/x"), Duration::from_millis(300)), Err(WebError::Spawn(_))));
    }
}
