# Nhật ký thay đổi — Cfharness Desktop

Mục mới nhất ở trên cùng. Quy trình phát hành: viết mục của phiên bản TRƯỚC khi nâng version/tag (xem `AGENTS.md`).

## Chưa phát hành

### Native: badge, thông báo, tray, deep link, giọng nói, tự cài cfharness (P3)
- Badge trên thanh bên + tray: số yêu cầu duyệt + câu hỏi đang chờ bạn của từng hub (đọc `/app/badge` của hub, chu kỳ chỉnh được);
  chấm đỏ khi hub bật ESTOP.
- Thông báo hệ điều hành khi có yêu cầu duyệt / câu hỏi mới ở hub không ở tiền cảnh — chữ cố định + tên hub trong app; bấm vào mở
  đúng hub ở trang Duyệt (`/approvals`) hoặc Hộp thư (`/inbox`).
- Biểu tượng tray / thanh menu: danh sách hub, hiện/ẩn cửa sổ, thoát. Đóng cửa sổ = ẩn vào tray.
- Deep link `cfharness://hub/<id>/<đường dẫn>` (đường dẫn tương đối, kiểm chặt); mở lần hai → về cửa sổ đang chạy.
- Giọng nói trong app: nút micro / hội thoại giọng nói của trang Điều phối dùng nhận dạng native — macOS `SFSpeechRecognizer`
  tiếng Việt; Windows whisper.cpp (chỉ bản build `--features whisper`, mô hình tải lần đầu sau khi bạn đồng ý).
- Tự cài cfharness: màn chào hiện kế hoạch (tag mới nhất, lệnh, thư mục) → bấm "Cài" → cài `uv` (nếu thiếu) bằng bộ cài Astral và
  `uv tool install cfharness@<tag>`, log trực tiếp, rồi mở hub mặc định. Không sudo. Thiếu Claude CLI → chỉ hướng dẫn.
- Hộp **Cài đặt**: số hub giữ sống, chu kỳ badge, thông báo, mở cùng hệ điều hành (tự khởi động vào tray).
- Nút Sao chép của hub dùng clipboard của app khi trang không ở secure context.
- An toàn: trang hub chỉ gọi được 6 lệnh của app, chỉ từ đúng origin đã đăng ký; micro/clipboard/lưu tệp chỉ cho hub đang hiển thị
  trong cửa sổ đang dùng; mọi dữ liệu trang gửi lên được kiểm kiểu chặt.

### Hub cloud, import URL, chuyển hub (P2)
- Thêm hub cloud / Import URL: dán origin của một hub (`https://…`, xác nhận qua `/.well-known/cfharness.json`) hoặc URL directory
  của máy cloud (`/.well-known/cfharness-directory?t=…`) → xem trước rồi mới thêm. Chỉ https + tên miền, không IP/localhost/path,
  ≤ 200 hub, mục không hợp lệ bị bỏ kèm lý do; không đi theo redirect; tên hiển thị bỏ ký tự điều khiển/bidi.
- Token directory lưu trong Keychain / Credential Manager, `hubs.json` chỉ giữ URL không token.
- Thanh bên nhóm hub theo directory; đồng bộ lại tự động mỗi 6 giờ (ETag) và bằng tay; hub không còn trong directory hiện mờ (không
  xoá); directory đổi địa chỉ của hub đã có → hỏi bạn trước khi đổi.
- Quản lý hub: đổi tên, màu, ẩn/hiện, lên/xuống, xoá (hỏi có xoá dữ liệu đăng nhập không); xoá directory (kèm token, tuỳ chọn xoá hub).
- Chuyển hub: ⌘/Ctrl+1…9, ⌘/Ctrl+K bảng chuyển nhanh, ⌘/Ctrl+Shift+[ ] hub trước/sau (menu Hub); nhớ hub + trang cuối khi mở lại.
- Trạng thái hub cloud: trực tuyến / cần đăng nhập / lỗi TLS / mất kết nối (app kiểm trước khi mở, lỗi hiện rõ thay vì trang trắng).
- Đăng nhập hub cloud là trang `/login` (TOTP) của chính hub trong WebView — app không lưu thông tin đăng nhập.

### Vỏ app + hub local (P1)
- Cửa sổ "Cfharness Desktop" (Tauri v2): thanh hub bên trái (màu, chữ cái đầu, chấm trạng thái, chỗ cho badge), bên phải là
  giao diện web nguyên bản của hub đang chọn.
- Tự tìm hub local: `~/.cfharness` và các hub đã đăng ký trong `~/.config/cfharness/hubs.json`; bỏ qua hub trong thư mục tạm
  hoặc thiếu `config.yaml`. Menu "+" → "Làm mới danh sách hub local" (thêm hub cloud / import directory: sắp có).
- Tự tìm `cfharness` (PATH, `~/.local/bin`, thư mục `uv tool`, `.exe` trên Windows); chưa cài → màn hướng dẫn cài bằng tiếng Việt.
- Mở hub local = chạy `cfharness web --json --ensure` (cần hub có lệnh này), đăng nhập bằng link một lần, về đúng trang xem lần
  trước; phiên hết hạn → tự đăng nhập lại (tối đa 3 lần/2 phút để tránh vòng lặp).
- Mỗi hub một kho cookie/phiên riêng (macOS 14+: data store riêng; Windows: thư mục WebView2 riêng); giữ tối đa 5 WebView sống,
  hub lâu không dùng "ngủ" và mở lại vẫn còn phiên.
- An toàn: WebView của hub chỉ tải đúng origin của hub, link ngoài mở bằng trình duyệt hệ thống, chặn cửa sổ mới và
  `javascript:`/`file:`/`data:`; trang hub không gọi được lệnh nào của app; tải tệp qua hộp thoại Lưu (chỉ từ chính hub).
- Menu tiếng Việt: Tải lại, Phóng to/Thu nhỏ/Cỡ thật, Tìm trong trang (thanh tìm), In, Sửa (cắt/chép/dán), Toàn màn hình.
- User-Agent có thêm ` CfharnessDesktop/<phiên bản>` để hub ghi log (không đổi giao diện).
- macOS: cho phép http chỉ với `localhost` và subdomain (hub local); bản `.app`/`.dmg` ký ad-hoc, tối thiểu macOS 14.
  Windows: bộ cài NSIS cho user hiện tại, tự tải WebView2 nếu thiếu.

### Lưu ý khi cập nhật
- Badge/thông báo cần hub có `/app/badge` (C3); giọng nói cần `voice.js` có cầu nối `window.cfhNative` (C4) — cùng PR phía agent-hub
  với C1. Hub cũ: app vẫn dùng được, chỉ không có badge/giọng nói.
- macOS: lần đầu dùng micro trong app, hệ điều hành hỏi quyền Micro + Nhận dạng giọng nói.
- Cần hub cfharness có `cfharness web --json --ensure` (hợp đồng C1, phía agent-hub). Hub cũ hơn → app báo lỗi "dữ liệu không
  hợp lệ … hãy cập nhật".
