// E2E trên APP THẬT qua tauri-driver (WebDriver). tauri-driver chỉ hỗ trợ Windows (msedgedriver) và Linux (WebKitWebDriver) —
// KHÔNG có trên macOS (WKWebView không có WebDriver), nên CI chạy file này trên windows-latest; macOS dùng Playwright WebKit
// (tests/e2e/smoke.spec.ts) làm đại diện.
//
//   node tests/e2e/tauri-driver/smoke.mjs <đường dẫn cfharness-desktop.exe>
//
// Cần: `cargo install tauri-driver --locked`, msedgedriver cùng phiên bản WebView2 trong PATH. Không phụ thuộc npm nào
// (W3C WebDriver qua fetch). App chạy với thư mục dữ liệu + danh sách hub tạm (biến dev của app), không đụng dữ liệu thật.
import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const app = process.argv[2];
if (!app) {
  console.error("thiếu đường dẫn binary app");
  process.exit(2);
}
const DRIVER = "http://127.0.0.1:4444";
const work = mkdtempSync(join(tmpdir(), "cfh-tauri-driver-"));
process.env.CFHARNESS_DESKTOP_DATA_DIR = join(work, "appdata");
process.env.CFHARNESS_DESKTOP_HOMES = join(work, "khong-co"); // nonexistent → no local hub discovered (empty rail)
const driver = spawn("tauri-driver", [], { stdio: "inherit", env: process.env });

async function wd(method, path, body) {
  const r = await fetch(DRIVER + path, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(`${method} ${path} → ${r.status} ${JSON.stringify(j)}`);
  return j.value;
}

async function waitDriver() {
  for (let i = 0; i < 60; i++) {
    try {
      await fetch(DRIVER + "/status");
      return;
    } catch {
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  throw new Error("tauri-driver không lên");
}

let session;
let failed = false;
try {
  await waitDriver();
  const s = await wd("POST", "/session", {
    capabilities: { alwaysMatch: { browserName: "wry", "tauri:options": { application: app } } },
  });
  session = s.sessionId;
  const find = async (css) => wd("POST", `/session/${session}/element`, { using: "css selector", value: css });
  // The shell UI (first webview) must render the hub rail and the "+" menu.
  let ok = false;
  for (let i = 0; i < 40 && !ok; i++) {
    try {
      await find("#rail");
      await find("#add");
      ok = true;
    } catch {
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  if (!ok) throw new Error("không thấy thanh hub (#rail, #add) trong shell");
  const title = await wd("GET", `/session/${session}/title`);
  if (title !== "Cfharness Desktop") throw new Error(`tiêu đề cửa sổ shell sai: ${title}`);
  // Shell IPC works (get_state rendered the content area).
  const content = await find("#content");
  const text = await wd("GET", `/session/${session}/element/${Object.values(content)[0]}/text`);
  if (!text.trim()) throw new Error("vùng nội dung trống — shell không nhận được trạng thái từ Rust");
  console.log("tauri-driver smoke: OK —", text.trim().split("\n")[0]);
} catch (e) {
  failed = true;
  console.error("tauri-driver smoke: LỖI —", e.message);
} finally {
  if (session) await wd("DELETE", `/session/${session}`).catch(() => {});
  driver.kill();
}
process.exit(failed ? 1 : 0);
