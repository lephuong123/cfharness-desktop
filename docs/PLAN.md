# Plan — Cfharness Desktop (macOS + Windows) + quản lý nhiều hub

Trạng thái: **ĐÃ DUYỆT 2026-11-15** (quyết định ở mục 10). Ngày: 2026-11-15. Nguồn: khảo sát `agent-hub` @ `0f25c35`
(`web/security.py`, `web/public.py`, `web/devices.py`, `web/app.py`, `transport.py`, `confine.py`, `admin_overview.py`,
`docs/dev-notes/web*.md`, `plan-multi-instance.md`, `voice.md`).

Repo: `~/Workspace/cfharness-desktop` (repo RIÊNG). Các thay đổi phía hub (mục 6) làm bằng PR trong `agent-hub`.

---

## 1. Mục tiêu

1. **App desktop macOS + Windows, chức năng giống 100% web** (mọi trang, mọi thao tác `ACTIONS`, SSE live, Điều phối,
   3D office, giọng nói, quản trị máy…).
2. **Quản lý nhiều hub**:
   - Mặc định có **hub local** (giống `cfharness web` local, tự đăng nhập).
   - **Import một URL** → lấy **toàn bộ hub cfharness trên cloud** (một máy cloud chạy nhiều hub của nhiều người /
     một người nhiều hub — `plan-multi-instance.md`).
   - **Chuyển qua lại** giữa các hub trong cùng một cửa sổ, mỗi hub giữ phiên/trạng thái riêng.

Không mục tiêu (v1): app di động; app nói chuyện RPC trực tiếp với daemon; viết lại UI bằng framework khác; hub
đa người thuê.

## 2. Quyết định cốt lõi

### Q1. App = vỏ native + chính giao diện web của từng hub (KHÔNG viết lại UI)
Web hiện là server-render (Starlette + Jinja2 + htmx + SSE, ~9.600 dòng Python + ~60 template). Viết lại UI native để
"giống 100%" nghĩa là nhân đôi toàn bộ và cần một JSON API mới cho mọi trang — không bao giờ theo kịp. Thay vào đó:

- Mỗi hub hiển thị trong một **WebView** trỏ tới origin web của chính hub đó (local `http://cfharness.localhost:<port>`,
  cloud `https://<public_origin>`).
- **Ngang chức năng do cấu trúc**: tính năng web mới xuất hiện trong app ngay khi hub cập nhật; giao diện luôn khớp
  phiên bản của hub đang xem (hub cũ/mới lẫn lộn vẫn đúng).
- Mọi bất biến bảo mật của web giữ nguyên (CSP, CSRF, khoá phiên `k`, Host, HUMAN_ONLY, audit kênh `web`).
- App chỉ thêm phần **native**: thanh chọn hub, đăng nhập tự động hub local, thông báo hệ điều hành, badge, tray,
  tải file, giọng nói (cầu nối), deep link, tự cập nhật.

### Q2. Khung: **Tauri v2** (Rust + WebView hệ thống)
| | Tauri v2 (đề xuất) | Electron |
|---|---|---|
| Cỡ cài đặt | ~10–15 MB | ~120 MB+ |
| Engine | WKWebView (mac) / WebView2 (Win, Chromium) | Chromium giống nhau 2 OS |
| Tách phiên theo hub | `data_store_identifier` (macOS 14+) / `data_directory` (WebView2) | `session.fromPartition` |
| IPC cho trang remote | allowlist theo domain + capability + isolation pattern | contextIsolation + preload |
| Bề mặt tấn công | không Node trong renderer | Node trong main, cần cứng hoá |
| Updater ký số | có (ed25519) | electron-updater |

Rủi ro của Tauri là khác biệt WKWebView ↔ WebView2 — xử lý bằng spike P0 (mục 8). Nếu spike chặn (vd tách data store
không chạy trên macOS 13), phương án dự phòng là Electron với cùng kiến trúc (chỉ thay lớp vỏ).

Yêu cầu tối thiểu đề xuất: **macOS 14+** (data store theo hub), **Windows 10 21H2+/11** (WebView2 Evergreen).

### Q3. Danh mục hub cloud = "directory" JSON, import KHÔNG cấp quyền
Import URL chỉ thêm **danh sách dấu trang** (origin + tên). Vào từng hub vẫn đăng nhập riêng bằng TOTP (+ thiết bị tin
cậy) như web public hiện nay. Không có token chung cho nhiều hub.

### Q4. App không giữ bí mật của hub
Không lưu token người (`<socket>.auth`), khoá ký link (`auth/web.json`), bí mật TOTP. Hub local: app gọi CLI
`cfharness` (cùng user OS) để xin link đăng nhập một lần — giống `cfharness web --open`. Hub cloud: cookie phiên +
cookie thiết bị tin cậy nằm trong data store riêng của hub đó. Token directory (nếu có) nằm trong Keychain / Windows
Credential Manager.

## 3. Kiến trúc

```
┌──────────────────────── cfharness Desktop (một cửa sổ) ─────────────────────────┐
│ Rail hub (UI vỏ, bundle cục bộ, IPC đầy đủ)  │  Vùng nội dung: WebView của hub đang chọn │
│  ● Local (~/.cfharness)        [3]           │  ┌─────────────────────────────────────┐  │
│  ● Local (~/hub-thu-2)                       │  │ http://cfharness.localhost:8711/…   │  │
│  ── Cloud "Mac mini công ty" ──              │  │  (web của hub, nguyên bản)          │  │
│  ● an.example.com              [1]           │  └─────────────────────────────────────┘  │
│  ● binh.example.com   (cần đăng nhập)        │  Mỗi hub = 1 WebView + 1 data store riêng │
│  + Thêm hub / Import URL                     │  (giữ sống tối đa N=5, LRU, còn lại ngủ)  │
└──────────────────────────────────────────────┴─────────────────────────────────────────────┘
        │ Rust core
        ├─ hubs.rs       registry app (hubs.json), đồng bộ directory, sức khoẻ
        ├─ local.rs      tìm `cfharness`, `cfharness web --json --ensure`, tự đăng nhập lại
        ├─ webviews.rs   tạo/khoá điều hướng/LRU, tải file, link ngoài → trình duyệt hệ thống
        ├─ bridge.rs     lệnh IPC tối thiểu cho trang hub: badge, notify, speech, download
        ├─ notify.rs     thông báo OS (chữ cố định, không chữ agent)
        ├─ speech/       mac: SFSpeechRecognizer · win: whisper.cpp (vi) — cầu nối cho voice.js
        └─ updater, tray, deep link `cfharness://`
```

### 3.1 Hub local
- **Khám phá**: `~/.config/cfharness/hubs.json` (registry hub do daemon ghi — `confine.register_home`; Windows:
  `%USERPROFILE%\.config\cfharness\hubs.json`) + mặc định `~/.cfharness`. Lọc: thư mục còn tồn tại, không nằm trong
  thư mục tạm (`/tmp`, `/private/var/folders`, `%TEMP%`), có `config.yaml`. Người dùng ẩn/hiện được; thêm tay theo
  đường dẫn.
- **Tìm binary `cfharness`**: PATH → `~/.local/bin` → thư mục `uv tool`; Windows thêm `.exe`. Không thấy → màn hướng dẫn
  cài **tự động** (chốt 2026-11-15): app hiện kế hoạch (lệnh sẽ chạy, thư mục cài, tag) → người bấm "Cài" → app chạy
  `uv tool install --force "cfharness @ git+https://github.com/<repo>@vX.Y.Z"` (tag mới nhất từ GitHub Releases, cùng
  nguồn với `updater.REPO`), hiện log trực tiếp, xong thì `cfharness doctor` + khởi động daemon. Máy chưa có `uv` → bước
  0 cài `uv` bằng bộ cài chính thức của Astral (mac: `curl -LsSf https://astral.sh/uv/install.sh | sh`, Win: PowerShell
  `irm https://astral.sh/uv/install.ps1 | iex`), cũng hiện trước + cần bấm xác nhận. Không bao giờ chạy với quyền
  admin/sudo; chỉ cài vào thư mục user. Chưa có `claude` CLI → chỉ hướng dẫn (đăng nhập Claude là việc của người).
  Cập nhật `cfharness` sau đó vẫn qua cơ chế sẵn có của hub (`cfharness update` / nút "Cập nhật hệ thống").
- **Đảm bảo web chạy + đăng nhập**: `cfharness web --json --ensure` (MỚI, H1) với `CFHARNESS_HOME=<home>` → khởi động
  web tách tiến trình nếu chưa chạy (cổng riêng theo hub, `web.port`), trả `{origin, login_url, hub_id, version,
  name}`. WebView mở `login_url` → cookie + `#k=` như trình duyệt.
- **Tự đăng nhập lại**: khi WebView bị chuyển về `/login` (phiên local hết hạn idle 1 h / tối đa 12 h), app tự xin link mới
  và đăng nhập lại — người dùng không thấy màn login với hub local.
- Daemon chưa chạy → CLI tự khởi động daemon (hành vi sẵn có).

### 3.2 Hub cloud + import URL
Hai loại URL người dùng có thể dán:

1. **Origin của một hub** `https://an.example.com` → app đọc `GET /.well-known/cfharness.json` (MỚI, H2, công khai,
   chỉ metadata tối thiểu) để xác nhận đó là hub cfharness → thêm 1 hub.
2. **URL directory** (vd `https://admin.example.com/.well-known/cfharness-directory?t=<token>` hoặc file JSON tĩnh bất kỳ
   qua https) → nhận danh sách hub → thêm tất cả, nhóm theo directory.

Định dạng directory (schema 1, danh sách trắng trường):
```json
{
  "kind": "cfharness.directory", "schema": 1,
  "name": "Mac mini công ty", "generated_at": "2026-11-15T10:00:00Z",
  "hubs": [
    {"id": "a1b2c3d4", "name": "An — dự án A", "user": "an", "origin": "https://an.example.com"},
    {"id": "e5f6a7b8", "name": "Bình — dự án B", "user": "binh", "origin": "https://binh.example.com"}
  ]
}
```
Kiểm hợp lệ phía app: `origin` chỉ `https://host[:port]`, không IP, không path (cùng luật `web/public.parse_public_origin`),
`id` hex ≤ 16, `name`/`user` một dòng ≤ 60 ký tự (bỏ ký tự điều khiển/bidi), ≤ 200 hub. Sai → bỏ mục đó + báo.
**Đồng bộ lại** (nút + tự động mỗi 6 h, ETag): hub mới thêm vào; hub biến mất → đánh dấu "không còn trong directory"
(không tự xoá data store); origin của một `id` đổi → hỏi người trước khi đổi (chống directory bị chiếm hướng sang
domain lạ).

Ai sinh directory: admin máy cloud — `cfharness admin directory export|serve|token` (MỚI, H4) dựng từ dữ liệu
`admin_overview` (đã có `user`, `hub_id`; bổ sung `public_origin` + tên hiển thị vào SPEC). Hai cách phát hành:
(a) web của hub admin phục vụ `/.well-known/cfharness-directory` có token (mặc định); (b) xuất file JSON tĩnh để admin
tự host.

### 3.3 Chuyển hub
- Rail bên trái kiểu Slack workspace: màu/chữ cái đầu, badge số việc cần người (duyệt + câu hỏi), chấm trạng thái
  (online / cần đăng nhập / mất kết nối / ESTOP).
- Phím: `Cmd/Ctrl+1…9`, `Cmd/Ctrl+K` bảng tìm hub, `Cmd/Ctrl+Shift+[ ]` hub trước/sau. Menu tray liệt kê hub.
- Mỗi hub giữ WebView sống (giữ trang đang xem, chat Điều phối, SSE) tối đa **N = 5** (setting); quá N → hub lâu không
  dùng nhất bị "ngủ" (huỷ WebView, giữ data store → mở lại vẫn còn phiên).
- Mở lại app → về hub + URL cuối cùng của từng hub.

### 3.4 Badge + thông báo
- Endpoint MỚI `GET /app/badge` (H2): JSON chỉ số nguyên/từ khoá `{approvals, questions, running, estop, version}`,
  cần phiên (401 nếu chưa đăng nhập), không gia hạn phiên — cùng quy tắc `/office/state`.
- Script khởi tạo do app tiêm vào WebView của hub gọi `/app/badge` cùng origin mỗi 10 s (dùng cookie của chính hub),
  gửi số qua IPC `badge_update`. App không cần credential riêng.
- Số duyệt/câu hỏi tăng khi hub không ở tiền cảnh → thông báo OS **chữ cố định**: "Hub *<tên trong registry của app>*:
  2 yêu cầu duyệt mới". Không bao giờ đưa chữ agent vào thông báo. Bấm → chuyển hub + mở `/approvals`.

### 3.5 Khoảng trống "web chạy trong trình duyệt" ↔ "web chạy trong WebView" (phải xử lý để đạt 100%)
| # | Chức năng web | Vấn đề trong WebView | Xử lý |
|---|---|---|---|
| G1 | Giọng nói (`voice.js`, Web Speech API, `vi-VN`) | WKWebView/WebView2 **không có** SpeechRecognition | Cầu nối `window.cfhNative.speech` cùng giao diện; mac: `SFSpeechRecognizer` (vi-VN), Win: whisper.cpp model nhỏ đa ngữ tải lần đầu. `voice.js` dò cầu nối trước (H3). TTS: `speechSynthesis` có sẵn, kiểm giọng vi-VN; thiếu → TTS native |
| G2 | Micro | Cần quyền OS | mac `NSMicrophoneUsageDescription` + `NSSpeechRecognitionUsageDescription`; WebView2 permission handler chỉ cấp cho origin hub đang chọn |
| G3 | Tải file (file pages, `[[file:]]`, export) | WebView không tự lưu | Download handler → hộp thoại Lưu của OS |
| G4 | Link ngoài (`rel=noopener`, `target=_blank`) | Có thể mở trong WebView | Khoá điều hướng: chỉ trong origin hub; khác → trình duyệt hệ thống; chặn cửa sổ mới, `javascript:`, `file:` |
| G5 | Nút Sao chép (clipboard) | Cần secure context | Spike `cfharness.localhost` có được coi là secure context; không → cầu nối `clipboard_write` |
| G6 | Phân giải `*.localhost` | WKWebView dùng resolver hệ thống | Spike; dự phòng `--host-name 127.0.0.1` KHÔNG dùng (mất WT-1) → nếu không phân giải được thì app thêm custom scheme proxy (phương án B trong spike) |
| G7 | Cookie `SameSite=Strict`, `__Host-…_dev`, `localStorage k` | Theo data store | Kiểm trong spike; data store riêng theo hub nên không đá phiên nhau (C2) |
| G8 | SSE, htmx polling khi WebView ẩn | Hệ điều hành bóp timer nền | Chấp nhận (khi mở lại, trang tự nối lại `since=`); badge chạy ở script riêng có tần suất thấp |
| G9 | 3D office (Three.js, WebGL) | Hiệu năng WKWebView | Đo `#perf3d` trong spike, mục tiêu như `office-3d-perf.md` |
| G10 | "Mở file trên máy" (`fileview.open_command`) | Chỉ hợp lệ khi local | Web đã chỉ cho phép khi kết nối cục bộ → giữ nguyên, app không thêm gì |
| G11 | `<dialog>`, form POST không JS, `confirm` 2 bước | Không vấn đề | Test hồi quy |
| G12 | In / phóng to / tìm trong trang | Trình duyệt có sẵn | Menu app: Cmd/Ctrl +/−/0, Cmd/Ctrl+F (find-in-page), In |

## 4. Mô hình dữ liệu của app

`<app data>/hubs.json` (mac `~/Library/Application Support/io.cfharness.desktop/`, Win `%APPDATA%\cfharness-desktop\`),
quyền 0600:
```json
{
  "version": 1,
  "active": "loc-3f2a",
  "hubs": [
    {"id": "loc-3f2a", "kind": "local", "name": "Local", "home": "/Users/x/.cfharness", "color": "#d97757",
     "last_url": "/manager", "hidden": false},
    {"id": "rem-a1b2c3d4", "kind": "remote", "name": "An — dự án A", "origin": "https://an.example.com",
     "directory": "dir-01", "hub_id": "a1b2c3d4", "last_url": "/", "stale": false}
  ],
  "directories": [
    {"id": "dir-01", "name": "Mac mini công ty", "url": "https://admin.example.com/.well-known/cfharness-directory",
     "token_ref": "keychain:cfharness-desktop/dir-01", "etag": "…", "synced_at": "…"}
  ],
  "settings": {"live_webviews": 5, "notify": true, "autostart": false, "badge_poll_s": 10}
}
```
Data store WebView: một thư mục/identifier cho mỗi `hubs[].id` (xoá hub = hỏi có xoá luôn phiên/cookie không).

## 5. Bảo mật (bất biến của app)

1. **Khoá điều hướng theo hub**: WebView của hub X chỉ tải origin X (+ `about:blank`). Redirect sang origin khác → huỷ +
   mở trình duyệt hệ thống.
2. **IPC tối thiểu cho trang hub** (trang hub chứa dữ liệu agent đã qua `md()`/autoescape nhưng vẫn coi là không tin
   cậy): chỉ lệnh `badge_update{ints}`, `speech_start|stop`, `download_save`, `clipboard_write{text ≤ 1 MB}`; capability
   Tauri giới hạn theo đúng origin đã đăng ký; **isolation pattern** + kiểm kiểu payload phía Rust. UI vỏ (bundle cục bộ)
   là nơi duy nhất có lệnh quản lý hub. Thêm lệnh IPC mới cho trang hub = thay đổi bảo mật, cần test đỏ→xanh.
3. Không bao giờ tiêm token/credential vào trang hub; script khởi tạo không đọc DOM ngoài việc gọi `/app/badge`.
4. Thông báo OS chỉ chữ cố định + tên hub do người đặt trong app.
5. Directory: chỉ https, kiểm từng mục, đổi origin của hub đã có → hỏi người. Token directory trong Keychain/Credential
   Manager, không ghi `hubs.json`.
6. Agent của hub local chạy cùng user OS: thêm thư mục data của app vào `denyRead`/`denyWrite` sandbox + `permissions.deny`
   + classifier (H5) — agent không đọc được cookie phiên WebView hay token directory.
7. Ký số: **chưa có chứng chỉ** (chốt 2026-11-15) → bản phát hành nội bộ KHÔNG ký OS: macOS ký ad-hoc (bắt buộc trên
   Apple Silicon, Tauri tự làm), người cài phải mở bằng chuột phải → Mở lần đầu (hoặc `xattr -dr com.apple.quarantine`);
   Windows hiện cảnh báo SmartScreen. Hệ quả cần ghi trong README: chữ ký ad-hoc đổi mỗi bản → macOS có thể hỏi lại quyền
   micro/nhận dạng giọng nói sau mỗi lần cập nhật. **Updater Tauri vẫn ký ed25519** (độc lập với chứng chỉ OS) — khoá riêng
   chỉ ở CI secret, app từ chối gói cập nhật sai chữ ký. Khi có Developer ID/Authenticode → bật ký + notarize trong
   `release.yml` (đã chừa sẵn bước, tắt bằng biến).
8. Không có `--dangerously-*`, không mở cổng mạng nào từ app (app chỉ là client).

## 6. Thay đổi phía hub (PR trong `agent-hub`)

| PR | Nội dung | Ghi chú bất biến |
|---|---|---|
| **H1** | `cfharness web --json [--ensure]`: in JSON `{origin, login_url, hub_id, version, name}`; `--ensure` khởi động web tách tiến trình nếu chưa chạy, cổng từ `web.port` (mặc định cấp cổng riêng theo hub để tránh C1); chạy được trên Windows | Không in token người; `login_url` một lần, TTL 300 s như cũ; ∈ `HUB_HUMAN_ONLY_CLI` (lệnh `web` đã forbidden với agent) |
| **H2** | `GET /.well-known/cfharness.json` (công khai: `kind`, `schema`, `app_api`, không version chi tiết/tên người) + `GET /app/badge` (cần phiên, chỉ số nguyên, không gia hạn phiên, nằm trong `GuardMiddleware.API`) | Như `/office/state`: không chữ tự do; test 401 + danh sách trắng trường |
| **H3** | `voice.js`: dùng `window.cfhNative.speech` nếu có, cùng sự kiện `mgr:*`; giữ bất biến `voice=dictation|conversation` (giọng nói không duyệt/chốt) | Không nới CSP/Permissions-Policy |
| **H4** | Directory: thêm `public_origin` + `display_name` vào SPEC `admin_overview` (schema 3, kiểm bằng `parse_public_origin`); `cfharness admin directory export|token|serve`; route `/.well-known/cfharness-directory?t=` trên hub admin | `admin.*` ∈ HUMAN_ONLY; token directory chỉ cấp quyền ĐỌC danh sách; không chữ tự do ngoài trường đã kiểm |
| **H5** | Sandbox `denyRead/denyWrite` + `permissions.deny` + classifier cho thư mục data của app (mac/Win) | Ca trong `tests/test_guard_classifier.py` + `tests/test_sec_*` |
| **H6** | Header nhận diện `X-Cfharness-App: <ver>` (tuỳ chọn) chỉ để log/thống kê; KHÔNG đổi giao diện theo header (giữ "giống 100%") | — |

Hub chưa có H2/H3 vẫn dùng được trong app (không badge, voice báo "cần cập nhật hub").

## 7. Cấu trúc repo `cfharness-desktop`

```
cfharness-desktop/
├── AGENTS.md              quy ước (tiếng Việt cho người dùng, định danh tiếng Anh), bất biến mục 5
├── CHANGELOG.md           release note trước khi tag (giống quy trình agent-hub)
├── docs/PLAN.md           tài liệu này
├── shell-ui/              UI vỏ: rail hub, hộp Thêm hub/Import, Cài đặt app, màn chưa cài cfharness (TS + Svelte/vanilla, bundle cục bộ)
├── src-tauri/
│   ├── tauri.conf.json, capabilities/{shell.json, hub-remote.json}
│   └── src/{main,hubs,directory,local,webviews,bridge,notify,tray,deeplink}.rs, speech/{mod,macos,windows}.rs
├── inject/badge.js        script khởi tạo cho WebView hub (nhỏ, chỉ fetch /app/badge)
├── tests/
│   ├── rust (cargo test): parse/kiểm directory, registry, khoá điều hướng, payload IPC
│   ├── e2e/ (WebDriver tauri-driver trên Windows; macOS: Playwright WebKit + checklist tay)
│   └── fixtures/ hub giả = `python -m cfharness.web.demo` (FakeRuntime, không gọi claude)
└── .github/workflows/{ci.yml, release.yml}   ma trận macos-14 (arm64 + x64) + windows-latest
```

## 8. Lộ trình

### P0 — Spike (≈ 1 tuần) — quyết định Tauri hay Electron
Trên macOS 14/15 và Windows 11, với `web.demo` của agent-hub:
- [ ] 2 WebView, 2 data store riêng → đăng nhập 2 hub local cùng lúc không đá nhau (G7).
- [ ] `cfharness.localhost` phân giải + được coi là secure context (G5, G6).
- [ ] Đăng nhập link một lần + `#k=` + POST có CSRF + SSE `/sse/run`, `/sse/manager` chạy.
- [ ] Hub cloud qua tunnel TLS: TOTP + cookie `__Host-…_dev` thiết bị tin cậy sống qua khởi động lại app.
- [ ] Micro trong WebView; SFSpeechRecognizer vi-VN; whisper.cpp vi trên Windows (độ trễ, cỡ model).
- [ ] 3D office `#perf3d` trong WKWebView/WebView2; tải file; khoá điều hướng; IPC chỉ cho origin đăng ký.
Đầu ra: `docs/spikes/P0.md` + quyết định khung.

### P1 — Vỏ + hub local (MVP, ≈ 2 tuần)
- Cửa sổ, rail hub, khám phá hub local, tìm `cfharness`, H1 (agent-hub), tự đăng nhập/đăng nhập lại.
- Khoá điều hướng, link ngoài, tải file, menu (zoom, tìm, in, tải lại), nhớ hub/URL cuối.
- Nghiệm thu: mở app trên máy có `~/.cfharness` → vào thẳng web local đã đăng nhập; mọi trang trong checklist ngang
  chức năng (mục 9) chạy; Windows 11 chạy hub local thật.

### P2 — Hub cloud, import URL, chuyển hub (≈ 1,5 tuần)
- H2 + H4 (agent-hub). Thêm hub theo origin, import directory, đồng bộ lại, nhóm theo directory, đổi tên/màu/ẩn/xoá.
- LRU WebView, phím tắt, `Cmd/Ctrl+K`, trạng thái hub (online / cần đăng nhập / lỗi TLS / mất mạng).
- Nghiệm thu: import directory 3 hub → đăng nhập TOTP từng hub → chuyển qua lại giữ nguyên trang và phiên; directory bị
  sửa origin → app hỏi trước khi đổi.

### P3 — Native (≈ 2 tuần)
- Badge + thông báo OS (H2), tray (danh sách hub, ESTOP chỉ là link mở trang — không bấm thẳng từ tray), deep link
  `cfharness://hub/<id>/approvals`, tự khởi động cùng OS (tuỳ chọn).
- Giọng nói: H3 + cầu nối speech mac/Win (G1, G2).
- H5 (sandbox chặn thư mục app).
- Nghiệm thu: hub ẩn có approval mới → thông báo, bấm vào mở đúng trang; hội thoại giọng nói với điều phối viên chạy
  trên cả 2 OS.

### P4 — Đóng gói, ký số, tự cập nhật (≈ 1 tuần)
- `.dmg` universal (arm64 + x64, ký ad-hoc); NSIS `.exe` cho Windows (không ký); WebView2 bootstrapper. Bước notarize/
  Authenticode có sẵn trong workflow, tắt tới khi có chứng chỉ.
- Updater Tauri (GitHub Releases của repo app), kênh `stable`/`beta`.
- Quy trình phát hành: CHANGELOG trước → nâng version → tag → Release body = mục CHANGELOG.

### P5 — Kiểm thử ngang chức năng liên tục (≈ 1 tuần, rồi duy trì)
- Script sinh checklist từ `agent-hub`: liệt kê mọi `Route` trong `web/app.py` + mọi khoá `ACTIONS` + `OFFICE_ACTIONS`
  → so với bộ e2e của app. Route/ACTION mới chưa có ca → CI đỏ.
- E2E chạy trên `web.demo` (FakeRuntime, không gọi `claude`, không tốn tiền).

Tổng: **≈ 8–9 tuần** cho một người, P0 có thể rút nếu chọn ngay Electron.

## 9. Checklist ngang chức năng (rút gọn — bản đầy đủ sinh tự động ở P5)
Tổng quan · Điều phối (chat, SSE, Dừng, luồng việc, giọng nói) · Agents (tạo/sửa/template/xoá mềm/revert) · Runs (live,
steer, continue, diff, revert) · Tasks (bảng realtime, comment) · Duyệt + câu hỏi (2 bước, luôn cho phép, grant luật) ·
Inbox · Lịch · Learnings/ledger/golden · Memory + graph · Skills · MCP (browse/install/OAuth — callback OAuth phải quay
về đúng WebView) · Quyền đã cấp · Chi phí · Audit · Cài đặt (mọi nhóm, Slack wizard, Telegram, backup, cập nhật hệ thống)
· Tài khoản Claude · File (xem/tải) · Tìm kiếm · 3D office (duyệt/câu hỏi/chat trong cảnh) · Quản trị máy/admin ·
Đăng nhập/TOTP enroll/thiết bị tin cậy/đăng xuất.

Điểm cần chú ý riêng: **MCP OAuth** mở trang nhà cung cấp — phải mở trong trình duyệt hệ thống rồi callback về hub (đã
là callback của web) → kiểm luồng khi người dùng hoàn tất ngoài app.

## 10. Quyết định của chủ repo (2026-11-15)
1. Khung: **Tauri v2** (Electron chỉ là dự phòng nếu spike P0 chặn).
2. Directory: **hub admin phục vụ có token** (`/.well-known/cfharness-directory?t=`), xuất file tĩnh là phụ.
3. Ký số: **chưa có** Apple Developer ID / chứng chỉ Windows → bản không ký OS (mục 5.7), updater vẫn ký ed25519.
4. Tên app: **Cfharness Desktop**; bundle id `io.cfharness.desktop`; tên binary `cfharness-desktop`.
5. **App tự cài `cfharness`** (và `uv` nếu thiếu) sau khi người bấm xác nhận (mục 3.1).
6. OS tối thiểu: **macOS 14+**, **Windows 10 21H2+ / 11**.
