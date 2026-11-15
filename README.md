# Cfharness Desktop

App desktop (macOS + Windows) cho [cfharness](https://github.com/lephuong123/agent-hub): một cửa sổ, thanh hub bên trái, bên phải
là **chính giao diện web của từng hub** (không viết lại UI) — nên mọi trang, mọi thao tác của web đều có trong app ngay khi hub
cập nhật. Kế hoạch đầy đủ và các quyết định đã chốt: [`docs/PLAN.md`](docs/PLAN.md).

Trạng thái: **P1 (vỏ + hub local)** đang làm — xem [`CHANGELOG.md`](CHANGELOG.md).

## Yêu cầu

- macOS 14 trở lên (cần cho kho dữ liệu WebView riêng theo hub), hoặc Windows 10 21H2+ / Windows 11 (WebView2 Evergreen — bộ cài
  tự tải nếu máy chưa có).
- `cfharness` đã cài cho user hiện tại (`uv tool install …`). Chưa có thì app hiện màn hướng dẫn; bản sau app sẽ tự cài sau khi
  bạn xác nhận.

## Cài bản phát hành (chưa ký số)

Hiện chưa có chứng chỉ Apple Developer ID / Authenticode, nên bản phát hành **không ký bởi hệ điều hành** (macOS chỉ ký ad-hoc).

**macOS** (`.dmg`):
1. Mở `.dmg`, kéo **Cfharness Desktop** vào **Applications**.
2. Lần đầu: trong Finder, **chuột phải (hoặc Control-click) vào app → Mở → Mở**. macOS chỉ hỏi một lần.
   Nếu macOS báo "đã hỏng" hoặc không cho mở, chạy trong Terminal:
   ```bash
   xattr -dr com.apple.quarantine "/Applications/Cfharness Desktop.app"
   ```
3. Lưu ý: chữ ký ad-hoc đổi sau mỗi bản cập nhật, nên macOS có thể hỏi lại quyền (micro, nhận dạng giọng nói…) sau khi cập nhật.

**Windows** (`…-setup.exe`, NSIS, cài cho user hiện tại — không cần quyền quản trị):
1. Chạy file cài. Windows SmartScreen có thể hiện "Windows đã bảo vệ máy tính của bạn" → bấm **Thông tin thêm → Vẫn chạy**.
2. Máy chưa có WebView2 thì bộ cài tự tải về.

Gói tự cập nhật của app (bản sau) vẫn được ký ed25519 — app từ chối gói sai chữ ký, độc lập với việc ký của hệ điều hành.

## Dùng

- Mở app trên máy đã có hub (`~/.cfharness` hoặc hub đã đăng ký trong `~/.config/cfharness/hubs.json`) → app tự chạy
  `cfharness web --json --ensure` cho hub đó, đăng nhập bằng link một lần và mở thẳng trang bạn xem lần trước.
- Phiên hết hạn (về trang `/login`) → app tự xin link mới và đăng nhập lại, bạn không phải gõ gì.
- Hub trong thư mục tạm (`/tmp`, `/var/folders`, `%TEMP%`) hoặc không có `config.yaml` bị bỏ qua.
- Mỗi hub có kho cookie/phiên riêng; tối đa 5 hub giữ WebView sống cùng lúc (hub lâu không dùng "ngủ", mở lại vẫn còn phiên).
- Link ra ngoài hub mở bằng trình duyệt hệ thống; tải tệp hiện hộp thoại Lưu.
- Menu: **Xem** → Tải lại (⌘R), Phóng to/Thu nhỏ/Cỡ thật (⌘= / ⌘- / ⌘0); **Sửa** → Tìm trong trang (⌘F, ⌘G tìm tiếp);
  **Tệp** → In (⌘P). Trên Windows dùng Ctrl thay ⌘.

Dữ liệu của app (danh sách hub `hubs.json` quyền 0600, kho WebView): macOS
`~/Library/Application Support/io.cfharness.desktop/`, Windows `%APPDATA%\io.cfharness.desktop\`. App **không** lưu token
người, khoá ký link hay bí mật TOTP của hub.

## Phát triển

```bash
npm install
npm run tauri dev              # chạy app (Vite + cargo)
npm run build                  # kiểm kiểu + build shell-ui
cd src-tauri && cargo test     # test Rust (không gọi claude, không mạng)
```

Toolchain: Rust stable (rustup), Node 22. Trên macOS nạp cargo bằng `. "$HOME/.cargo/env"` nếu cần.

Chạy app với hub demo của `agent-hub` (FakeRuntime — không gọi `claude`, không tốn tiền), tách khỏi dữ liệu thật:

```bash
AGENT_HUB=~/Workspace/agent-hub scripts/dev-demo.sh
```

Biến môi trường chỉ dành cho phát triển: `CFHARNESS_DESKTOP_DATA_DIR` (thư mục dữ liệu app), `CFHARNESS_DESKTOP_HOMES`
(danh sách HUB_HOME thay cho khám phá), `CFHARNESS_DESKTOP_CFHARNESS` (binary `cfharness`).

Kiểm tra mã Windows từ macOS (không link được, chỉ `check`): `rustup target add x86_64-pc-windows-msvc` rồi
`cargo check --target x86_64-pc-windows-msvc` trong `src-tauri/`. `tauri-build` cần `llvm-rc` để nhúng tài nguyên Windows;
máy không có thì đặt tạm một `llvm-rc` giả vào PATH (chỉ cho `check`). Build Windows thật chạy trên CI (`.github/workflows/ci.yml`).

Quy ước repo, bất biến bảo mật: [`AGENTS.md`](AGENTS.md).
