//! Tauri glue for cloud hubs (PLAN §3.2, §3.3): import dialog (inspect → confirm), directory resync (manual + every
//! 6 h with ETag), reachability probes, origin-change answers, hub/directory removal (optionally wiping the hub's
//! webview data store). Validation rules live in `directory.rs`; HTTP in `fetch.rs`; tokens in `secrets.rs`.

use crate::core::{Core, PendingImport, PendingKind};
use crate::directory::{self, DirDoc, DirError, ImportTarget, OriginChange, Warning};
use crate::fetch::{FetchError, Fetched, Reach};
use crate::hubs::{Directory, Hub, HubKind};
use crate::nav::Origin;
use crate::secrets;
use crate::webviews;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// How often the background loop wakes up (resync is due every 6 h; probes run each wake-up).
const LOOP_EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(Serialize, Clone, Debug)]
pub struct PreviewHub {
    pub id: String,
    pub name: String,
    pub user: String,
    pub origin: String,
    /// new | exists | adopt | skip
    pub state: &'static str,
    pub reason: Option<&'static str>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ImportPreview {
    pub nonce: u64,
    /// hub | directory
    pub kind: &'static str,
    pub name: String,
    pub origin: Option<String>,
    /// Directory URL for display (host + path, no token).
    pub url: Option<String>,
    pub has_token: bool,
    pub hubs: Vec<PreviewHub>,
    pub warnings: Vec<Warning>,
    /// Registry id of the hub/directory that already exists for this input.
    pub existing: Option<String>,
    /// The hub answered C2 (false: an older hub without `/.well-known/cfharness.json` — still usable).
    pub verified: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct SyncSummary {
    pub added: usize,
    pub adopted: usize,
    pub updated: usize,
    pub stale: usize,
    pub skipped: usize,
    pub origin_changes: usize,
    pub not_modified: bool,
    /// Hub to select after a single-hub import.
    pub select: Option<String>,
}

fn err_msg(e: FetchError) -> String {
    e.message()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------- import

/// "Kiểm tra": classify the URL, fetch C2 or the directory, validate, keep the result until "Thêm".
pub async fn inspect(app: &AppHandle, input: &str) -> Result<ImportPreview, String> {
    let target = directory::classify_input(input).map_err(|e| e.message())?;
    let core = app.state::<Core>();
    match target {
        ImportTarget::Hub(origin) => inspect_hub(app, origin, false).await,
        ImportTarget::Directory { url, token } => {
            let req = directory::with_token(&url, token.as_deref());
            log::info!("import: tải directory {}", directory::display_url(&url));
            let fetched = core.fetcher.get(&req, None).await;
            let (text, etag) = match fetched {
                Ok(Fetched::Body { text, etag }) => (text, etag),
                Ok(Fetched::NotModified) => return Err("Máy chủ trả 304 khi chưa gửi ETag".into()),
                // A hub page URL (https://an.example.com/manager) is not a directory: try the origin as a hub.
                Err(FetchError::Status(404)) if token.is_none() => {
                    let origin = Origin::of(&url).ok_or("URL không hợp lệ")?;
                    return inspect_hub(app, origin, true).await;
                }
                Err(e) => return Err(err_msg(e)),
            };
            let host = url.host_str().unwrap_or_default().to_string();
            let (doc, warnings) = match directory::parse_directory(&text, &host) {
                Ok(x) => x,
                Err(DirError::NotJson | DirError::NotDirectory) if token.is_none() => {
                    let origin = Origin::of(&url).ok_or("URL không hợp lệ")?;
                    return inspect_hub(app, origin, true).await;
                }
                Err(e) => return Err(e.message()),
            };
            let dir_id = directory::directory_id(&url);
            let mut g = core.lock();
            let plan = directory::plan_sync(&g.reg, &dir_id, &doc);
            let existing_dir = g.reg.directory(&dir_id).map(|d| d.id.clone());
            let hubs_preview = doc
                .hubs
                .iter()
                .map(|e| {
                    let (state, reason) = if plan.add.iter().any(|x| x.id == e.id) {
                        ("new", None)
                    } else if plan.adopt.iter().any(|(_, x)| x.id == e.id) {
                        ("adopt", None)
                    } else if let Some((_, r)) = plan.skipped.iter().find(|(id, _)| id == &e.id) {
                        ("skip", Some(*r))
                    } else {
                        ("exists", None)
                    };
                    PreviewHub {
                        id: e.id.clone(),
                        name: e.name.clone(),
                        user: e.user.clone(),
                        origin: e.origin.clone(),
                        state,
                        reason,
                    }
                })
                .collect();
            g.import_seq += 1;
            let nonce = g.import_seq;
            g.pending_import = Some(PendingImport {
                nonce,
                kind: PendingKind::Directory {
                    url: url.clone(),
                    token: token.clone(),
                    name: doc.name.clone(),
                    hubs: doc.hubs.clone(),
                    etag,
                },
            });
            Ok(ImportPreview {
                nonce,
                kind: "directory",
                name: doc.name,
                origin: None,
                url: Some(directory::display_url(&url)),
                has_token: token.is_some(),
                hubs: hubs_preview,
                warnings,
                existing: existing_dir,
                verified: true,
            })
        }
    }
}

async fn inspect_hub(app: &AppHandle, origin: Origin, from_page: bool) -> Result<ImportPreview, String> {
    let core = app.state::<Core>();
    let verified = match core.fetcher.hub_meta(&origin).await {
        Ok(Some(_)) => true,
        Ok(None) | Err(FetchError::Status(404)) if !from_page => false,
        Ok(None) | Err(FetchError::Status(_)) => {
            return Err("URL này không phải directory cfharness và origin của nó không trả lời như một hub".into())
        }
        Err(e) => return Err(err_msg(e)),
    };
    let os = origin.as_string();
    let mut g = core.lock();
    let existing = g.reg.hubs.iter().find(|h| h.origin.as_deref() == Some(os.as_str())).map(|h| h.id.clone());
    g.import_seq += 1;
    let nonce = g.import_seq;
    g.pending_import = Some(PendingImport { nonce, kind: PendingKind::Hub { origin: origin.clone() } });
    Ok(ImportPreview {
        nonce,
        kind: "hub",
        name: origin.host.clone(),
        origin: Some(os),
        url: None,
        has_token: false,
        hubs: vec![],
        warnings: vec![],
        existing,
        verified,
    })
}

/// "Thêm": apply the inspected result. Directory token → OS keychain first; on failure nothing is written.
pub async fn confirm(app: &AppHandle, nonce: u64, name: Option<String>) -> Result<SyncSummary, String> {
    let core = app.state::<Core>();
    let pending = {
        let mut g = core.lock();
        match g.pending_import.take() {
            Some(p) if p.nonce == nonce => p,
            other => {
                g.pending_import = other;
                return Err("Kết quả kiểm tra đã cũ — hãy bấm Kiểm tra lại".into());
            }
        }
    };
    let summary = match pending.kind {
        PendingKind::Hub { origin } => {
            let os = origin.as_string();
            let mut g = core.lock();
            let id = match g.reg.hubs.iter().find(|h| h.origin.as_deref() == Some(os.as_str())) {
                Some(h) => h.id.clone(),
                None => {
                    let id = directory::origin_reg_id(&origin);
                    let display = name.map(|n| directory::clean_line(&n, directory::NAME_MAX)).unwrap_or_default();
                    g.reg.hubs.push(Hub::new_remote(&id, &display, &os));
                    id
                }
            };
            core.persist(&g);
            SyncSummary { added: 1, select: Some(id), ..Default::default() }
        }
        PendingKind::Directory { url, token, name: dname, hubs: dhubs, etag } => {
            let dir_id = directory::directory_id(&url);
            let key = secrets::token_key(&dir_id);
            let tok = token.clone();
            let k2 = key.clone();
            let core2 = app.clone();
            blocking(move || {
                let core = core2.state::<Core>();
                match tok {
                    Some(t) => core.tokens.set(&k2, &t),
                    None => core.tokens.delete(&k2),
                }
            })
            .await??;
            let doc = DirDoc { name: dname.clone(), generated_at: None, hubs: dhubs };
            let mut g = core.lock();
            let token_ref = token.as_ref().map(|_| secrets::token_ref(&dir_id));
            let now = directory::iso_utc(directory::now_s());
            match g.reg.directory_mut(&dir_id) {
                Some(d) => {
                    d.name = dname;
                    d.url = url.to_string();
                    d.token_ref = token_ref;
                    d.etag = etag;
                    d.synced_at = Some(now);
                    d.dismissed.clear(); // re-importing brings removed hubs back
                }
                None => g.reg.directories.push(Directory {
                    id: dir_id.clone(),
                    name: dname,
                    url: url.to_string(),
                    token_ref,
                    etag,
                    synced_at: Some(now),
                    dismissed: vec![],
                    extra: Default::default(),
                }),
            }
            let summary = apply_doc(&mut g, &dir_id, &doc);
            g.dir_errors.remove(&dir_id);
            core.persist(&g);
            log::info!(
                "import directory {}: +{} hub, gộp {}, bỏ qua {}",
                directory::display_url(&url),
                summary.added,
                summary.adopted,
                summary.skipped
            );
            summary
        }
    };
    webviews::emit_state(app);
    let a = app.clone();
    tauri::async_runtime::spawn(async move { probe_all(&a).await });
    Ok(summary)
}

/// Plan + apply a validated document; replace this directory's pending origin changes.
fn apply_doc(g: &mut crate::core::Inner, dir_id: &str, doc: &DirDoc) -> SyncSummary {
    let plan = directory::plan_sync(&g.reg, dir_id, doc);
    directory::apply_sync(&mut g.reg, dir_id, &plan);
    g.origin_changes.retain(|c| c.directory != dir_id);
    g.origin_changes.extend(plan.origin_changes.iter().cloned());
    SyncSummary {
        added: plan.add.len(),
        adopted: plan.adopt.len(),
        updated: plan.update.len(),
        stale: plan.stale.len(),
        skipped: plan.skipped.len(),
        origin_changes: plan.origin_changes.len(),
        not_modified: false,
        select: None,
    }
}

// ---------------------------------------------------------------------------------------------- resync

pub async fn sync_directory(app: &AppHandle, dir_id: &str) -> Result<SyncSummary, String> {
    let core = app.state::<Core>();
    let (url, token_ref, etag) = {
        let mut g = core.lock();
        let d = g.reg.directory(dir_id).ok_or("Không tìm thấy directory")?.clone();
        if !g.syncing.insert(dir_id.to_string()) {
            return Err("Directory đang được đồng bộ".into());
        }
        (d.url, d.token_ref, d.etag)
    };
    webviews::emit_state(app);
    let res = sync_inner(app, dir_id, &url, token_ref, etag).await;
    {
        let mut g = core.lock();
        g.syncing.remove(dir_id);
        match &res {
            Ok(_) => {
                g.dir_errors.remove(dir_id);
            }
            Err(e) => {
                g.dir_errors.insert(dir_id.to_string(), e.clone());
            }
        }
    }
    webviews::emit_state(app);
    if res.as_ref().is_ok_and(|s| s.added + s.adopted > 0) {
        let a = app.clone();
        tauri::async_runtime::spawn(async move { probe_all(&a).await });
    }
    res
}

async fn sync_inner(
    app: &AppHandle,
    dir_id: &str,
    url_s: &str,
    token_ref: Option<String>,
    etag: Option<String>,
) -> Result<SyncSummary, String> {
    let core = app.state::<Core>();
    let url = url::Url::parse(url_s).map_err(|_| "URL directory trong hubs.json không hợp lệ".to_string())?;
    // Re-check the stored URL with the same rules as the import (hubs.json is user-editable).
    match directory::classify_input(url.as_str()) {
        Ok(ImportTarget::Directory { token: None, .. }) => {}
        _ => return Err("URL directory trong hubs.json không hợp lệ (chỉ https, không IP, không token)".into()),
    }
    let token = match token_ref {
        Some(r) => {
            let key = secrets::key_of_ref(&r).ok_or("token_ref không hợp lệ")?;
            let a = app.clone();
            let t = blocking(move || a.state::<Core>().tokens.get(&key)).await??;
            Some(t.ok_or("Token của directory không còn trong kho bí mật — hãy import lại URL có token")?)
        }
        None => None,
    };
    let req = directory::with_token(&url, token.as_deref());
    let now = directory::iso_utc(directory::now_s());
    match core.fetcher.get(&req, etag.as_deref()).await.map_err(err_msg)? {
        Fetched::NotModified => {
            let mut g = core.lock();
            if let Some(d) = g.reg.directory_mut(dir_id) {
                d.synced_at = Some(now);
            }
            core.persist(&g);
            Ok(SyncSummary { not_modified: true, ..Default::default() })
        }
        Fetched::Body { text, etag } => {
            let host = url.host_str().unwrap_or_default().to_string();
            let (doc, warnings) = directory::parse_directory(&text, &host).map_err(|e| e.message())?;
            if !warnings.is_empty() {
                log::warn!("directory {dir_id}: bỏ {} mục không hợp lệ", warnings.len());
            }
            let mut g = core.lock();
            let summary = apply_doc(&mut g, dir_id, &doc);
            if let Some(d) = g.reg.directory_mut(dir_id) {
                d.name = doc.name.clone();
                d.etag = etag;
                d.synced_at = Some(now);
            }
            core.persist(&g);
            log::info!(
                "đồng bộ directory {dir_id}: +{} ~{} cũ {} đổi origin {}",
                summary.added,
                summary.updated,
                summary.stale,
                summary.origin_changes
            );
            Ok(summary)
        }
    }
}

/// Resync every directory (menu "Đồng bộ lại").
pub async fn sync_all(app: &AppHandle) {
    let ids: Vec<String> = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.directories.iter().map(|d| d.id.clone()).collect()
    };
    for id in ids {
        if let Err(e) = sync_directory(app, &id).await {
            log::warn!("đồng bộ directory {id} lỗi: {e}");
        }
    }
}

// ---------------------------------------------------------------------------------------------- probes

pub async fn probe_one(app: &AppHandle, id: &str) -> Option<Reach> {
    let core = app.state::<Core>();
    let origin = {
        let g = core.lock();
        let h = g.reg.get(id)?;
        if h.kind != HubKind::Remote {
            return None;
        }
        Origin::parse(h.origin.as_deref()?)?
    };
    let r = core.fetcher.probe(&origin).await;
    core.lock().reach.insert(id.to_string(), r);
    Some(r)
}

pub async fn probe_all(app: &AppHandle) {
    let ids: Vec<String> = {
        let core = app.state::<Core>();
        let g = core.lock();
        g.reg.hubs.iter().filter(|h| h.kind == HubKind::Remote && !h.hidden).map(|h| h.id.clone()).collect()
    };
    // Bounded concurrency: 8 at a time.
    for chunk in ids.chunks(8) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|id| {
                let (a, id) = (app.clone(), id.clone());
                tauri::async_runtime::spawn(async move { probe_one(&a, &id).await })
            })
            .collect();
        for h in handles {
            let _ = h.await;
        }
    }
    webviews::emit_state(app);
}

/// Background loop: automatic resync of due directories (6 h, ETag) + reachability probes.
pub fn start_background(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            let due: Vec<String> = {
                let core = app.state::<Core>();
                let g = core.lock();
                let now = directory::now_s();
                g.reg
                    .directories
                    .iter()
                    .filter(|d| directory::resync_due(d.synced_at.as_deref(), now) && !g.syncing.contains(&d.id))
                    .map(|d| d.id.clone())
                    .collect()
            };
            for id in due {
                if let Err(e) = sync_directory(&app, &id).await {
                    log::warn!("tự đồng bộ directory {id} lỗi: {e}");
                }
            }
            probe_all(&app).await;
            tokio::time::sleep(LOOP_EVERY).await;
        }
    });
}

// ---------------------------------------------------------------------------------------------- answers / removal

pub fn resolve_origin(app: &AppHandle, hub: &str, to: &str, accept: bool) -> Result<(), String> {
    let core = app.state::<Core>();
    let changed = {
        let mut g = core.lock();
        let change: OriginChange = g
            .origin_changes
            .iter()
            .find(|c| c.hub == hub && c.to == to)
            .cloned()
            .ok_or("Không còn yêu cầu đổi địa chỉ này")?;
        directory::resolve_origin_change(&mut g.reg, &change, accept)?;
        g.origin_changes.retain(|c| c.hub != hub);
        if accept {
            g.origins.remove(hub);
            g.reach.remove(hub);
        }
        core.persist(&g);
        accept
    };
    log::info!("hub {hub}: đổi origin theo directory → {}", if changed { "đồng ý" } else { "từ chối" });
    if changed {
        // The live webview still points at the old origin: drop it, it reopens on the new one when selected.
        close_webview(app, hub);
    }
    webviews::emit_state(app);
    Ok(())
}

fn close_webview(app: &AppHandle, id: &str) {
    if let Some(wv) = app.get_webview(&webviews::label_of(id)) {
        let _ = wv.close();
    }
    let core = app.state::<Core>();
    let mut g = core.lock();
    g.lru.retain(|x| x != id);
    g.status.remove(id);
}

/// Delete the hub's webview data store (cookies, local storage — i.e. its login session).
pub async fn wipe_store(app: &AppHandle, id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // WebKit refuses to remove a store still in use: give the closed webview a moment.
        let mut last = String::new();
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            match app.remove_data_store(crate::hubs::data_store_id(id)).await {
                Ok(()) => return Ok(()),
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!("Không xoá được dữ liệu phiên của hub: {last}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let dir = app.state::<Core>().data_dir.join("webviews").join(id);
        // WebView2 keeps files open for a short while after the controller closes.
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(_) => continue,
            }
        }
        Err("Không xoá được dữ liệu phiên của hub (tệp đang được dùng) — thử lại sau khi khởi động lại app".into())
    }
}

/// Remove a remote hub from the app; `wipe` also deletes its webview data store.
pub async fn remove_hub(app: &AppHandle, id: &str, wipe: bool) -> Result<(), String> {
    let core = app.state::<Core>();
    {
        let g = core.lock();
        let h = g.reg.get(id).ok_or("Không tìm thấy hub")?;
        if h.kind == HubKind::Local {
            return Err("Hub local được phát hiện tự động — chỉ ẩn được, không xoá được".into());
        }
    }
    close_webview(app, id);
    let new_active = {
        let mut g = core.lock();
        let was_active = g.reg.active.as_deref() == Some(id);
        g.reg.remove(id)?;
        g.origins.remove(id);
        g.reach.remove(id);
        g.zoom.remove(id);
        g.origin_changes.retain(|c| c.hub != id);
        core.persist(&g);
        if was_active {
            g.reg.active.clone()
        } else {
            None
        }
    };
    log::info!("hub {id}: đã xoá khỏi app (xoá dữ liệu phiên: {wipe})");
    webviews::emit_state(app);
    if let Some(a) = new_active {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = webviews::select(app2, a).await;
        });
    } else {
        webviews::layout(app);
    }
    if wipe {
        wipe_store(app, id).await?;
    }
    Ok(())
}

/// Remove a directory: its token is deleted from the keychain; its hubs are removed (`remove_hubs`) or kept as
/// standalone hubs.
pub async fn remove_directory(app: &AppHandle, dir_id: &str, remove_hubs: bool, wipe: bool) -> Result<(), String> {
    let core = app.state::<Core>();
    let (token_ref, members) = {
        let g = core.lock();
        let d = g.reg.directory(dir_id).ok_or("Không tìm thấy directory")?;
        let members: Vec<String> =
            g.reg.hubs.iter().filter(|h| h.directory.as_deref() == Some(dir_id)).map(|h| h.id.clone()).collect();
        (d.token_ref.clone(), members)
    };
    if let Some(key) = token_ref.as_deref().and_then(secrets::key_of_ref) {
        let a = app.clone();
        blocking(move || a.state::<Core>().tokens.delete(&key)).await??;
    }
    if remove_hubs {
        for id in &members {
            remove_hub(app, id, wipe).await?;
        }
    }
    {
        let mut g = core.lock();
        for h in g.reg.hubs.iter_mut().filter(|h| h.directory.as_deref() == Some(dir_id)) {
            h.directory = None;
            h.hub_id = None;
            h.stale = false;
        }
        g.reg.directories.retain(|d| d.id != dir_id);
        g.origin_changes.retain(|c| c.directory != dir_id);
        g.dir_errors.remove(dir_id);
        core.persist(&g);
    }
    log::info!("directory {dir_id}: đã xoá (xoá cả hub: {remove_hubs})");
    webviews::emit_state(app);
    Ok(())
}
