# inject/ — script khởi tạo cho WebView của hub

Thư mục này chứa script mà app tiêm vào WebView của hub (chỉ origin hub đã đăng ký). **P1 chưa tiêm script nào.**

Kế hoạch (P3, PLAN §3.4 + hợp đồng C4):
- `badge.js` — gọi `GET /app/badge` cùng origin mỗi `settings.badge_poll_s` giây, gửi số nguyên qua IPC `badge_update`.
- cầu nối `window.cfhNative.speech` cho `voice.js` (C4).

Bất biến (PLAN §5.2–5.3): script KHÔNG đọc DOM ngoài việc gọi `/app/badge`, KHÔNG nhận/tiêm token hay credential, chỉ gửi
payload đã kiểm kiểu (số nguyên / sự kiện giọng nói). Thêm lệnh IPC mới cho trang hub = thay đổi bảo mật, cần test đỏ→xanh.
