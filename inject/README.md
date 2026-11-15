# inject/ — script khởi tạo cho WebView của hub

`hub.js` được app tiêm (`initialization_script`) vào WebView của từng hub đã đăng ký — xem `src-tauri/src/bridge.rs` (`init_script`).
App thay `__CFH_ORIGIN__` (origin của hub, chuỗi JSON) và `__CFH_POLL_S__` (chu kỳ badge, giây) trước khi tiêm.

Việc script làm (PLAN §3.4, hợp đồng C3/C4):
- chỉ chạy ở khung trên cùng và đúng origin đã nhúng;
- gọi `GET /app/badge` cùng origin theo chu kỳ (cookie của chính hub), gửi `{approvals, questions, running, estop, version}` qua
  lệnh `badge_update`; Rust trả `next_s` (chu kỳ hiện hành trong Cài đặt). 404 (hub cũ chưa có C3) → dừng hỏi;
- `window.cfhNative` (đóng băng, không ghi đè được): `speech.{available,start,stop,on}` cho `voice.js` (C4), `clipboard.writeText`,
  `download.save(name, base64)`; sự kiện giọng nói đến qua `window.__cfhNativeDispatch` (app gọi bằng JSON);
- trang không ở secure context (không có `navigator.clipboard`) → gắn `navigator.clipboard.writeText` đi qua app.

Bất biến (PLAN §5.2–5.3): KHÔNG đọc DOM ngoài việc gọi `/app/badge`, KHÔNG nhận/tiêm token hay credential, chỉ gửi payload mà Rust
kiểm lại (số nguyên/bool/version, tham số giọng nói, chữ clipboard ≤ 1 MB, tệp qua hộp thoại Lưu). Thêm lệnh IPC mới cho trang hub
= thay đổi bảo mật (`bridge::HUB_COMMANDS` + `build.rs` + test đỏ→xanh).
