//! Automatic install of `cfharness` (+ `uv` when missing) — PLAN §3.1, decision 10.5.
//!
//! Flow: the onboarding screen asks `install_plan` → the app reads the latest release tag of the hub repo (same source
//! as the hub's `updater.REPO`), checks `uv` / `git` / `claude`, and shows the exact commands. Nothing runs until the
//! user clicks "Cài" (`install_run` with the plan's nonce). Then:
//!   0. no `uv` → the official Astral installer (macOS/Linux: `curl -LsSf https://astral.sh/uv/install.sh | sh`,
//!      Windows: PowerShell `irm https://astral.sh/uv/install.ps1 | iex`);
//!   1. `uv tool install --force "cfharness @ git+https://github.com/<repo>@<tag>"` (a git install, so the hub's own
//!      "Cập nhật hệ thống" keeps working);
//!   2. `cfharness doctor` (information only);
//!   3. the shell then opens the default hub, which runs `cfharness web --json --ensure` (C1).
//!
//! Output is streamed line by line to the shell (`shell://install`). Never sudo / admin: user directories only, and
//! the app refuses to install when it runs as root. Missing `claude` CLI → guidance only (logging in is the human's job).

use crate::core::Core;
use crate::fetch::Fetched;
use crate::local;
use crate::webviews;
use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, EventTarget, Manager};
use url::Url;

/// Release source of the hub — must equal `REPO` in agent-hub `src/cfharness/updater.py`.
pub const HUB_REPO: &str = "lephuong123/agent-hub";
pub const UV_SH: &str = "https://astral.sh/uv/install.sh";
pub const UV_PS1: &str = "https://astral.sh/uv/install.ps1";
const STEP_TIMEOUT: Duration = Duration::from_secs(20 * 60);

pub fn valid_tag(t: &str) -> bool {
    parse_version(t).is_some()
}

pub fn parse_version(t: &str) -> Option<(u64, u64, u64)> {
    let r = t.strip_prefix('v')?;
    let mut it = r.split('.');
    let mut n = [0u64; 3];
    for slot in &mut n {
        let p = it.next()?;
        if p.is_empty() || p.len() > 6 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = p.parse().ok()?;
    }
    if it.next().is_some() {
        return None;
    }
    Some((n[0], n[1], n[2]))
}

/// `GET /repos/<repo>/releases/latest` → tag.
pub fn tag_from_release(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let t = v.get("tag_name")?.as_str()?;
    valid_tag(t).then(|| t.to_string())
}

/// `GET /repos/<repo>/tags` → highest `vX.Y.Z` (no release published).
pub fn tag_from_tags(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.as_array()?
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .filter_map(|n| parse_version(n).map(|v| (v, n.to_string())))
        .max_by_key(|(v, _)| *v)
        .map(|(_, n)| n)
}

pub fn install_spec(tag: &str) -> String {
    format!("cfharness @ git+https://github.com/{HUB_REPO}@{tag}")
}

/// Remove ANSI escapes and control characters from a log line; ≤ 500 chars.
pub fn clean_log(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ ... final byte @..~ ; OSC: ESC ] ... BEL / ESC \
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for d in chars.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(d) = chars.next() {
                        if d == '\u{7}' || (d == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if c == '\t' {
            out.push(' ');
        } else if !crate::directory::is_unsafe_char(c) {
            out.push(c);
        }
    }
    out.chars().take(500).collect()
}

fn executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.is_file() && std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

fn find_in(names: &[&str], extra: &[PathBuf]) -> Option<PathBuf> {
    let mut dirs_: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs_.extend(extra.iter().cloned());
    for d in dirs_.into_iter().filter(|d| d.is_absolute()) {
        for n in names {
            let c = d.join(n);
            if executable(&c) {
                return Some(c);
            }
        }
    }
    None
}

fn user_bin_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let mut v = local::extra_bin_dirs(&home);
    v.push(home.join(".cargo").join("bin"));
    #[cfg(windows)]
    if let Some(p) = std::env::var_os("USERPROFILE") {
        v.push(PathBuf::from(p).join(".local").join("bin"));
    }
    v
}

pub fn find_uv() -> Option<PathBuf> {
    find_in(if cfg!(windows) { &["uv.exe"] } else { &["uv"] }, &user_bin_dirs())
}

pub fn find_claude() -> Option<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let mut extra = user_bin_dirs();
    extra.push(home.join(".claude").join("local"));
    #[cfg(windows)]
    if let Some(p) = std::env::var_os("APPDATA") {
        extra.push(PathBuf::from(p).join("npm"));
    }
    find_in(if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] }, &extra)
}

fn git_works() -> bool {
    let mut c = Command::new("git");
    c.arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    no_window(&mut c);
    c.status().map(|s| s.success()).unwrap_or(false)
}

fn no_window(_c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        _c.creation_flags(CREATE_NO_WINDOW);
    }
}

#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

// ------------------------------------------------------------------------------------------------ plan

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub id: String,
    pub label: String,
    /// Exact command shown to the user.
    pub command: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub nonce: u64,
    pub tag: String,
    pub repo: String,
    pub uv: Option<String>,
    pub git_ok: bool,
    pub claude: Option<String>,
    /// Where `uv tool` puts the `cfharness` command.
    pub target_dir: String,
    pub steps: Vec<Step>,
    /// Blocking problem (shown instead of the "Cài" button).
    pub blocker: Option<String>,
    pub os: String,
}

pub fn uv_step() -> Step {
    let command = if cfg!(windows) {
        format!("powershell -NoProfile -ExecutionPolicy ByPass -Command \"irm {UV_PS1} | iex\"")
    } else {
        format!("curl -LsSf {UV_SH} | sh")
    };
    Step {
        id: "uv".into(), label: "Cài uv (trình quản lý Python của Astral) vào thư mục người dùng".into(), command
    }
}

pub fn build_steps(tag: &str, uv: Option<&Path>) -> Vec<Step> {
    let mut v = vec![];
    if uv.is_none() {
        v.push(uv_step());
    }
    let uv_cmd = uv.map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "uv".into());
    v.push(Step {
        id: "cfharness".into(),
        label: format!("Cài cfharness {tag} từ GitHub ({HUB_REPO})"),
        command: format!("{uv_cmd} tool install --force \"{}\"", install_spec(tag)),
    });
    v.push(Step {
        id: "doctor".into(),
        label: "Kiểm tra sức khoẻ (cfharness doctor)".into(),
        command: "cfharness doctor".into(),
    });
    v.push(Step {
        id: "web".into(),
        label: "Khởi động hub mặc định (~/.cfharness) và đăng nhập".into(),
        command: "cfharness web --json --ensure".into(),
    });
    v
}

async fn latest_tag(app: &AppHandle) -> Result<String, String> {
    let core = app.state::<Core>();
    let base = format!("https://api.github.com/repos/{HUB_REPO}");
    let rel = Url::parse(&format!("{base}/releases/latest")).map_err(|e| e.to_string())?;
    match core.fetcher.get(&rel, None).await {
        Ok(Fetched::Body { text, .. }) => {
            if let Some(t) = tag_from_release(&text) {
                return Ok(t);
            }
        }
        Ok(Fetched::NotModified) => {}
        Err(crate::fetch::FetchError::Status(404)) => {}
        Err(e) => return Err(format!("Không đọc được bản phát hành mới nhất: {}", e.message())),
    }
    let tags = Url::parse(&format!("{base}/tags?per_page=100")).map_err(|e| e.to_string())?;
    match core.fetcher.get(&tags, None).await {
        Ok(Fetched::Body { text, .. }) => {
            tag_from_tags(&text).ok_or_else(|| "Repo phát hành chưa có tag vX.Y.Z nào".into())
        }
        Ok(Fetched::NotModified) => Err("Không đọc được danh sách tag".into()),
        Err(e) => Err(format!("Không đọc được danh sách tag: {}", e.message())),
    }
}

pub async fn plan(app: &AppHandle) -> Result<Plan, String> {
    let tag = latest_tag(app).await?;
    let (uv, claude, git_ok) = tauri::async_runtime::spawn_blocking(|| (find_uv(), find_claude(), git_works()))
        .await
        .map_err(|e| e.to_string())?;
    let home = dirs::home_dir().unwrap_or_default();
    let target_dir = std::env::var_os("UV_TOOL_BIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("bin"))
        .to_string_lossy()
        .to_string();
    let blocker = if is_root() {
        Some("App đang chạy với quyền root — không cài. Hãy mở app bằng tài khoản người dùng thường.".to_string())
    } else if !git_ok {
        Some(if cfg!(windows) {
            "Chưa có git. Cài Git for Windows (https://git-scm.com/download/win) rồi bấm “Kiểm tra lại”.".to_string()
        } else {
            "Chưa có git. Chạy `xcode-select --install` trong Terminal (cài Command Line Tools của Apple) rồi bấm “Kiểm tra lại”."
                .to_string()
        })
    } else {
        None
    };
    let nonce = {
        let core = app.state::<Core>();
        let mut g = core.lock();
        g.import_seq += 1;
        g.import_seq
    };
    let p = Plan {
        nonce,
        steps: build_steps(&tag, uv.as_deref()),
        tag,
        repo: HUB_REPO.into(),
        uv: uv.map(|p| p.to_string_lossy().to_string()),
        git_ok,
        claude: claude.map(|p| p.to_string_lossy().to_string()),
        target_dir,
        blocker,
        os: std::env::consts::OS.into(),
    };
    {
        let core = app.state::<Core>();
        core.lock().pending_install = Some(p.clone());
    }
    Ok(p)
}

// ------------------------------------------------------------------------------------------------ run

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallEvent {
    Step { id: String, label: String },
    Log { text: String },
    Done,
    Error { message: String },
}

fn emit(app: &AppHandle, ev: InstallEvent) {
    let _ = app.emit_to(EventTarget::webview(webviews::SHELL_LABEL), "shell://install", ev);
}

/// Run one command, streaming stdout+stderr lines. Blocking.
fn run_streamed(app: &AppHandle, program: &Path, args: &[String], timeout: Duration) -> Result<(), String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(p) = local::child_path(&home) {
        cmd.env("PATH", p);
    }
    for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "VIRTUAL_ENV", "PYTHONPATH"] {
        cmd.env_remove(k);
    }
    cmd.env("NO_COLOR", "1");
    no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("không chạy được {}: {e}", program.display()))?;
    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = vec![];
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(l);
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for l in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = tx.send(l);
            }
        }));
    }
    drop(tx);
    let start = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(l) => {
                let t = clean_log(&l);
                if !t.trim().is_empty() {
                    emit(app, InstallEvent::Log { text: t });
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("quá {} phút chưa xong", timeout.as_secs() / 60));
        }
    }
    for r in readers {
        let _ = r.join();
    }
    let st = child.wait().map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("lệnh kết thúc với mã {}", st.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())))
    }
}

fn install_uv(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(windows)]
    let (prog, args) = (
        PathBuf::from("powershell"),
        vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".into(),
            "ByPass".into(),
            "-Command".into(),
            format!("irm {UV_PS1} | iex"),
        ],
    );
    #[cfg(not(windows))]
    let (prog, args) = (PathBuf::from("/bin/sh"), vec!["-c".to_string(), format!("curl -LsSf {UV_SH} | sh")]);
    run_streamed(app, &prog, &args, STEP_TIMEOUT)?;
    find_uv().ok_or_else(|| "Đã chạy bộ cài uv nhưng không tìm thấy lệnh uv (~/.local/bin)".to_string())
}

fn run_steps(app: &AppHandle, p: &Plan) -> Result<(), String> {
    let mut uv = p.uv.as_ref().map(PathBuf::from).filter(|u| u.is_file());
    for s in &p.steps {
        emit(app, InstallEvent::Step { id: s.id.clone(), label: s.label.clone() });
        match s.id.as_str() {
            "uv" => {
                if uv.is_none() {
                    uv = Some(install_uv(app)?);
                }
            }
            "cfharness" => {
                let uvp = uv.clone().or_else(find_uv).ok_or("Không tìm thấy uv")?;
                let args = vec!["tool".to_string(), "install".into(), "--force".into(), install_spec(&p.tag)];
                run_streamed(app, &uvp, &args, STEP_TIMEOUT)?;
                let bin = local::find_binary().ok_or("Đã cài nhưng không tìm thấy lệnh cfharness")?;
                emit(app, InstallEvent::Log { text: format!("cfharness: {}", bin.display()) });
                let core = app.state::<Core>();
                core.lock().bin = Some(bin);
            }
            "doctor" => {
                let bin = local::find_binary().ok_or("Không tìm thấy lệnh cfharness")?;
                if let Err(e) = run_streamed(app, &bin, &["doctor".to_string()], Duration::from_secs(120)) {
                    // information only
                    emit(
                        app,
                        InstallEvent::Log {
                            text: format!("(cfharness doctor: {e} — bỏ qua, không chặn việc cài)")
                        },
                    );
                }
            }
            _ => {} // "web": done by the caller through the normal hub opening path (C1)
        }
    }
    Ok(())
}

pub async fn run(app: AppHandle, nonce: u64) -> Result<(), String> {
    let plan = {
        let core = app.state::<Core>();
        let mut g = core.lock();
        if g.installing {
            return Err("Đang cài".into());
        }
        let Some(p) = g.pending_install.clone().filter(|p| p.nonce == nonce) else {
            return Err("Kế hoạch cài đã cũ — bấm “Kiểm tra lại”".into());
        };
        if let Some(b) = &p.blocker {
            return Err(b.clone());
        }
        g.installing = true;
        p
    };
    webviews::emit_state(&app);
    let a = app.clone();
    let res = tauri::async_runtime::spawn_blocking(move || run_steps(&a, &plan))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    {
        let core = app.state::<Core>();
        let mut g = core.lock();
        g.installing = false;
        g.pending_install = None;
    }
    match res {
        Ok(()) => {
            emit(&app, InstallEvent::Step { id: "web".into(), label: "Khởi động hub mặc định".into() });
            crate::commands::refresh_local_inner(&app);
            let target = {
                let core = app.state::<Core>();
                let g = core.lock();
                g.reg.active.clone().or_else(|| g.reg.rail_order(false).first().cloned())
            };
            let opened = match target {
                Some(id) => webviews::select(app.clone(), id).await,
                None => crate::commands::create_default_hub(app.clone()).await,
            };
            match opened {
                Ok(()) => {
                    emit(&app, InstallEvent::Done);
                    Ok(())
                }
                Err(e) => {
                    emit(&app, InstallEvent::Error { message: e.clone() });
                    Err(e)
                }
            }
        }
        Err(e) => {
            log::warn!("cài cfharness lỗi: {e}");
            emit(&app, InstallEvent::Error { message: e.clone() });
            webviews::emit_state(&app);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert!(valid_tag("v0.2.10"));
        for t in ["0.2.10", "v1.2", "v1.2.3.4", "v1.2.x", "v1.2.3-rc1", "v1.2.3;rm", ""] {
            assert!(!valid_tag(t), "{t}");
        }
        assert_eq!(tag_from_release(r#"{"tag_name":"v0.2.10","name":"x"}"#).as_deref(), Some("v0.2.10"));
        assert_eq!(tag_from_release(r#"{"tag_name":"nightly"}"#), None);
        assert_eq!(
            tag_from_tags(r#"[{"name":"v0.2.9"},{"name":"v0.2.10"},{"name":"junk"},{"name":"v0.10.0"}]"#).as_deref(),
            Some("v0.10.0")
        );
        assert_eq!(tag_from_tags("[]"), None);
    }

    #[test]
    fn spec_and_steps() {
        assert_eq!(install_spec("v0.2.10"), "cfharness @ git+https://github.com/lephuong123/agent-hub@v0.2.10");
        let s = build_steps("v0.2.10", None);
        assert_eq!(s.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["uv", "cfharness", "doctor", "web"]);
        assert!(s[1]
            .command
            .contains("tool install --force \"cfharness @ git+https://github.com/lephuong123/agent-hub@v0.2.10\""));
        assert!(!s.iter().any(|x| x.command.contains("sudo")));
        let s2 = build_steps("v0.2.10", Some(Path::new("/u/.local/bin/uv")));
        assert_eq!(s2[0].id, "cfharness");
        assert!(s2[0].command.starts_with("/u/.local/bin/uv tool install"));
    }

    #[test]
    fn hub_repo_matches_hub_updater() {
        // The hub repo lives next to this one in development; skip when absent (CI).
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agent-hub/src/cfharness/updater.py");
        if let Ok(src) = std::fs::read_to_string(p) {
            assert!(src.contains(&format!("REPO = \"{HUB_REPO}\"")));
        }
    }

    #[test]
    fn log_cleaning() {
        assert_eq!(clean_log("\u{1b}[32mInstalled\u{1b}[0m 3 packages"), "Installed 3 packages");
        assert_eq!(clean_log("\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7}"), "link");
        assert_eq!(clean_log("a\tb\u{202e}c\r"), "a bc");
        assert_eq!(clean_log(&"x".repeat(900)).len(), 500);
    }
}
