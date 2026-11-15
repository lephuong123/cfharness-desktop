# AGENTS.md — hướng dẫn cho agent code tiếp trên repo này

## Bối cảnh
- Sản phẩm: **Cfharness Desktop** — vỏ native (Tauri v2) hiển thị **chính giao diện web của từng hub cfharness** trong WebView.
  KHÔNG viết lại UI của hub, KHÔNG nói RPC trực tiếp với daemon. App chỉ thêm phần native (rail hub, đăng nhập tự động hub
  local, thông báo, badge, tải tệp, giọng nói, tự cập nhật).
- Đọc trước: `docs/PLAN.md` (đã duyệt; quyết định ở mục 10). Hợp đồng với hub (C1…C7) nằm trong `docs/PLAN.md` mục 6 và
  `agent-hub/docs/dev-notes/desktop-app.md`.
- Tên: productName "Cfharness Desktop", identifier `io.cfharness.desktop`, binary `cfharness-desktop`. macOS 14+, Windows 10 21H2+.
- Mốc hiện tại: P1 (vỏ + hub local). Tiếp: P2 hub cloud/import directory, P3 native (badge, thông báo, giọng nói), P4 đóng gói.
- Không sửa repo `agent-hub` từ đây (thay đổi phía hub = PR riêng bên đó).

## Cấu trúc
| Đường dẫn | Vai trò |
|---|---|
| `shell-ui/` | UI vỏ (Vite + TypeScript thuần): rail hub, menu "+", màn chưa cài cfharness, thanh tìm. Bundle cục bộ, là bề mặt DUY NHẤT có IPC |
| `src-tauri/src/hubs.rs` | Registry app `hubs.json` (PLAN §4): schema `version`, migrate v0, giữ trường lạ (`extra`), file mới hơn → chỉ đọc, ghi nguyên tử 0600, `last_url_of` |
| `src-tauri/src/local.rs` | Khám phá hub local (`~/.config/cfharness/hubs.json` + `~/.cfharness`, lọc thư mục tạm/thiếu `config.yaml`), tìm binary `cfharness`, chạy + parse C1 `cfharness web --json --ensure` |
| `src-tauri/src/nav.rs` | `Origin`, khoá điều hướng `decide()` (Allow/External/Block), `is_login_page` |
| `src-tauri/src/webviews.rs` | Một child WebView/hub cạnh rail: data store riêng, khoá điều hướng, chặn cửa sổ mới, tải tệp → hộp thoại Lưu, LRU, `last_url`, UA C7, đăng nhập lại |
| `src-tauri/src/core.rs` | Trạng thái dùng chung (`Core`), `ShellState` gửi cho shell |
| `src-tauri/src/commands.rs` | Lệnh IPC của shell (được khai báo trong `build.rs` → chỉ capability `shell` gọi được) |
| `src-tauri/src/menu.rs` | Menu: zoom, tìm, tải lại, in, Sửa (copy/paste cần trên macOS) |
| `src-tauri/capabilities/shell.json` | Capability DUY NHẤT, chỉ cho webview `shell` |
| `src-tauri/Info.plist` | Ngoại lệ ATS chỉ cho `localhost` + subdomain (hub local là http loopback) |
| `inject/` | Script tiêm vào WebView hub (P3) — xem `inject/README.md` |
| `scripts/dev-demo.sh` | Chạy app với hub demo của agent-hub (FakeRuntime), tách khỏi dữ liệu thật |

## Lệnh
```bash
npm install
npm run build                       # tsc --noEmit + vite build (shell-ui → dist/)
npm run tauri dev                   # chạy app
cd src-tauri && cargo test          # test Rust — KHÔNG gọi claude, KHÔNG mạng
cd src-tauri && cargo clippy --all-targets -- -D warnings
cd src-tauri && cargo check --target x86_64-pc-windows-msvc   # cần llvm-rc (giả được, xem README)
npm run tauri build -- --debug --bundles app                  # .app thử trên macOS
```
Thử với hub thật mà không đụng dữ liệu thật: `scripts/dev-demo.sh` (biến `CFHARNESS_DESKTOP_DATA_DIR|HOMES|CFHARNESS` chỉ cho dev).

## Bất biến bảo mật (PLAN §5) — không được phá
1. **Khoá điều hướng theo hub**: WebView của hub X chỉ tải origin X (+ `about:blank`, `blob:` của chính X). Origin khác (kể cả
   khác cổng/scheme) → huỷ + mở trình duyệt hệ thống; `javascript:`/`file:`/`data:`/userinfo → chặn. Cửa sổ mới luôn bị từ chối.
   Sửa `nav::decide` → thêm ca vào test của `nav.rs`.
2. **IPC tối thiểu cho trang hub**: trang hub là dữ liệu không tin cậy (có chữ agent). WebView hub KHÔNG có capability nào
   (đã kiểm thật: `get_state not allowed on … webview "hub-…"`). Lệnh app mới phải khai báo trong `build.rs` (`AppManifest`) và
   chỉ cấp cho `shell`. Lệnh IPC mới cho trang hub (P3: `badge_update`, speech, download, clipboard) = thay đổi bảo mật → capability
   giới hạn đúng origin đã đăng ký + kiểm kiểu payload phía Rust + test đỏ→xanh.
3. Không bao giờ tiêm token/credential vào trang hub; script khởi tạo không đọc DOM ngoài việc gọi `/app/badge`.
4. Thông báo OS chỉ chữ cố định + tên hub do người đặt trong app (không chữ agent).
5. Directory (P2): chỉ https, kiểm từng mục (origin `https://host[:port]`, không IP/path; id hex ≤16; tên một dòng ≤60/≤32;
   ≤200 hub), đổi origin của hub đã có → hỏi người. Token directory trong Keychain/Credential Manager, không ghi `hubs.json`.
6. App không giữ bí mật của hub: không lưu token người, khoá ký link, bí mật TOTP. Hub local đăng nhập bằng link một lần từ C1.
   `last_url` chỉ là path+query, KHÔNG BAO GIỜ fragment (`#k=`) hay `/login?t=` (`hubs::last_url_of`). Log URL qua
   `webviews::redact_url` (không query, không fragment).
7. Thư mục dữ liệu app (`io.cfharness.desktop`) được hub chặn với agent (C6/H5) — đừng đổi identifier.
8. Không `--dangerously-*`, không mở cổng mạng nào từ app (app chỉ là client). Không chạy gì bằng sudo/admin.
9. Ngoại lệ ATS chỉ cho `localhost` và subdomain; hub cloud bắt buộc https (kiểm ở `open_remote`).

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
