//! Cloud hubs (PLAN §3.2, contract C2/C5): classify the URL the user pastes, validate a directory document strictly,
//! and plan a resync against the registry. Everything here is pure (no I/O) so it is fully unit-tested; fetching
//! lives in `fetch.rs`, the Tauri glue in `remote.rs`.
//!
//! Rules (same as the hub's `web/public.parse_public_origin`): origin = `https://host[:port]` only — no http, no IP,
//! no `localhost`, no path/query/fragment/userinfo. Directory: `kind=cfharness.directory`, `schema=1`, ≤ 200 hubs,
//! `id` hex ≤ 16, `name`/`user` one line ≤ 60/≤ 32 with control/bidi/invisible characters removed. Invalid entries
//! are dropped with a warning; an origin change for a known id is never applied without asking the user.

use crate::hubs::{self, Hub, Registry};
use crate::nav::Origin;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use url::Url;

pub const MAX_HUBS: usize = 200;
pub const NAME_MAX: usize = 60;
pub const USER_MAX: usize = 32;
pub const HUB_ID_MAX: usize = 16;
pub const TOKEN_MAX: usize = 512;
/// Supported directory schema.
pub const SCHEMA: u64 = 1;

// ------------------------------------------------------------------------------------------------- text

/// Characters that must never reach the UI from remote data: C0/C1 controls, bidi controls/isolates, zero-width and
/// other invisible format characters, line/paragraph separators, tag characters.
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c as u32,
            0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F
            | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF
            | 0xFFA0 | 0xFFF0..=0xFFFB | 0xE0000..=0xE0FFF)
}

/// One line of display text: whitespace (incl. newlines/tabs) collapsed to single spaces, unsafe characters removed,
/// trimmed and cut to `max` characters.
pub fn clean_line(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if is_unsafe_char(c) {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out.chars().take(max).collect::<String>().trim_end().to_string()
}

// ---------------------------------------------------------------------------------------------- origins

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginError {
    Malformed,
    NotHttps,
    HasUserinfo,
    HasPath,
    IpHost,
    LocalHost,
    BadHost,
}

impl OriginError {
    pub fn message(self) -> &'static str {
        match self {
            OriginError::Malformed => "URL không hợp lệ",
            OriginError::NotHttps => "Chỉ chấp nhận https:// (hub cloud phải có TLS)",
            OriginError::HasUserinfo => "URL không được chứa tên người dùng/mật khẩu",
            OriginError::HasPath => "Origin chỉ gồm https://host[:cổng] — không path, query hay fragment",
            OriginError::IpHost => "Dùng tên miền, không dùng địa chỉ IP",
            OriginError::LocalHost => "Không dùng localhost — hub trên máy này được app tự tìm",
            OriginError::BadHost => "Tên miền không hợp lệ",
        }
    }
}

/// DNS host name check (same shape as the hub's `_HOST_RE`): ≥ 2 labels, `[a-z0-9-]`, no leading/trailing hyphen.
fn valid_dns_host(h: &str) -> bool {
    if h.is_empty() || h.len() > 253 || !h.contains('.') {
        return false;
    }
    h.split('.').all(|l| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    })
}

/// Host rules shared by hub origins and directory URLs.
fn check_host(u: &Url) -> Result<(), OriginError> {
    match u.host() {
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => return Err(OriginError::IpHost),
        Some(url::Host::Domain(_)) => {}
        None => return Err(OriginError::Malformed),
    }
    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(OriginError::LocalHost);
    }
    // An all-numeric dotted name that the URL parser did not read as IPv4 is still an address in disguise.
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(OriginError::IpHost);
    }
    if !valid_dns_host(&host) {
        return Err(OriginError::BadHost);
    }
    Ok(())
}

fn parse_https(s: &str) -> Result<Url, OriginError> {
    let s = s.trim();
    if s.is_empty() || s.len() > 2048 || s.chars().any(|c| c.is_whitespace() || is_unsafe_char(c)) {
        return Err(OriginError::Malformed);
    }
    let u = Url::parse(s).map_err(|_| OriginError::Malformed)?;
    if u.scheme() != "https" {
        return Err(if u.scheme() == "http" { OriginError::NotHttps } else { OriginError::Malformed });
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(OriginError::HasUserinfo);
    }
    check_host(&u)?;
    Ok(u)
}

/// Validate a remote hub origin (`https://host[:port]`, optional trailing `/`).
pub fn validate_origin(s: &str) -> Result<Origin, OriginError> {
    let u = parse_https(s)?;
    if !(u.path().is_empty() || u.path() == "/") || u.query().is_some() || u.fragment().is_some() {
        return Err(OriginError::HasPath);
    }
    Origin::of(&u).ok_or(OriginError::Malformed)
}

/// Directory hub id: 1–16 hex digits, normalised to lowercase.
pub fn valid_hub_id(s: &str) -> Option<String> {
    if s.is_empty() || s.len() > HUB_ID_MAX || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(s.to_ascii_lowercase())
}

// ---------------------------------------------------------------------------------------------- input URL

/// What the user pasted in "Thêm hub / Import URL".
#[derive(Debug, Clone, PartialEq)]
pub enum ImportTarget {
    /// The origin of one hub (`https://an.example.com`).
    Hub(Origin),
    /// A directory URL, the token (`t=`) split off. `url` never contains the token.
    Directory { url: Url, token: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputError {
    Empty,
    Origin(OriginError),
    BadToken,
}

impl InputError {
    pub fn message(&self) -> String {
        match self {
            InputError::Empty => "Hãy dán URL của hub hoặc của directory".into(),
            InputError::Origin(e) => e.message().into(),
            InputError::BadToken => "Token trong URL (tham số t=) không hợp lệ".into(),
        }
    }
}

fn valid_token(t: &str) -> bool {
    !t.is_empty() && t.len() <= TOKEN_MAX && t.bytes().all(|b| b.is_ascii_graphic())
}

/// Classify a pasted URL. No scheme → `https://` is assumed; `http://` is refused (never downgraded or upgraded
/// silently). Root path without query = one hub; `/.well-known/cfharness.json` = that hub; anything else = directory.
pub fn classify_input(input: &str) -> Result<ImportTarget, InputError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(InputError::Empty);
    }
    let with_scheme = if s.contains("://") { s.to_string() } else { format!("https://{s}") };
    let mut u = parse_https(&with_scheme).map_err(InputError::Origin)?;
    u.set_fragment(None);
    let root = (u.path().is_empty() || u.path() == "/") && u.query().is_none();
    if root || (u.path() == "/.well-known/cfharness.json" && u.query().is_none()) {
        return Origin::of(&u).map(ImportTarget::Hub).ok_or(InputError::Origin(OriginError::Malformed));
    }
    let (token, url) = split_token(&u);
    if let Some(t) = &token {
        if !valid_token(t) {
            return Err(InputError::BadToken);
        }
    }
    Ok(ImportTarget::Directory { url, token })
}

/// Remove every `t=` query parameter; returns `(last token, url without it)`.
pub fn split_token(u: &Url) -> (Option<String>, Url) {
    let mut token = None;
    let rest: Vec<(String, String)> = u
        .query_pairs()
        .filter_map(|(k, v)| {
            if k == "t" {
                token = Some(v.into_owned());
                None
            } else {
                Some((k.into_owned(), v.into_owned()))
            }
        })
        .collect();
    let mut out = u.clone();
    if rest.is_empty() {
        out.set_query(None);
    } else {
        out.query_pairs_mut().clear().extend_pairs(rest);
    }
    (token, out)
}

/// Put the token back for the request (never stored, never logged).
pub fn with_token(url: &Url, token: Option<&str>) -> Url {
    let mut u = url.clone();
    if let Some(t) = token {
        u.query_pairs_mut().append_pair("t", t);
    }
    u
}

/// Display form of a directory URL: host + path, never the query.
pub fn display_url(u: &Url) -> String {
    format!("{}{}", u.host_str().unwrap_or_default(), u.path())
}

// ---------------------------------------------------------------------------------------------- C2 metadata

#[derive(Debug, Clone, PartialEq)]
pub struct HubMeta {
    pub schema: u64,
    pub app_api: u64,
}

/// `GET /.well-known/cfharness.json` → `{"kind":"cfharness.hub","schema":1,"app_api":1}`.
pub fn parse_hub_meta(text: &str) -> Option<HubMeta> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("kind")?.as_str()? != "cfharness.hub" {
        return None;
    }
    let schema = v.get("schema")?.as_u64().filter(|&s| s >= 1)?;
    let app_api = v.get("app_api").and_then(Value::as_u64).unwrap_or(0);
    Some(HubMeta { schema, app_api })
}

// ---------------------------------------------------------------------------------------------- directory

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DirHub {
    pub id: String,
    pub name: String,
    pub user: String,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirDoc {
    pub name: String,
    pub generated_at: Option<String>,
    pub hubs: Vec<DirHub>,
}

/// A dropped entry: position in `hubs[]` + fixed Vietnamese reason (never text from the document).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Warning {
    pub index: usize,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DirError {
    NotJson,
    NotDirectory,
    SchemaTooNew(u64),
    BadSchema,
    TooMany(usize),
}

impl DirError {
    pub fn message(&self) -> String {
        match self {
            DirError::NotJson => "Nội dung không phải JSON".into(),
            DirError::NotDirectory => "Đây không phải directory cfharness (thiếu kind = cfharness.directory)".into(),
            DirError::SchemaTooNew(s) => format!("Directory dùng schema {s}, mới hơn bản app này — hãy cập nhật app"),
            DirError::BadSchema => "Directory thiếu schema hoặc schema không hợp lệ".into(),
            DirError::TooMany(n) => format!("Directory có {n} hub, vượt giới hạn {MAX_HUBS}"),
        }
    }
}

/// Parse + validate a directory document. `fallback_name` (the directory host) is used when `name` is empty.
pub fn parse_directory(text: &str, fallback_name: &str) -> Result<(DirDoc, Vec<Warning>), DirError> {
    let v: Value = serde_json::from_str(text).map_err(|_| DirError::NotJson)?;
    let obj = v.as_object().ok_or(DirError::NotDirectory)?;
    if obj.get("kind").and_then(Value::as_str) != Some("cfharness.directory") {
        return Err(DirError::NotDirectory);
    }
    match obj.get("schema").and_then(Value::as_u64) {
        Some(SCHEMA) => {}
        Some(s) if s > SCHEMA => return Err(DirError::SchemaTooNew(s)),
        _ => return Err(DirError::BadSchema),
    }
    let list = obj.get("hubs").and_then(Value::as_array).ok_or(DirError::NotDirectory)?;
    if list.len() > MAX_HUBS {
        return Err(DirError::TooMany(list.len()));
    }
    let mut name = obj.get("name").and_then(Value::as_str).map(|s| clean_line(s, NAME_MAX)).unwrap_or_default();
    if name.is_empty() {
        name = clean_line(fallback_name, NAME_MAX);
    }
    let generated_at =
        obj.get("generated_at").and_then(Value::as_str).map(|s| clean_line(s, 40)).filter(|s| !s.is_empty());

    let mut hubs = vec![];
    let mut warnings = vec![];
    let mut ids = HashSet::new();
    let mut origins = HashSet::new();
    for (index, item) in list.iter().enumerate() {
        let warn = |reason| Warning { index, reason };
        let Some(m) = item.as_object() else {
            warnings.push(warn("mục không phải object"));
            continue;
        };
        let Some(id) = m.get("id").and_then(Value::as_str).and_then(valid_hub_id) else {
            warnings.push(warn("id phải là 1–16 chữ số hex"));
            continue;
        };
        let origin = match m.get("origin").and_then(Value::as_str).map(validate_origin) {
            Some(Ok(o)) => o.as_string(),
            Some(Err(e)) => {
                warnings.push(warn(e.message()));
                continue;
            }
            None => {
                warnings.push(warn("thiếu origin"));
                continue;
            }
        };
        if !ids.insert(id.clone()) {
            warnings.push(warn("id trùng với mục trước"));
            continue;
        }
        if !origins.insert(origin.clone()) {
            warnings.push(warn("origin trùng với mục trước"));
            continue;
        }
        let user = m.get("user").and_then(Value::as_str).map(|s| clean_line(s, USER_MAX)).unwrap_or_default();
        let mut hname = m.get("name").and_then(Value::as_str).map(|s| clean_line(s, NAME_MAX)).unwrap_or_default();
        if hname.is_empty() {
            hname = if user.is_empty() { origin.trim_start_matches("https://").to_string() } else { user.clone() };
        }
        hubs.push(DirHub { id, name: hname, user, origin });
    }
    Ok((DirDoc { name, generated_at, hubs }, warnings))
}

// ---------------------------------------------------------------------------------------------- ids

/// Registry id of a directory, derived from its URL WITHOUT token (re-importing the same URL updates it).
pub fn directory_id(url_without_token: &Url) -> String {
    format!("dir-{}", hubs::short_hash(url_without_token.as_str()))
}

/// Registry id of a hub added from a directory: unique per (directory, hub id), stable across resyncs.
pub fn dir_hub_reg_id(dir_id: &str, hub_id: &str) -> String {
    let d = dir_id.strip_prefix("dir-").unwrap_or(dir_id);
    format!("rem-{d}-{hub_id}")
}

/// Registry id of a hub added by its origin.
pub fn origin_reg_id(origin: &Origin) -> String {
    format!("rem-{}", hubs::short_hash(&origin.as_string()))
}

// ---------------------------------------------------------------------------------------------- resync plan

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OriginChange {
    /// Registry id of the hub.
    pub hub: String,
    pub directory: String,
    pub name: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SyncPlan {
    pub add: Vec<DirHub>,
    /// A hub added by origin earlier that this directory now lists: attach it (keeps its id and session).
    pub adopt: Vec<(String, DirHub)>,
    pub update: Vec<(String, DirHub)>,
    pub stale: Vec<String>,
    pub unstale: Vec<String>,
    pub origin_changes: Vec<OriginChange>,
    /// Entries not added: (directory hub id, fixed reason).
    pub skipped: Vec<(String, &'static str)>,
}

impl SyncPlan {
    pub fn is_noop(&self) -> bool {
        self.add.is_empty()
            && self.adopt.is_empty()
            && self.update.is_empty()
            && self.stale.is_empty()
            && self.unstale.is_empty()
    }
}

/// Compare a validated directory with the registry. Never deletes; never changes an origin (→ `origin_changes`).
pub fn plan_sync(reg: &Registry, dir_id: &str, doc: &DirDoc) -> SyncPlan {
    let dismissed: HashSet<&str> =
        reg.directory(dir_id).map(|d| d.dismissed.iter().map(String::as_str).collect()).unwrap_or_default();
    let mut plan = SyncPlan::default();
    let mut listed = HashSet::new();
    for e in &doc.hubs {
        listed.insert(e.id.as_str());
        let existing =
            reg.hubs.iter().find(|h| h.directory.as_deref() == Some(dir_id) && h.hub_id.as_deref() == Some(&e.id));
        if let Some(h) = existing {
            if h.stale {
                plan.unstale.push(h.id.clone());
            }
            let cur = h.origin.clone().unwrap_or_default();
            if cur != e.origin && h.rejected_origin.as_deref() != Some(e.origin.as_str()) {
                plan.origin_changes.push(OriginChange {
                    hub: h.id.clone(),
                    directory: dir_id.to_string(),
                    name: h.name.clone(),
                    from: cur,
                    to: e.origin.clone(),
                });
            }
            if h.dir_name.as_deref() != Some(e.name.as_str()) || h.user.as_deref().unwrap_or("") != e.user {
                plan.update.push((h.id.clone(), e.clone()));
            }
            continue;
        }
        if dismissed.contains(e.id.as_str()) {
            plan.skipped.push((e.id.clone(), "bạn đã xoá hub này khỏi app"));
            continue;
        }
        match reg.hubs.iter().find(|h| h.origin.as_deref() == Some(e.origin.as_str())) {
            Some(h) if h.kind == hubs::HubKind::Remote && h.directory.is_none() => {
                plan.adopt.push((h.id.clone(), e.clone()));
            }
            Some(_) => plan.skipped.push((e.id.clone(), "origin trùng với một hub đã có")),
            None => plan.add.push(e.clone()),
        }
    }
    for h in &reg.hubs {
        if h.directory.as_deref() == Some(dir_id) && !h.stale {
            if let Some(hid) = &h.hub_id {
                if !listed.contains(hid.as_str()) {
                    plan.stale.push(h.id.clone());
                }
            }
        }
    }
    plan
}

/// Apply a plan (except origin changes). New hubs go right after the directory's last hub.
pub fn apply_sync(reg: &mut Registry, dir_id: &str, plan: &SyncPlan) {
    for (id, e) in &plan.update {
        if let Some(h) = reg.get_mut(id) {
            let follows = h.dir_name.is_none() || h.dir_name.as_deref() == Some(h.name.as_str());
            if follows {
                h.name = e.name.clone();
            }
            h.dir_name = Some(e.name.clone());
            h.user = Some(e.user.clone()).filter(|u| !u.is_empty());
        }
    }
    for id in &plan.unstale {
        if let Some(h) = reg.get_mut(id) {
            h.stale = false;
        }
    }
    for id in &plan.stale {
        if let Some(h) = reg.get_mut(id) {
            h.stale = true;
        }
    }
    for (id, e) in &plan.adopt {
        if let Some(h) = reg.get_mut(id) {
            h.directory = Some(dir_id.to_string());
            h.hub_id = Some(e.id.clone());
            h.dir_name = Some(e.name.clone());
            h.user = Some(e.user.clone()).filter(|u| !u.is_empty());
            h.stale = false;
        }
    }
    let mut at = reg.hubs.iter().rposition(|h| h.directory.as_deref() == Some(dir_id)).map(|i| i + 1);
    for e in &plan.add {
        let mut h = Hub::new_remote(&dir_hub_reg_id(dir_id, &e.id), &e.name, &e.origin);
        h.directory = Some(dir_id.to_string());
        h.hub_id = Some(e.id.clone());
        h.dir_name = Some(e.name.clone());
        h.user = Some(e.user.clone()).filter(|u| !u.is_empty());
        if reg.get(&h.id).is_some() {
            continue;
        }
        match at {
            Some(i) => {
                reg.hubs.insert(i, h);
                at = Some(i + 1);
            }
            None => reg.hubs.push(h),
        }
    }
    if reg.active.is_none() {
        reg.active = reg.rail_order(false).into_iter().next();
    }
}

/// Accept (`true`) or refuse an origin change proposed by a directory.
pub fn resolve_origin_change(reg: &mut Registry, change: &OriginChange, accept: bool) -> Result<(), String> {
    let h = reg.get_mut(&change.hub).ok_or("Không tìm thấy hub")?;
    if h.origin.as_deref() != Some(change.from.as_str()) {
        return Err("Hub đã đổi từ lúc hỏi — hãy đồng bộ lại".into());
    }
    if accept {
        let o = validate_origin(&change.to).map_err(|e| e.message().to_string())?;
        h.origin = Some(o.as_string());
        h.last_url = None; // a path of the old site means nothing on the new one
        h.rejected_origin = None;
    } else {
        h.rejected_origin = Some(change.to.clone());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------- time

/// Seconds since the Unix epoch.
pub fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `YYYY-MM-DDTHH:MM:SSZ` (UTC) for a Unix timestamp.
pub fn iso_utc(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // civil_from_days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// Inverse of `iso_utc` (only that exact format).
pub fn parse_iso_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // days_from_civil
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Automatic resync period (PLAN §3.2).
pub const RESYNC_S: i64 = 6 * 3600;

/// A directory is due for an automatic resync when never synced or synced ≥ 6 h ago (or in the future: clock moved).
pub fn resync_due(synced_at: Option<&str>, now: i64) -> bool {
    match synced_at.and_then(parse_iso_utc) {
        Some(t) => now - t >= RESYNC_S || t > now + 300,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hubs::{Directory, HubKind};
    use serde_json::json;

    // -- text

    #[test]
    fn clean_line_strips_controls_bidi_invisible() {
        assert_eq!(clean_line("  An\n— dự\tán  A  ", 60), "An — dự án A");
        assert_eq!(clean_line("ab\u{202E}cd\u{2066}e\u{200B}f\u{FEFF}g\u{0007}h", 60), "abcdefgh");
        assert_eq!(clean_line("x\u{2028}y\u{2029}z", 60), "x y z"); // separators are whitespace → space
        assert_eq!(clean_line("\u{E0041}tag\u{061C}", 60), "tag");
        assert_eq!(clean_line("\u{1b}[31mred", 60), "[31mred");
        assert_eq!(clean_line(&"ă".repeat(80), 60).chars().count(), 60);
        assert_eq!(clean_line("abc def", 4), "abc"); // no trailing space after the cut
        assert_eq!(clean_line("\u{200F}\u{200E}", 60), "");
    }

    // -- origins

    #[test]
    fn good_origins() {
        for (input, want) in [
            ("https://an.example.com", "https://an.example.com"),
            ("https://An.Example.COM/", "https://an.example.com"),
            ("https://an.example.com:443", "https://an.example.com"),
            ("https://an.example.com:8443", "https://an.example.com:8443"),
            ("  https://hub-1.tail1234.ts.net ", "https://hub-1.tail1234.ts.net"),
            ("https://xn--bcher-kva.example", "https://xn--bcher-kva.example"),
        ] {
            assert_eq!(validate_origin(input).map(|o| o.as_string()), Ok(want.to_string()), "{input}");
        }
        // IDN is converted to punycode by the URL parser, then accepted
        assert_eq!(validate_origin("https://bücher.example").unwrap().as_string(), "https://xn--bcher-kva.example");
    }

    #[test]
    fn bad_origins() {
        use OriginError::*;
        for (input, want) in [
            ("http://an.example.com", NotHttps),
            ("ftp://an.example.com", Malformed),
            ("https://1.2.3.4", IpHost),
            ("https://[::1]", IpHost),
            ("https://[2001:db8::1]:8443", IpHost),
            ("https://0x7f.0.0.1", IpHost), // parsed as IPv4 by the WHATWG parser
            ("https://localhost", LocalHost),
            ("https://cfharness.localhost:8711", LocalHost),
            ("https://an.example.com/path", HasPath),
            ("https://an.example.com/?x=1", HasPath),
            ("https://an.example.com/#k=1", HasPath),
            ("https://u:p@an.example.com", HasUserinfo),
            ("https://user@an.example.com", HasUserinfo),
            ("https://intranet", BadHost),
            ("https://-bad.example.com", BadHost),
            ("https://bad_host.example.com", BadHost),
            ("https://an.example.com\u{202E}", Malformed),
            ("https://an .example.com", Malformed),
            ("not a url", Malformed),
            ("", Malformed),
        ] {
            assert_eq!(validate_origin(input).map(|o| o.as_string()), Err(want), "{input:?}");
        }
    }

    #[test]
    fn hub_ids() {
        assert_eq!(valid_hub_id("a1B2c3D4").as_deref(), Some("a1b2c3d4"));
        assert_eq!(valid_hub_id("0123456789abcdef").as_deref(), Some("0123456789abcdef"));
        assert_eq!(valid_hub_id("0123456789abcdef0"), None); // 17
        assert_eq!(valid_hub_id(""), None);
        assert_eq!(valid_hub_id("xyz"), None);
        assert_eq!(valid_hub_id("../a"), None);
        assert_eq!(valid_hub_id("a1 b2"), None);
    }

    // -- input classification

    #[test]
    fn classify_hub_and_directory() {
        let hub = |s| match classify_input(s) {
            Ok(ImportTarget::Hub(o)) => o.as_string(),
            other => panic!("{s}: {other:?}"),
        };
        assert_eq!(hub("https://an.example.com"), "https://an.example.com");
        assert_eq!(hub("an.example.com"), "https://an.example.com"); // scheme assumed
        assert_eq!(hub("https://an.example.com/.well-known/cfharness.json"), "https://an.example.com");
        assert_eq!(hub("https://an.example.com/#frag"), "https://an.example.com");

        match classify_input("https://admin.example.com/.well-known/cfharness-directory?t=s3cr3t-T0k&x=1#f").unwrap() {
            ImportTarget::Directory { url, token } => {
                assert_eq!(token.as_deref(), Some("s3cr3t-T0k"));
                assert_eq!(url.as_str(), "https://admin.example.com/.well-known/cfharness-directory?x=1");
                assert!(!url.as_str().contains("s3cr3t"));
                assert_eq!(with_token(&url, token.as_deref()).query(), Some("x=1&t=s3cr3t-T0k"));
                assert_eq!(display_url(&url), "admin.example.com/.well-known/cfharness-directory");
            }
            other => panic!("{other:?}"),
        }
        match classify_input("https://files.example.com/team/hubs.json").unwrap() {
            ImportTarget::Directory { url, token } => {
                assert_eq!(token, None);
                assert_eq!(url.query(), None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn classify_rejects() {
        assert_eq!(classify_input("  "), Err(InputError::Empty));
        assert_eq!(classify_input("http://an.example.com"), Err(InputError::Origin(OriginError::NotHttps)));
        assert_eq!(classify_input("http://admin.example.com/dir?t=x"), Err(InputError::Origin(OriginError::NotHttps)));
        assert_eq!(classify_input("https://10.0.0.5/dir"), Err(InputError::Origin(OriginError::IpHost)));
        assert_eq!(classify_input("192.168.1.10"), Err(InputError::Origin(OriginError::IpHost)));
        assert_eq!(classify_input("https://a.example.com/d?t="), Err(InputError::BadToken));
        assert_eq!(classify_input("https://a.example.com/d?t=%20x"), Err(InputError::BadToken));
        assert_eq!(classify_input("javascript:alert(1)"), Err(InputError::Origin(OriginError::Malformed)));
    }

    // -- C2

    #[test]
    fn hub_meta() {
        assert_eq!(
            parse_hub_meta(r#"{"kind":"cfharness.hub","schema":1,"app_api":1}"#),
            Some(HubMeta { schema: 1, app_api: 1 })
        );
        assert_eq!(parse_hub_meta(r#"{"kind":"cfharness.hub","schema":0}"#), None);
        assert_eq!(parse_hub_meta(r#"{"kind":"other","schema":1}"#), None);
        assert_eq!(parse_hub_meta("<html>"), None);
    }

    // -- directory parsing

    fn doc(hubs: Value) -> String {
        json!({"kind":"cfharness.directory","schema":1,"name":"Mac mini công ty","generated_at":"2026-11-15T10:00:00Z","hubs":hubs})
            .to_string()
    }

    #[test]
    fn parse_good_directory() {
        let text = doc(json!([
            {"id":"a1b2c3d4","name":"An — dự án A","user":"an","origin":"https://an.example.com"},
            {"id":"E5F6A7B8","name":"","user":"binh","origin":"https://binh.example.com:8443/"},
            {"id":"0f","user":"","origin":"https://c.example.com","extra":"ignored"}
        ]));
        let (d, w) = parse_directory(&text, "admin.example.com").unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(d.name, "Mac mini công ty");
        assert_eq!(d.generated_at.as_deref(), Some("2026-11-15T10:00:00Z"));
        assert_eq!(d.hubs.len(), 3);
        assert_eq!(
            d.hubs[1],
            DirHub {
                id: "e5f6a7b8".into(),
                name: "binh".into(),
                user: "binh".into(),
                origin: "https://binh.example.com:8443".into()
            }
        );
        assert_eq!(d.hubs[2].name, "c.example.com");
    }

    #[test]
    fn parse_drops_bad_entries_with_warnings() {
        let text = doc(json!([
            {"id":"a1","name":"ok","origin":"https://a.example.com"},
            {"id":"zz","name":"bad id","origin":"https://b.example.com"},
            {"id":"0123456789abcdef0","origin":"https://b.example.com"},
            {"id":"b2","origin":"http://b.example.com"},
            {"id":"b3","origin":"https://1.2.3.4"},
            {"id":"b4","origin":"https://b.example.com/x"},
            {"id":"b5"},
            "not an object",
            {"id":"a1","origin":"https://other.example.com"},
            {"id":"c1","origin":"https://A.example.com/"},
            {"id":"d1","name":"Evil\u{202E}name\nwith\u{0000}junk","user":"u\u{200B}ser\u{2066}","origin":"https://d.example.com"}
        ]));
        let (d, w) = parse_directory(&text, "x").unwrap();
        assert_eq!(d.hubs.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["a1", "d1"]);
        assert_eq!(d.hubs[1].name, "Evilname withjunk");
        assert_eq!(d.hubs[1].user, "user");
        assert_eq!(w.iter().map(|x| x.index).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(w[2].reason, OriginError::NotHttps.message());
        assert_eq!(w[3].reason, OriginError::IpHost.message());
        assert_eq!(w[7].reason, "id trùng với mục trước");
        assert_eq!(w[8].reason, "origin trùng với mục trước");
    }

    #[test]
    fn parse_limits_and_document_errors() {
        let many: Vec<Value> =
            (0..201).map(|i| json!({"id": format!("{i:x}"), "origin": format!("https://h{i}.example.com")})).collect();
        assert_eq!(parse_directory(&doc(Value::Array(many.clone())), "x"), Err(DirError::TooMany(201)));
        let (d, _) = parse_directory(&doc(Value::Array(many[..200].to_vec())), "x").unwrap();
        assert_eq!(d.hubs.len(), 200);

        assert_eq!(parse_directory("{", "x"), Err(DirError::NotJson));
        assert_eq!(parse_directory("[]", "x"), Err(DirError::NotDirectory));
        assert_eq!(parse_directory(r#"{"kind":"cfharness.hub","schema":1}"#, "x"), Err(DirError::NotDirectory));
        assert_eq!(
            parse_directory(r#"{"kind":"cfharness.directory","schema":2,"hubs":[]}"#, "x"),
            Err(DirError::SchemaTooNew(2))
        );
        assert_eq!(
            parse_directory(r#"{"kind":"cfharness.directory","schema":"1","hubs":[]}"#, "x"),
            Err(DirError::BadSchema)
        );
        assert_eq!(parse_directory(r#"{"kind":"cfharness.directory","schema":1}"#, "x"), Err(DirError::NotDirectory));
        let (d, _) = parse_directory(
            "{\"kind\":\"cfharness.directory\",\"schema\":1,\"name\":\" \u{202e} \",\"hubs\":[]}",
            "admin.example.com",
        )
        .unwrap();
        assert_eq!(d.name, "admin.example.com");
        let long = format!(r#"{{"kind":"cfharness.directory","schema":1,"name":"{}","hubs":[]}}"#, "n".repeat(100));
        assert_eq!(parse_directory(&long, "x").unwrap().0.name.len(), NAME_MAX);
    }

    // -- resync

    fn registry_with_dir(dir: &str) -> Registry {
        let mut reg = Registry::default();
        reg.directories.push(Directory {
            id: dir.into(),
            name: "D".into(),
            url: "https://admin.example.com/d".into(),
            token_ref: None,
            etag: None,
            synced_at: None,
            dismissed: vec![],
            extra: Default::default(),
        });
        reg
    }

    fn dh(id: &str, name: &str, origin: &str) -> DirHub {
        DirHub { id: id.into(), name: name.into(), user: "u".into(), origin: origin.into() }
    }

    fn sync(reg: &mut Registry, dir: &str, hubs: Vec<DirHub>) -> SyncPlan {
        let d = DirDoc { name: "D".into(), generated_at: None, hubs };
        let plan = plan_sync(reg, dir, &d);
        apply_sync(reg, dir, &plan);
        plan
    }

    #[test]
    fn first_sync_adds_everything_grouped() {
        let mut reg = registry_with_dir("dir-1");
        reg.merge_local(&[std::path::PathBuf::from("/Users/x/.cfharness")]);
        let p = sync(
            &mut reg,
            "dir-1",
            vec![dh("a1", "An", "https://a.example.com"), dh("b2", "Bình", "https://b.example.com")],
        );
        assert_eq!(p.add.len(), 2);
        assert_eq!(reg.hubs.len(), 3);
        let a = reg.get("rem-1-a1").unwrap();
        assert_eq!((a.kind, a.directory.as_deref(), a.hub_id.as_deref()), (HubKind::Remote, Some("dir-1"), Some("a1")));
        assert_eq!(a.origin.as_deref(), Some("https://a.example.com"));
        assert_eq!(
            reg.rail_order(false),
            vec![hubs::local_id("/Users/x/.cfharness"), "rem-1-a1".into(), "rem-1-b2".into()]
        );
        // second identical sync: nothing to do
        let p = sync(
            &mut reg,
            "dir-1",
            vec![dh("a1", "An", "https://a.example.com"), dh("b2", "Bình", "https://b.example.com")],
        );
        assert!(p.is_noop() && p.origin_changes.is_empty(), "{p:?}");
    }

    #[test]
    fn missing_hub_becomes_stale_not_deleted_and_comes_back() {
        let mut reg = registry_with_dir("dir-1");
        sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://a.example.com"), dh("b2", "B", "https://b.example.com")]);
        let p = sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://a.example.com")]);
        assert_eq!(p.stale, vec!["rem-1-b2".to_string()]);
        assert!(reg.get("rem-1-b2").unwrap().stale);
        assert_eq!(reg.hubs.len(), 2);
        let p = sync(
            &mut reg,
            "dir-1",
            vec![dh("a1", "An", "https://a.example.com"), dh("b2", "B", "https://b.example.com")],
        );
        assert_eq!(p.unstale, vec!["rem-1-b2".to_string()]);
        assert!(!reg.get("rem-1-b2").unwrap().stale);
    }

    #[test]
    fn origin_change_is_detected_never_applied_silently() {
        let mut reg = registry_with_dir("dir-1");
        sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://a.example.com")]);
        reg.get_mut("rem-1-a1").unwrap().last_url = Some("/manager".into());
        let p = sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://evil.example.net")]);
        assert_eq!(
            p.origin_changes,
            vec![OriginChange {
                hub: "rem-1-a1".into(),
                directory: "dir-1".into(),
                name: "An".into(),
                from: "https://a.example.com".into(),
                to: "https://evil.example.net".into()
            }]
        );
        assert_eq!(reg.get("rem-1-a1").unwrap().origin.as_deref(), Some("https://a.example.com"));

        // refuse → remembered, not asked again for the same value
        resolve_origin_change(&mut reg, &p.origin_changes[0], false).unwrap();
        let p2 = sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://evil.example.net")]);
        assert!(p2.origin_changes.is_empty());
        // a different new value is asked again
        let p3 = sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://a2.example.com")]);
        assert_eq!(p3.origin_changes.len(), 1);
        // accept → origin changed, last_url dropped
        resolve_origin_change(&mut reg, &p3.origin_changes[0], true).unwrap();
        let h = reg.get("rem-1-a1").unwrap();
        assert_eq!(h.origin.as_deref(), Some("https://a2.example.com"));
        assert_eq!(h.last_url, None);
        // a stale answer (hub changed meanwhile) is refused
        assert!(resolve_origin_change(&mut reg, &p3.origin_changes[0], true).is_err());
    }

    #[test]
    fn names_follow_directory_until_renamed() {
        let mut reg = registry_with_dir("dir-1");
        sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://a.example.com")]);
        sync(&mut reg, "dir-1", vec![dh("a1", "An (mới)", "https://a.example.com")]);
        assert_eq!(reg.get("rem-1-a1").unwrap().name, "An (mới)");
        reg.rename("rem-1-a1", "Của tôi").unwrap();
        sync(&mut reg, "dir-1", vec![dh("a1", "An v3", "https://a.example.com")]);
        let h = reg.get("rem-1-a1").unwrap();
        assert_eq!(h.name, "Của tôi");
        assert_eq!(h.dir_name.as_deref(), Some("An v3"));
        // clearing the custom name goes back to the directory name
        reg.rename("rem-1-a1", "  ").unwrap();
        assert_eq!(reg.get("rem-1-a1").unwrap().name, "An v3");
    }

    #[test]
    fn dismissed_hubs_are_not_re_added_and_duplicates_skipped() {
        let mut reg = registry_with_dir("dir-1");
        sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://a.example.com")]);
        reg.remove("rem-1-a1").unwrap();
        assert_eq!(reg.directory("dir-1").unwrap().dismissed, vec!["a1".to_string()]);
        let p = sync(&mut reg, "dir-1", vec![dh("a1", "An", "https://a.example.com")]);
        assert!(p.add.is_empty());
        assert_eq!(p.skipped.len(), 1);

        // a hub added by origin earlier is adopted (same id, same session)
        reg.hubs.push(Hub::new_remote("rem-x", "Tự thêm", "https://c.example.com"));
        // a hub of another directory with the same origin is not duplicated
        let mut other = Hub::new_remote("rem-2-d4", "D", "https://d.example.com");
        other.directory = Some("dir-2".into());
        reg.hubs.push(other);
        let p = sync(
            &mut reg,
            "dir-1",
            vec![dh("c3", "C", "https://c.example.com"), dh("d4", "D", "https://d.example.com")],
        );
        assert_eq!(p.adopt.len(), 1);
        assert_eq!(p.skipped.last().unwrap().1, "origin trùng với một hub đã có");
        let c = reg.get("rem-x").unwrap();
        assert_eq!(
            (c.directory.as_deref(), c.hub_id.as_deref(), c.name.as_str()),
            (Some("dir-1"), Some("c3"), "Tự thêm")
        );
    }

    #[test]
    fn ids_are_stable_and_valid() {
        let u = Url::parse("https://admin.example.com/.well-known/cfharness-directory").unwrap();
        let d = directory_id(&u);
        assert!(d.starts_with("dir-") && d.len() == 12);
        let h = dir_hub_reg_id(&d, "0123456789abcdef");
        assert!(h.len() <= 64 && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert_eq!(
            origin_reg_id(&validate_origin("https://a.example.com/").unwrap()),
            origin_reg_id(&validate_origin("https://A.example.com").unwrap())
        );
    }

    #[test]
    fn time_round_trip_and_due() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_763_200_800), "2025-11-15T10:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        for t in [0, 951_782_400, 1_763_200_800, 4_102_444_799] {
            assert_eq!(parse_iso_utc(&iso_utc(t)), Some(t));
        }
        assert_eq!(parse_iso_utc("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_iso_utc("garbage"), None);
        let now = 1_763_200_800;
        assert!(resync_due(None, now));
        assert!(resync_due(Some(&iso_utc(now - RESYNC_S)), now));
        assert!(!resync_due(Some(&iso_utc(now - 60)), now));
        assert!(resync_due(Some(&iso_utc(now + 3600)), now)); // clock went backwards
    }
}
