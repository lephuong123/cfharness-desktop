//! HTTP for cloud hubs: C2 `/.well-known/cfharness.json`, C5 directory documents and reachability probes.
//!
//! Strict client: https only (outside tests), no redirects (a redirect is an error, never followed — a directory or
//! a hub origin must answer at the URL the user gave), small bodies (≤ 1 MB), short timeouts, system TLS trust
//! (same store as the webview), no cookies. The directory token is part of the request URL only — it is never logged
//! (`log` lines use `directory::display_url`).

use crate::directory::{self, HubMeta};
use crate::nav::Origin;
use std::time::Duration;
use url::Url;

pub const MAX_BODY: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    /// DNS / connection refused / network down.
    Offline,
    /// Certificate or TLS handshake failure.
    Tls,
    Timeout,
    /// 3xx — never followed.
    Redirect,
    Status(u16),
    TooLarge,
    Other(String),
}

impl FetchError {
    pub fn message(&self) -> String {
        match self {
            FetchError::Offline => "Không kết nối được (mất mạng, sai tên miền hoặc máy chủ tắt)".into(),
            FetchError::Tls => "Lỗi TLS: chứng chỉ của máy chủ không hợp lệ hoặc không được tin cậy".into(),
            FetchError::Timeout => "Máy chủ không trả lời kịp".into(),
            FetchError::Redirect => "Máy chủ chuyển hướng sang địa chỉ khác — app không đi theo chuyển hướng".into(),
            FetchError::Status(404) => "Không tìm thấy (404) — URL hoặc token sai, hoặc hub chưa hỗ trợ app".into(),
            FetchError::Status(429) => "Thử quá nhiều lần (429) — đợi một lúc rồi thử lại".into(),
            FetchError::Status(c) => format!("Máy chủ trả lỗi HTTP {c}"),
            FetchError::TooLarge => "Phản hồi quá lớn (> 1 MB)".into(),
            FetchError::Other(_) => "Lỗi mạng không xác định".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fetched {
    NotModified,
    Body { text: String, etag: Option<String> },
}

/// Reachability of a remote hub (rail status when its webview is not open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    Online,
    Offline,
    Tls,
    /// Answers, but not as a cfharness hub (or an HTTP error).
    Bad,
}

pub struct Fetcher {
    client: reqwest::Client,
}

fn classify(e: &reqwest::Error) -> FetchError {
    if e.is_timeout() {
        return FetchError::Timeout;
    }
    if e.is_redirect() {
        return FetchError::Redirect;
    }
    let mut chain = String::new();
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        chain.push_str(&err.to_string().to_ascii_lowercase());
        chain.push('|');
        cur = err.source();
    }
    const TLS_HINTS: [&str; 8] =
        ["certificate", "tls", "ssl", "handshake", "x509", "principal name", "not trusted", "trust"];
    if TLS_HINTS.iter().any(|h| chain.contains(h)) {
        return FetchError::Tls;
    }
    if e.is_connect() || e.is_request() {
        return FetchError::Offline;
    }
    FetchError::Other(chain)
}

impl Fetcher {
    fn build(https_only: bool) -> Fetcher {
        let client = reqwest::Client::builder()
            .user_agent(format!("CfharnessDesktop/{}", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(https_only)
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("reqwest client");
        Fetcher { client }
    }

    /// The production client: https only.
    pub fn new() -> Fetcher {
        Fetcher::build(true)
    }

    /// Plain-http allowed: ONLY for tests against a local fixture server.
    #[cfg(test)]
    pub fn for_tests() -> Fetcher {
        Fetcher::build(false)
    }

    /// GET a small JSON document. `etag` → `If-None-Match` (304 → `NotModified`).
    pub async fn get(&self, url: &Url, etag: Option<&str>) -> Result<Fetched, FetchError> {
        let mut req = self.client.get(url.clone()).header("Accept", "application/json");
        if let Some(t) = etag {
            req = req.header("If-None-Match", t);
        }
        let mut resp = req.send().await.map_err(|e| classify(&e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        if status.is_redirection() {
            return Err(FetchError::Redirect);
        }
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }
        if resp.content_length().is_some_and(|n| n as usize > MAX_BODY) {
            return Err(FetchError::TooLarge);
        }
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() <= 256 && v.bytes().all(|b| b.is_ascii_graphic() || b == b' '))
            .map(str::to_string);
        let mut body: Vec<u8> = vec![];
        while let Some(chunk) = resp.chunk().await.map_err(|e| classify(&e))? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(body).map_err(|_| FetchError::Other("not utf-8".into()))?;
        Ok(Fetched::Body { text, etag })
    }

    /// C2: is `origin` a cfharness hub? `Ok(None)` = it answered, but not as a hub.
    pub async fn hub_meta(&self, origin: &Origin) -> Result<Option<HubMeta>, FetchError> {
        let url = origin.join("/.well-known/cfharness.json").ok_or(FetchError::Other("bad origin".into()))?;
        match self.get(&url, None).await? {
            Fetched::Body { text, .. } => Ok(directory::parse_hub_meta(&text)),
            Fetched::NotModified => Ok(None),
        }
    }

    pub async fn probe(&self, origin: &Origin) -> Reach {
        match self.hub_meta(origin).await {
            Ok(Some(_)) => Reach::Online,
            Ok(None) => Reach::Bad,
            Err(FetchError::Tls) => Reach::Tls,
            Err(FetchError::Offline | FetchError::Timeout) => Reach::Offline,
            Err(_) => Reach::Bad,
        }
    }
}

impl Default for Fetcher {
    fn default() -> Self {
        Fetcher::new()
    }
}

#[cfg(test)]
pub mod fixture {
    //! Tiny local HTTP/1.1 server for fetch tests (plain http on 127.0.0.1, one thread, canned routes).
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub struct Response {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    pub type Handler = dyn Fn(&str, &[(String, String)]) -> Response + Send + Sync;

    pub struct Server {
        pub base: String,
        pub requests: Arc<Mutex<Vec<String>>>,
    }

    pub fn json(status: u16, body: &str) -> Response {
        Response {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn serve(handler: Box<Handler>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let requests = Arc::new(Mutex::new(vec![]));
        let log = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut headers = vec![];
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.trim_end().split_once(':') {
                        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                    }
                }
                log.lock().unwrap().push(target.clone());
                let r = handler(&target, &headers);
                let mut out =
                    format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n", r.status, r.body.len());
                for (k, v) in &r.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let _ = stream.write_all(out.as_bytes());
                let _ = stream.write_all(&r.body);
                let _ = stream.flush();
            }
        });
        Server { base, requests }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{json, serve, Response};
    use super::*;

    fn url(base: &str, p: &str) -> Url {
        Url::parse(&format!("{base}{p}")).unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn etag_round_trip() {
        let srv = serve(Box::new(|_t, h| {
            if h.iter().any(|(k, v)| k == "if-none-match" && v == "\"v1\"") {
                Response { status: 304, headers: vec![], body: vec![] }
            } else {
                let mut r = json(200, r#"{"kind":"cfharness.directory","schema":1,"hubs":[]}"#);
                r.headers.push(("ETag".into(), "\"v1\"".into()));
                r
            }
        }));
        let f = Fetcher::for_tests();
        let u = url(&srv.base, "/d?t=secret");
        match f.get(&u, None).await.unwrap() {
            Fetched::Body { text, etag } => {
                assert!(text.contains("cfharness.directory"));
                assert_eq!(etag.as_deref(), Some("\"v1\""));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(f.get(&u, Some("\"v1\"")).await.unwrap(), Fetched::NotModified);
        assert_eq!(srv.requests.lock().unwrap()[0], "/d?t=secret");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn redirects_are_refused_errors_classified() {
        let srv = serve(Box::new(|t, _| match t {
            "/redirect" => Response {
                status: 302,
                headers: vec![("Location".into(), "https://evil.example.net/".into())],
                body: vec![],
            },
            "/big" => json(200, &"x".repeat(MAX_BODY + 1)),
            "/.well-known/cfharness.json" => json(200, r#"{"kind":"cfharness.hub","schema":1,"app_api":1}"#),
            _ => json(404, r#"{"error":"not_found"}"#),
        }));
        let f = Fetcher::for_tests();
        assert_eq!(f.get(&url(&srv.base, "/redirect"), None).await, Err(FetchError::Redirect));
        assert_eq!(f.get(&url(&srv.base, "/big"), None).await, Err(FetchError::TooLarge));
        assert_eq!(f.get(&url(&srv.base, "/nope"), None).await, Err(FetchError::Status(404)));
        let origin = Origin::parse(&srv.base).unwrap();
        assert_eq!(f.hub_meta(&origin).await.unwrap(), Some(HubMeta { schema: 1, app_api: 1 }));
        assert_eq!(f.probe(&origin).await, Reach::Online);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn not_a_hub_and_offline() {
        let srv = serve(Box::new(|_t, _| json(200, "<html>hello</html>")));
        let f = Fetcher::for_tests();
        let origin = Origin::parse(&srv.base).unwrap();
        assert_eq!(f.hub_meta(&origin).await.unwrap(), None);
        assert_eq!(f.probe(&origin).await, Reach::Bad);
        // nothing listens on this port (bound then dropped)
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let dead = Origin::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        assert_eq!(f.probe(&dead).await, Reach::Offline);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn production_client_refuses_http() {
        let srv = serve(Box::new(|_t, _| json(200, "{}")));
        let f = Fetcher::new();
        assert!(f.get(&url(&srv.base, "/"), None).await.is_err());
        assert!(srv.requests.lock().unwrap().is_empty());
    }
}
