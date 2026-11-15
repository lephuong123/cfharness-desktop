# Cfharness Desktop

App desktop (macOS + Windows) cho [cfharness](https://github.com/lephuong123/agent-hub): một cửa sổ, thanh hub bên trái, bên phải
là **chính giao diện web của từng hub** (không viết lại UI) — nên mọi trang, mọi thao tác của web đều có trong app ngay khi hub
cập nhật. Kế hoạch đầy đủ và các quyết định đã chốt: [`docs/PLAN.md`](docs/PLAN.md).

Trạng thái: **P1 (vỏ + hub local)** và **P2 (hub cloud, import directory, chuyển hub)** đã có — xem [`CHANGELOG.md`](CHANGELOG.md).

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

### Hub cloud và directory

- **+ → Thêm hub cloud / Import URL…** (hoặc ⌘⇧N / Ctrl+Shift+N). Dán một trong hai loại URL rồi bấm **Kiểm tra**:
  - **origin của một hub**, vd `https://an.example.com` → app hỏi `/.well-known/cfharness.json` để xác nhận đó là hub cfharness
    rồi thêm một hub;
  - **URL directory** do quản trị máy cloud cấp (`cfharness admin directory token` in ra), vd
    `https://admin.example.com/.well-known/cfharness-directory?t=…` → app kiểm từng mục và thêm cả danh sách, nhóm theo directory
    ở thanh bên.
- Chỉ chấp nhận **https** với tên miền (không http, không địa chỉ IP, không `localhost`); directory tối đa 200 hub, mục sai bị bỏ
  và liệt kê lý do. App không đi theo chuyển hướng (redirect).
- Token `t=` của directory được lưu trong **Keychain (macOS) / Credential Manager (Windows)**; `hubs.json` chỉ giữ URL không có token.
- Đăng nhập hub cloud là trang `/login` (mã TOTP) **của chính hub** trong WebView của nó — app không lưu mật khẩu hay mã TOTP.
  Cookie phiên và "thiết bị tin cậy" nằm trong kho WebView riêng của từng hub.
- **Đồng bộ lại**: tự động mỗi 6 giờ (dùng ETag) và bấm tay (Tệp → Đồng bộ lại các directory, hoặc Quản lý hub). Hub biến mất khỏi
  directory được đánh dấu "không còn trong directory" (hiện mờ), không bị xoá. Directory đổi địa chỉ của một hub đã có → app **hỏi
  bạn trước** (mặc định giữ địa chỉ cũ) để chống directory bị chiếm hướng sang trang giả mạo.
- Chấm trạng thái ở thanh bên: xanh = đang kết nối/trực tuyến, vàng = cần đăng nhập, đỏ = lỗi (vd **lỗi TLS**), xám = mất kết nối.

### Quản lý và chuyển hub

- Chuột phải vào một hub (hoặc **Hub → Quản lý hub…**): đổi tên, đổi màu, lên/xuống trong nhóm, ẩn/hiện, xoá. Hub local chỉ ẩn được
  (app tự tìm lại); xoá hub cloud thì hỏi có xoá luôn dữ liệu đăng nhập của hub đó không. Hub thuộc directory đã xoá sẽ không tự
  thêm lại khi đồng bộ (import lại URL directory để hiện lại).
- Phím tắt (menu **Hub**, dùng được cả khi đang ở trong trang hub): ⌘1…⌘9 chọn hub theo thứ tự thanh bên, ⌘K chuyển nhanh (tìm
  theo tên, địa chỉ, user — không cần gõ dấu), ⌘⇧[ / ⌘⇧] hub trước/sau. Windows: Ctrl thay ⌘.
- Mở lại app → về hub đang chọn và trang xem lần cuối của từng hub.

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
(danh sách HUB_HOME thay cho khám phá), `CFHARNESS_DESKTOP_CFHARNESS` (binary `cfharness`), `CFHARNESS_DESKTOP_TOKEN_STORE=memory` (token directory chỉ giữ trong RAM,
không đụng Keychain).

Kiểm tra mã Windows từ macOS (không link được, chỉ `check`): `rustup target add x86_64-pc-windows-msvc` rồi
`cargo check --target x86_64-pc-windows-msvc` trong `src-tauri/`. `tauri-build` cần `llvm-rc` để nhúng tài nguyên Windows;
máy không có thì đặt tạm một `llvm-rc` giả vào PATH (chỉ cho `check`). Các phụ thuộc mạng/kho bí mật được chọn để `check` chéo
không cần trình biên dịch C (reqwest `native-tls` = SChannel, keyring `windows-native`). Build Windows thật chạy trên CI (`.github/workflows/ci.yml`).

Quy ước repo, bất biến bảo mật: [`AGENTS.md`](AGENTS.md).
