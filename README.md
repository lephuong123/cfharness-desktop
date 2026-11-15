# Cfharness Desktop

App desktop (macOS + Windows) cho [cfharness](https://github.com/lephuong123/agent-hub): một cửa sổ, thanh hub bên trái, bên phải
là **chính giao diện web của từng hub** (không viết lại UI) — nên mọi trang, mọi thao tác của web đều có trong app ngay khi hub
cập nhật. Kế hoạch đầy đủ và các quyết định đã chốt: [`docs/PLAN.md`](docs/PLAN.md).

Trạng thái: **P1 (vỏ + hub local)**, **P2 (hub cloud, import directory, chuyển hub)**, **P3 (badge, thông báo, tray, deep link,
giọng nói, tự cài cfharness)**, **P4 (đóng gói, tự cập nhật ký ed25519, CI/phát hành)** và **P5 (kiểm ngang chức năng tự động)** đã có
— xem [`CHANGELOG.md`](CHANGELOG.md). Chưa có bản phát hành chính thức (repo chưa có remote): chưa kiểm trên máy Windows thật.

## Yêu cầu

- macOS 14 trở lên (cần cho kho dữ liệu WebView riêng theo hub), hoặc Windows 10 21H2+ / Windows 11 (WebView2 Evergreen — bộ cài
  tự tải nếu máy chưa có).
- `cfharness` cho user hiện tại. Chưa có thì app **tự cài sau khi bạn xác nhận** (xem "Tự cài cfharness" dưới). Claude CLI
  (`claude`) cài và đăng nhập là việc của bạn — app chỉ hướng dẫn.

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

Gói tự cập nhật của app vẫn được ký ed25519 — app từ chối gói sai chữ ký, độc lập với việc ký của hệ điều hành.

Cỡ (bản đo trên máy dev, v0.1.0): `.dmg` universal ~6,9 MB (`.app` ~16 MB, gói cập nhật `.app.tar.gz` ~7 MB); bản chỉ arm64:
`.dmg` ~3,4 MB, `.app` ~7,6 MB.

## Cập nhật

- App tự kiểm tra bản mới **mỗi ngày một lần** (tắt được) từ trang Releases của repo app, đọc `latest.json` của kênh bạn chọn:
  **Ổn định** (bản phát hành mới nhất) hoặc **Thử nghiệm (beta)** (gồm cả bản `-beta`). Có bản mới → nút **↑** xanh hiện trên thanh
  bên; menu **Kiểm tra cập nhật…** (macOS: menu Cfharness Desktop; Windows: Trợ giúp) hoặc **Cài đặt → Cập nhật app** để xem ghi chú
  phát hành và bấm **Cập nhật lên X.Y.Z**. App **không bao giờ tự cài** khi bạn chưa bấm.
- App tải gói, **kiểm chữ ký ed25519** bằng khoá công khai nhúng trong app (sai chữ ký → từ chối, không cài), cài rồi tự khởi động lại.
  Windows: bộ cài NSIS chạy ở chế độ hiện tiến trình (passive) rồi mở lại app.
- macOS (bản ký ad-hoc): mỗi bản có chữ ký khác nhau nên macOS **có thể hỏi lại quyền Micro / Nhận dạng giọng nói** sau khi cập
  nhật — cứ cho phép lại. Nếu đã từ chối nhầm: Cài đặt hệ thống → Quyền riêng tư & Bảo mật → Micro / Nhận dạng giọng nói.
- Bản tự build (không có khoá công khai) hiện "Tự cập nhật tắt" — cài bản mới bằng tay từ trang Releases.
- Cập nhật `cfharness` (hub) là việc riêng của hub: `cfharness update` / nút "Cập nhật hệ thống" trong Cài đặt của hub.

## Gỡ cài đặt

- **macOS**: thoát app (menu tray → Thoát hoặc ⌘Q), kéo `/Applications/Cfharness Desktop.app` vào Thùng rác. Xoá luôn dữ liệu app
  (danh sách hub, phiên đăng nhập trong WebView):
  ```bash
  rm -rf "$HOME/Library/Application Support/io.cfharness.desktop" "$HOME/Library/Caches/io.cfharness.desktop" \
         "$HOME/Library/WebKit/io.cfharness.desktop" "$HOME/Library/Preferences/io.cfharness.desktop.plist"
  ```
  Token directory nằm trong Keychain (dịch vụ `io.cfharness.desktop`) — xoá bằng Keychain Access nếu muốn. Đã bật "Mở cùng hệ
  điều hành": tắt trong Cài đặt trước khi gỡ (hoặc xoá tệp `.plist` của Cfharness Desktop trong `~/Library/LaunchAgents/`).
- **Windows**: Cài đặt → Ứng dụng → **Cfharness Desktop** → Gỡ cài đặt (hoặc "Uninstall" trong Start). Dữ liệu app:
  `%APPDATA%\io.cfharness.desktop` và `%LOCALAPPDATA%\io.cfharness.desktop` (WebView2) — bộ gỡ hỏi có xoá dữ liệu không, hoặc xoá tay.
  Token directory trong Credential Manager (mục `io.cfharness.desktop`).
- Gỡ app **không** đụng tới hub: `~/.cfharness`, `cfharness` và dữ liệu của hub vẫn nguyên.

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

### Tự cài cfharness

Máy chưa có `cfharness` → màn chào có nút **Xem kế hoạch cài**. App đọc bản phát hành mới nhất của
[`lephuong123/agent-hub`](https://github.com/lephuong123/agent-hub/releases) (cùng nguồn với nút "Cập nhật hệ thống" của hub) và
hiện **đúng các lệnh sẽ chạy**, thư mục cài, tình trạng `uv` / `git` / `claude`. Chỉ khi bạn bấm **Cài** app mới chạy:
1. chưa có `uv` → bộ cài chính thức của Astral (macOS: `curl -LsSf https://astral.sh/uv/install.sh | sh`; Windows: PowerShell
   `irm https://astral.sh/uv/install.ps1 | iex`);
2. `uv tool install --force "cfharness @ git+https://github.com/lephuong123/agent-hub@vX.Y.Z"`;
3. `cfharness doctor` (chỉ để xem), rồi mở hub mặc định `~/.cfharness` (`cfharness web --json --ensure`).

Log hiện trực tiếp trên màn hình. Không bao giờ dùng sudo/quyền quản trị — chỉ cài vào thư mục người dùng (`~/.local/bin`); app từ
chối cài khi đang chạy bằng root. Cần `git` (macOS: `xcode-select --install`; Windows: Git for Windows) — thiếu thì app báo trước.
Sau đó cập nhật `cfharness` qua cơ chế của hub (`cfharness update` / nút "Cập nhật hệ thống").

### Badge, thông báo, tray, deep link

- Trang của mỗi hub đang mở hỏi `/app/badge` (chu kỳ đặt trong **Cài đặt**, mặc định 10 giây) → số **duyệt + câu hỏi** đang chờ bạn
  hiện ở thanh bên và trong menu tray; chấm đỏ phát sáng = ESTOP đang bật. Hub đang "ngủ" giữ số cuối cùng biết được.
- Số duyệt/câu hỏi **tăng** khi hub không ở tiền cảnh (đang xem hub khác, cửa sổ ẩn/không focus) → thông báo hệ điều hành với chữ
  cố định, vd "Hub *Local*: 2 yêu cầu duyệt mới" (tên là tên bạn đặt trong app — không bao giờ có chữ của agent). Bấm thông báo →
  chuyển sang hub đó và mở `/approvals` (câu hỏi: `/inbox`). Tắt trong **Cài đặt**.
- **Tray / thanh menu**: danh sách hub (bấm → mở hub), Hiện/Ẩn cửa sổ, Thoát. Nút đóng cửa sổ chỉ **ẩn** app vào tray (thông báo
  vẫn chạy); thoát hẳn bằng ⌘Q / menu tray "Thoát". ESTOP không có trong tray — đó là một trang của hub.
- **Deep link** `cfharness://hub/<id>/<đường dẫn>`, vd `cfharness://hub/loc-3f2a/approvals`: `<id>` là id của hub trong app
  (`loc-…`/`rem-…`) hoặc `hub_id` của hub; đường dẫn phải tương đối trong hub (không `//`, `\`, `..`, `#…`, không `/login`).

### Giọng nói

Nút micro / hội thoại giọng nói trong trang Điều phối của hub dùng **nhận dạng giọng nói native** của app (WebView không có Web
Speech API):
- **macOS**: `SFSpeechRecognizer` tiếng Việt (vi-VN), chạy ngay trên máy khi macOS hỗ trợ cho ngôn ngữ đó. Lần đầu macOS hỏi quyền
  **Micro** và **Nhận dạng giọng nói**; từ chối thì trang hướng dẫn mở Cài đặt hệ thống → Quyền riêng tư. Bản ad-hoc có thể bị hỏi
  lại sau mỗi lần cập nhật. Chỉ hub đang hiển thị trong cửa sổ đang dùng mới mở được micro; chuyển hub/ẩn cửa sổ → micro tắt.
- **Windows**: whisper.cpp (mô hình `ggml-small-q5_1`, ~190 MB, tải lần đầu từ huggingface.co **sau khi bạn bấm "Tải"**, kiểm
  SHA-256) — chỉ có trong bản build với `--features whisper` (xem Phát triển). Bản thường: nút micro của hub ẩn như trên trình duyệt
  không hỗ trợ.

### Cài đặt

**Cài đặt…** (macOS: menu Cfharness Desktop, ⌘,; Windows: menu Hub, Ctrl+,; hoặc "+" → Cài đặt…): số hub giữ sống cùng lúc (1–20),
chu kỳ badge (5–300 giây), bật/tắt thông báo, **mở cùng hệ điều hành** (khởi động thu nhỏ vào tray), **Cập nhật app** (kênh, tự kiểm
tra mỗi ngày, kiểm tra ngay).

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

> **Chỉ đăng nhập hub demo khi chạy bản dev trên macOS.** Bản không đóng gói không có bundle id, nên WebKit lưu cookie phiên
> của từng hub ở `~/Library/WebKit/cfharness-desktop/` và `~/Library/HTTPStorages/cfharness-desktop/` — ngoài vùng hub chặn agent
> (C6/H5 chỉ chặn `io.cfharness.desktop`), và `CFHARNESS_DESKTOP_DATA_DIR` không đổi được chỗ này. Agent trên máy đọc được các thư mục
> đó. Dùng hub thật thì chạy bản đã đóng gói (`npm run tauri build`). Sau khi dev: `rm -rf ~/Library/WebKit/cfharness-desktop
> ~/Library/HTTPStorages/cfharness-desktop`.

Biến môi trường chỉ dành cho phát triển: `CFHARNESS_DESKTOP_DATA_DIR` (thư mục dữ liệu app), `CFHARNESS_DESKTOP_HOMES`
(danh sách HUB_HOME thay cho khám phá), `CFHARNESS_DESKTOP_CFHARNESS` (binary `cfharness`), `CFHARNESS_DESKTOP_TOKEN_STORE=memory` (token directory chỉ giữ trong RAM,
không đụng Keychain).

Giọng nói Windows (whisper.cpp) nằm sau feature `whisper` (cần `cmake` + trình biên dịch C/C++): `cargo build --features whisper`.
Trên macOS lệnh `cargo clippy --features whisper` vẫn biên dịch được module (không dùng) để kiểm mã; CI Windows chạy clippy với
feature này. **Chưa kiểm trên máy Windows thật** (micro, độ trễ, chất lượng tiếng Việt).

Kiểm tra mã Windows từ macOS (không link được, chỉ `check`): `rustup target add x86_64-pc-windows-msvc` rồi
`cargo check --target x86_64-pc-windows-msvc` trong `src-tauri/`. `tauri-build` cần `llvm-rc` để nhúng tài nguyên Windows;
máy không có thì đặt tạm một `llvm-rc` giả vào PATH (chỉ cho `check`). Các phụ thuộc mạng/kho bí mật được chọn để `check` chéo
không cần trình biên dịch C (reqwest `native-tls` = SChannel, keyring `windows-native`). Build Windows thật chạy trên CI (`.github/workflows/ci.yml`).

### Đóng gói

```bash
npm run tauri build -- --bundles app                    # .app (ký ad-hoc) ở src-tauri/target/release/bundle/macos/
CI=true npm run tauri build -- --bundles dmg            # .dmg; CI=true bỏ bước trang trí cửa sổ Finder (AppleScript)
npm run tauri build -- --target universal-apple-darwin --bundles app,dmg   # universal (cần rustup target x86_64-apple-darwin)
```
Không có `CI=true`, bước `.dmg` dùng AppleScript điều khiển Finder và có thể hỏng ("AppleEvent timed out") trong phiên không có
quyền Automation. Windows (`--bundles nsis`) build trên CI.

### Kiểm ngang chức năng + e2e (P5)

```bash
npm run parity                 # = python3 scripts/parity_routes.py [--hub ../agent-hub]: route/ACTIONS/OFFICE_ACTIONS của web hub
                               #   so với tests/e2e/parity.json — mục mới chưa có → thoát 1 (CI đỏ)
python3 scripts/parity_routes.py --write   # thêm mục mới với cover "TODO" để điền (vẫn đỏ tới khi điền)
npx playwright install webkit  # một lần
npm run e2e                    # Playwright WebKit trên hub demo của agent-hub (khởi động tự động, FakeRuntime)
python3 -m unittest scripts/test_parity_routes.py scripts/test_release_tools.py
```
- `parity_routes.py` đọc **tĩnh** mã nguồn `agent-hub/src/cfharness/web/*.py` (không import, không cần venv của hub). Mỗi mục trong
  `parity.json` có `cover`: `e2e:<id>` (ca `[id]` trong `tests/e2e/*.spec.ts`, script kiểm ca có thật), `structural` (WebView hiển thị
  chính trang/thao tác của hub), `native:…`, `manual:…` (checklist tay PLAN §9).
- E2E Playwright dùng **WebKit làm đại diện cho WKWebView** (cùng engine, không phải app thật): đăng nhập link một lần (xin qua chính
  C1 `cfharness web --json`), thoát, C2/C3/C5, mọi trang chính, SSE, office 3D, trang xác nhận + duyệt có bước 2, tải tệp.
  Biến: `AGENT_HUB` (mặc định `../agent-hub`), `E2E_PORT` (mặc định 8799).
- **E2E trên app thật** (`tests/e2e/tauri-driver/smoke.mjs`, WebDriver qua `tauri-driver`) **chỉ chạy trên Windows CI**: macOS không
  có WebDriver cho WKWebView. Trên CI job `parity-e2e` chạy khi đặt biến repo `AGENT_HUB_REPO` (+ secret `AGENT_HUB_TOKEN` nếu repo
  hub riêng tư).

### Phát hành và khoá ký cập nhật

Quy trình (AGENTS.md): viết mục CHANGELOG **trước** → nâng version (`package.json`, `src-tauri/Cargo.toml`,
`src-tauri/tauri.conf.json`) → tag `vX.Y.Z` → `.github/workflows/release.yml` build `.dmg` universal (ad-hoc) + NSIS `.exe`, ký gói
cập nhật, tạo GitHub Release với thân = đúng mục CHANGELOG, cập nhật `latest.json` (kênh ổn định = release mới nhất; kênh beta =
release cuộn tên `beta`). Tag có hậu tố (`v0.2.0-beta.1`) = pre-release, chỉ kênh beta thấy.

Thiết lập một lần cho repo app trên GitHub:
1. Sinh cặp khoá updater **trên máy của người phát hành** (không bao giờ commit khoá riêng):
   ```bash
   npx tauri signer generate -w ~/.tauri/cfharness-desktop.key
   ```
   In ra khoá công khai (cũng ở `~/.tauri/cfharness-desktop.key.pub`). Giữ khoá riêng + mật khẩu an toàn — **mất khoá riêng là mọi
   bản đã cài không cập nhật được nữa** (phải cài tay bản mới có khoá mới).
2. Settings → Secrets and variables → Actions:
   - secret `TAURI_SIGNING_PRIVATE_KEY` = nội dung file khoá riêng, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = mật khẩu;
   - variable `TAURI_UPDATER_PUBKEY` = nội dung file `.pub` (một dòng base64).
3. Endpoint cập nhật: `tauri.conf.json` chỉ chứa placeholder (`CFHARNESS_UPDATER_PUBKEY_PLACEHOLDER`,
   `https://github.com/OWNER/REPO/releases/latest/download/latest.json`). Lúc phát hành, `scripts/release_tools.py config` thay bằng
   khoá ở bước 2 và `https://github.com/<owner>/<repo>/…` của chính repo chạy workflow (`GITHUB_REPOSITORY`) — không cần sửa tay.
   Muốn build có cập nhật trên máy: `TAURI_UPDATER_PUBKEY=… python3 scripts/release_tools.py config --out src-tauri/release.conf.json
   --repo <owner>/<repo>` rồi `npm run tauri build -- --config src-tauri/release.conf.json` (cần `TAURI_SIGNING_PRIVATE_KEY`).
4. Ký hệ điều hành (khi có chứng chỉ): bật biến `APPLE_SIGN=true` (+ `APPLE_NOTARIZE=true`) hoặc `WINDOWS_SIGN=true` và thêm các
   secret ghi ở đầu `release.yml`. Mặc định tắt.

Quy ước repo, bất biến bảo mật: [`AGENTS.md`](AGENTS.md).
