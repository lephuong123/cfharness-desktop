# AGENTS.md — hướng dẫn cho agent code tiếp trên repo này

## Bối cảnh
- Sản phẩm: **Cfharness Desktop** — vỏ native (Tauri v2) hiển thị **chính giao diện web của từng hub cfharness** trong WebView.
  KHÔNG viết lại UI của hub, KHÔNG nói RPC trực tiếp với daemon. App chỉ thêm phần native (rail hub, đăng nhập tự động hub
  local, thông báo, badge, tải tệp, giọng nói, tự cập nhật).
- Đọc trước: `docs/PLAN.md` (đã duyệt; quyết định ở mục 10). Hợp đồng với hub (C1…C7) nằm trong `docs/PLAN.md` mục 6 và
  `agent-hub/docs/dev-notes/desktop-app.md`.
- Tên: productName "Cfharness Desktop", identifier `io.cfharness.desktop`, binary `cfharness-desktop`. macOS 14+, Windows 10 21H2+.
- Mốc hiện tại: P1 (vỏ + hub local) + P2 (hub cloud, import directory, quản lý/chuyển hub) + P3 (cầu nối trang hub, badge,
  thông báo, tray, deep link, giọng nói, tự cài cfharness, cài đặt/tự khởi động) + P4 (đóng gói, updater ed25519, CI/phát hành)
  + P5 (parity route/ACTIONS + e2e) xong. Còn: chạy thật trên Windows, bản phát hành đầu tiên (cần remote + khoá updater).
- Không sửa repo `agent-hub` từ đây (thay đổi phía hub = PR riêng bên đó).

## Cấu trúc
| Đường dẫn | Vai trò |
|---|---|
| `shell-ui/` | UI vỏ (Vite + TypeScript thuần): rail hub, menu "+", màn chưa cài cfharness, thanh tìm. Bundle cục bộ, là bề mặt DUY NHẤT có IPC |
| `src-tauri/src/hubs.rs` | Registry app `hubs.json` (PLAN §4): schema `version`, migrate v0, giữ trường lạ (`extra`), file mới hơn → chỉ đọc, ghi nguyên tử 0600, `last_url_of` |
| `src-tauri/src/local.rs` | Khám phá hub local (`~/.config/cfharness/hubs.json` + `~/.cfharness`, lọc thư mục tạm/thiếu `config.yaml`), tìm binary `cfharness`, chạy + parse C1 `cfharness web --json --ensure` |
| `src-tauri/src/nav.rs` | `Origin`, khoá điều hướng `decide()` (Allow/External/Block), `is_login_page` |
| `src-tauri/src/webviews.rs` | Một child WebView/hub cạnh rail: data store riêng, khoá điều hướng, chặn cửa sổ mới, tải tệp → hộp thoại Lưu, LRU, `last_url`, UA C7, đăng nhập lại |
| `src-tauri/src/directory.rs` | THUẦN (không I/O): phân loại URL dán vào (`classify_input`, tách token `t=`), `validate_origin` (= `parse_public_origin` của hub), `clean_line` (bỏ điều khiển/bidi/vô hình), `parse_directory` (C5, schema 1, ≤200), `plan_sync`/`apply_sync` (thêm, gộp, stale, `origin_changes` — không bao giờ tự đổi origin), `resolve_origin_change`, giờ ISO |
| `src-tauri/src/fetch.rs` | `Fetcher` reqwest native-tls: https only, không redirect, ≤1 MB, ETag/304, phân loại lỗi (mạng/TLS), `probe` → `Reach`; `fixture` = server HTTP cục bộ cho test |
| `src-tauri/src/secrets.rs` | Trait `TokenStore` (`KeyringStore` thật, `MemoryStore` cho test/dev), `token_ref`/`key_of_ref` |
| `src-tauri/src/remote.rs` | Glue Tauri: `inspect` → `confirm` (token vào keychain TRƯỚC khi ghi `hubs.json`), `sync_directory`, vòng nền (6 h + thăm dò), `resolve_origin`, xoá hub/directory, `wipe_store` |
| `src-tauri/src/core.rs` | Trạng thái dùng chung (`Core`), `ShellState` gửi cho shell |
| `src-tauri/src/commands.rs` | Lệnh IPC của shell (được khai báo trong `build.rs` → chỉ capability `shell` gọi được) |
| `src-tauri/src/menu.rs` | Menu: zoom, tìm, tải lại, in, Sửa, Cài đặt; id `tray:*` chuyển cho `tray::dispatch` (Tauri gửi mọi sự kiện menu, kể cả tray, về handler chung) |
| `src-tauri/src/bridge.rs` | IPC cho TRANG HUB (P3): 6 lệnh `HUB_COMMANDS`, capability thêm lúc chạy cho từng WebView hub đúng origin (`grant`), kiểm payload thuần (`validate_*`), `check_caller` (label `hub-<id>` + URL đúng origin), `caller_active` (hub đang hiển thị + cửa sổ focus) cho speech/clipboard/download; `init_script` = `inject/hub.js` + origin + chu kỳ |
| `src-tauri/src/notify.rs` | Badge (`Inner.badges`) + thông báo OS chữ cố định khi số tăng và hub không ở tiền cảnh; bấm → `webviews::open_path` (`/approvals`, `/inbox`). macOS `mac-notification-sys` (luồng riêng, chờ click), Windows WinRT toast |
| `src-tauri/src/tray.rs` | Tray: danh sách hub (+ số chờ), hiện/ẩn, thoát; dựng lại menu chỉ khi chữ ký đổi (`refresh`, gọi từ `emit_state`) |
| `src-tauri/src/deeplink.rs` | `parse` THUẦN `cfharness://hub/<id>/<path>` (từ chối `//`, `\`, `..`, `#`, `/login`, userinfo, cổng), `resolve` (id registry hoặc `hub_id`) |
| `src-tauri/src/speech/` | `mod.rs` một phiên micro thuộc một hub, sự kiện → `__cfhNativeDispatch(<JSON>)`; dừng khi hub không còn tiền cảnh. `macos.rs` + `native/speech_mac.m` (ObjC, cc, SFSpeechRecognizer, kiểm khoá Info.plist trước khi xin quyền). `whisper.rs` sau feature `whisper` (cpal + whisper-rs, VAD năng lượng, mô hình pin SHA-256, tải sau khi người bấm "Tải") |
| `src-tauri/src/updater.rs` | Tự cập nhật (P4): `setup_from_config` (placeholder = TẮT, không liên lạc gì), endpoint CHỈ `https://github.com/<owner>/<repo>/releases/…` (`repo_of`), kênh beta suy ra từ cùng repo (`endpoint_for`), `valid_pubkey` (minisign), `clean_notes`; `check` không bao giờ tải, `install` chỉ khi shell gọi `update_install` đúng phiên bản vừa tìm thấy; kiểm tra nền 24 h |
| `src-tauri/src/installer.rs` | Tự cài: `plan` (tag mới nhất `HUB_REPO` = `updater.REPO` của hub, uv/git/claude, bước + lệnh hiển thị, nonce) → `run` (uv Astral nếu thiếu, `uv tool install --force cfharness@tag`, doctor, mở hub mặc định), log `shell://install`; không sudo, chặn root |
| `src-tauri/capabilities/shell.json` | Capability TĨNH duy nhất, chỉ cho webview `shell` (capability của trang hub thêm lúc chạy trong `bridge.rs`) |
| `src-tauri/native/speech_mac.m`, `Entitlements.plist` | Cầu ObjC giọng nói macOS; entitlement micro (cho hardened runtime) |
| `src-tauri/Info.plist` | Ngoại lệ ATS chỉ cho `localhost` + subdomain (hub local là http loopback) |
| `inject/hub.js` | Script khởi tạo của WebView hub: chỉ khung trên cùng + đúng origin (app nhúng), hỏi `/app/badge`, `window.cfhNative` (speech C4, clipboard, download) — xem `inject/README.md` |
| `scripts/dev-demo.sh` | Chạy app với hub demo của agent-hub (FakeRuntime), tách khỏi dữ liệu thật |
| `scripts/parity_routes.py` | P5: đọc TĨNH (`ast`) `agent-hub/src/cfharness/web/*.py` → route/`ACTIONS`/`OFFICE_ACTIONS`, so với `tests/e2e/parity.json` (thiếu/TODO/nhãn e2e không tồn tại → mã 1). Đường dẫn route không tĩnh → script báo lỗi (sửa script, đừng bỏ qua) |
| `scripts/release_tools.py` | Phát hành: `check-version`, `notes` (mục CHANGELOG), `config` (pubkey + endpoint repo + `createUpdaterArtifacts`), `latest-json` |
| `tests/e2e/` | Playwright WebKit trên `web.demo` (`global-setup.ts` khởi động hub, link đăng nhập qua C1); nhãn `[id]` trong tên ca = `cover: "e2e:<id>"`. `tauri-driver/smoke.mjs` = app thật, chỉ Windows CI |
| `.github/workflows/` | `ci.yml` (macOS arm64/x86_64 + Windows: test, clippy, tauri build không ký, tauri-driver; job parity-e2e khi có `AGENT_HUB_REPO`), `release.yml` (tag → dmg universal + NSIS, ký updater, Release = mục CHANGELOG, `latest.json` + release cuộn `beta`; ký Apple/Authenticode tắt bằng biến) |

## Lệnh
```bash
npm install
npm run build                       # tsc --noEmit + vite build (shell-ui → dist/)
npm run tauri dev                   # chạy app
cd src-tauri && cargo test          # test Rust — KHÔNG gọi claude, KHÔNG mạng
cd src-tauri && cargo clippy --all-targets -- -D warnings
cd src-tauri && cargo check --target x86_64-pc-windows-msvc   # cần llvm-rc (giả được, xem README)
cd src-tauri && cargo clippy --features whisper --all-targets  # giọng nói Windows; cần cmake (có thể dùng cmake từ pip trong venv tạm)
npm run tauri build -- --debug --bundles app                  # .app thử trên macOS
CI=true npm run tauri build -- --bundles dmg                  # .dmg (CI=true: bỏ AppleScript Finder, hỏng khi thiếu quyền Automation)
npm run parity                                                # route/ACTIONS mới của hub chưa có trong tests/e2e/parity.json → mã 1
npm run e2e                                                   # Playwright WebKit trên web.demo (cần `npx playwright install webkit`)
python3 -m unittest scripts/test_parity_routes.py scripts/test_release_tools.py
```
Hub thêm route/ACTION mới → chạy `python3 scripts/parity_routes.py --write`, điền `cover` (ưu tiên thêm ca e2e có nhãn `[id]`).
Thử với hub thật mà không đụng dữ liệu thật: `scripts/dev-demo.sh` (biến `CFHARNESS_DESKTOP_DATA_DIR|HOMES|CFHARNESS` chỉ cho dev).
Bản dev macOS (không bundle id) lưu cookie hub ở `~/Library/WebKit/cfharness-desktop/` + `~/Library/HTTPStorages/cfharness-desktop/`,
NGOÀI vùng H5 của hub → bản dev chỉ đăng nhập hub demo (README).
Kết quả spike P0 (cái gì đã kiểm chứng, cái gì còn là giả định — nhất là mọi thứ Windows): `docs/spikes/P0.md`.

## Bất biến bảo mật (PLAN §5) — không được phá
1. **Khoá điều hướng theo hub**: WebView của hub X chỉ tải origin X (+ `about:blank`, `blob:` của chính X). Origin khác (kể cả
   khác cổng/scheme) → huỷ + mở trình duyệt hệ thống; `javascript:`/`file:`/`data:`/userinfo → chặn. Cửa sổ mới luôn bị từ chối.
   Sửa `nav::decide` → thêm ca vào test của `nav.rs`.
2. **IPC tối thiểu cho trang hub**: trang hub là dữ liệu không tin cậy (có chữ agent). WebView hub CHỈ có 6 lệnh `bridge::HUB_COMMANDS`
   (`badge_update`, `speech_start|stop|available`, `clipboard_write` ≤ 1 MB, `download_save` qua hộp thoại Lưu) qua capability thêm
   lúc chạy, giới hạn đúng origin đã đăng ký (`bridge::grant`; đã kiểm thật với `web.demo`: badge chạy, lệnh shell vẫn bị từ chối).
   Mọi lệnh kiểm lại caller (`check_caller`) + payload (`validate_*`, `deny` trường lạ); micro/clipboard/lưu tệp chỉ cho hub đang hiển
   thị + cửa sổ focus; `clipboard_write` còn giới hạn tần suất theo hub (`ClipboardLimiter`, 1 lần / 2 s — cầu nối không thấy
   user activation). Quyền trình duyệt của trang hub (micro, camera, vị trí, thông báo, đọc clipboard…) luôn bị TỪ CHỐI
   (`webviews::permission_decision`; không có handler thì WKWebView cấp getUserMedia im lặng). Không dùng isolation pattern (CSP `'self'` của hub chặn iframe `isolation://`, và pattern áp cho cả app).
   Lệnh app mới khai báo trong `build.rs` và chỉ cấp cho `shell`; thêm lệnh cho trang hub = thay đổi bảo mật → `HUB_COMMANDS` +
   `build.rs` + kiểm payload + test đỏ→xanh (test `command_list_matches_build_rs` cấm cấp lệnh hub cho shell). Origin của hub local đổi
   (cổng mới) → TẠO LẠI WebView (script + capability gắn với origin cũ).
3. Không bao giờ tiêm token/credential vào trang hub; script khởi tạo không đọc DOM ngoài việc gọi `/app/badge`. Sự kiện giọng nói
   vào trang chỉ qua `speech::event_js` (JSON, bỏ ký tự điều khiển/bidi, escape U+2028/2029).
4. Thông báo OS chỉ chữ cố định + nhãn hub do app chọn (không chữ agent) — `notify::message` + `notify::notify_name`: tên người đặt,
   hoặc host của origin khi hub directory vẫn mang tên do directory gửi; tray cũng chỉ tên + số.
12. Deep link chỉ mở đường dẫn TƯƠNG ĐỐI trong hub đã có (`deeplink::parse`); không bao giờ `/login`, fragment, origin khác.
13. Tự cài: không chạy gì trước khi người bấm "Cài" (nonce của kế hoạch), không sudo/admin, chặn khi chạy bằng root; `HUB_REPO`
    phải bằng `updater.REPO` của agent-hub (test so khi repo nằm cạnh).
5. Directory: chỉ https, kiểm từng mục (origin `https://host[:port]`, không IP/path; id hex ≤16; tên một dòng ≤60/≤32;
   ≤200 hub), đổi origin của hub đã có → hỏi người. Token directory trong Keychain/Credential Manager, không ghi `hubs.json`.
   Sửa luật → thêm ca vào test của `directory.rs`. Không theo redirect; không log URL có token (dùng `directory::display_url`).
   `hubs.json` do người sửa được → origin/URL đọc từ đó luôn kiểm lại (`open_remote`, `sync_inner`).
10. Hộp thoại/menu của shell nằm DƯỚI WebView hub → mở thì gọi `set_overlay(true)` (ẩn hub), đóng thì `false`.
11. Phím tắt chuyển hub là accelerator của menu (Rust), không bắt trong trang hub (trang hub không có IPC).
6. App không giữ bí mật của hub: không lưu token người, khoá ký link, bí mật TOTP. Hub local đăng nhập bằng link một lần từ C1.
   `last_url` chỉ là path+query, KHÔNG BAO GIỜ fragment (`#k=`) hay `/login?t=` (`hubs::last_url_of`). Log URL qua
   `webviews::redact_url` (không query, không fragment).
7. Thư mục dữ liệu app (`io.cfharness.desktop`) được hub chặn với agent (C6/H5) — đừng đổi identifier.
8. Không `--dangerously-*`, không mở cổng mạng nào từ app (app chỉ là client). Không chạy gì bằng sudo/admin.
9. Ngoại lệ ATS chỉ cho `localhost` và subdomain; hub cloud bắt buộc https (kiểm ở `open_remote`).
14. Tự cập nhật: KHÔNG commit khoá riêng updater (chỉ secret CI `TAURI_SIGNING_PRIVATE_KEY`); `tauri.conf.json` giữ placeholder
    (test `repo_config_checked_against_placeholder`), `createUpdaterArtifacts` chỉ bật lúc phát hành qua `--config`. Endpoint chỉ là
    GitHub Releases của repo app (`updater::repo_of`). Không cài khi người chưa bấm; lệnh `update_*` chỉ cấp cho `shell`.

## Quy ước
- Chữ cho người dùng (UI, thông báo, lỗi, README, CHANGELOG, AGENTS): **tiếng Việt**. Định danh, comment code: tiếng Anh.
- Shell UI chỉ đặt chữ bằng `textContent` (tên hub đến từ đĩa/CLI — không bao giờ innerHTML). CSP của shell: `'self'`, không inline.
- Không gọi phương thức webview/window khi đang giữ `Core::lock()` (nhiều hàm vòng qua main thread). Trong callback chạy trên
  main thread (navigation/page load/new window/menu) → dời việc gọi webview sang `tauri::async_runtime::spawn` (`defer`).
- Phiên bản crate/npm ghim chính xác (`=x.y.z` cho tauri, số cụ thể trong `package.json`).
- Test không gọi `claude` thật, không mạng; hub để thử là `python -m cfharness.web.demo` của agent-hub.

## Phát hành — viết release note TRƯỚC
Như agent-hub: `CHANGELOG.md` (tiếng Việt, mục mới nhất trên cùng). Phát hành `vX.Y.Z`: (1) đổi "Chưa phát hành" thành
`## vX.Y.Z — <YYYY-MM-DD>`, đủ mọi thay đổi từ tag trước, có "Lưu ý khi cập nhật" nếu cần; (2) mới nâng version
(`package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`); (3) tag; (4) thân GitHub Release = chép nguyên mục CHANGELOG.
Trong lúc làm: thay đổi người dùng thấy được → thêm một dòng vào "Chưa phát hành".
