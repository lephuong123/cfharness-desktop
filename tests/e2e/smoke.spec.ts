// E2E smoke trên hub demo (FakeRuntime) bằng WebKit — đại diện cho WKWebView của app trên macOS.
// Nhãn `[id]` trong tên ca là thứ `scripts/parity_routes.py` đối chiếu với `cover: "e2e:<id>"` của tests/e2e/parity.json.
import { expect, test, type Page } from "@playwright/test";
import { loginUrl, ORIGIN, webJson, writeAgentFile } from "./hub";

async function login(page: Page): Promise<void> {
  await page.goto(await loginUrl());
  await expect(page).toHaveURL(`${ORIGIN}/`);
}

test("[c1] cfharness web --json in đúng một dòng JSON theo hợp đồng C1", async () => {
  const j = await webJson();
  expect(j.ok).toBe(true);
  expect(j.origin).toBe(ORIGIN);
  expect(j.login_url).toMatch(new RegExp(`^${ORIGIN.replace(/[.]/g, "\\.")}/login\\?t=[A-Za-z0-9_.-]+$`));
  expect(j.hub_id).toMatch(/^[0-9a-f]{1,16}$/);
  expect(typeof j.version).toBe("string");
  expect(j.started).toBe(false); // the demo's server was already running
});

test("[login] link một lần → Tổng quan; dùng lại link bị từ chối; phiên + khoá phiên được đặt", async ({ page, browser }) => {
  const url = await loginUrl();
  await page.goto(url);
  await expect(page).toHaveURL(`${ORIGIN}/`);
  await expect(page).toHaveTitle(/Tổng quan/);
  // `#k=` → localStorage (khoá phiên ngoài cookie, WT-1); không còn trong URL
  expect(page.url()).not.toContain("#");
  const keys = await page.evaluate(() => Object.keys(localStorage));
  expect(keys.some((k) => k.endsWith("_k"))).toBe(true);
  const cookies = await page.context().cookies();
  const session = cookies.find((c) => c.httpOnly && c.sameSite === "Strict");
  expect(session, "cookie phiên HttpOnly + SameSite=Strict").toBeTruthy();

  const other = await browser.newContext();
  const p2 = await other.newPage();
  await p2.goto(url);
  await expect(p2).toHaveURL(/\/login/);
  await p2.goto(`${ORIGIN}/agents`);
  await expect(p2).toHaveURL(/\/login/);
  await other.close();
});

test("[logout] Thoát (POST /logout có CSRF) → về /login, trang cần phiên không mở được", async ({ page }) => {
  await login(page);
  await page.locator('form[action="/logout"] button[type=submit]').first().click();
  await expect(page).toHaveURL(/\/login/);
  await page.goto(`${ORIGIN}/settings`);
  await expect(page).toHaveURL(/\/login/);
});

test("[wellknown] /.well-known/cfharness.json công khai, chỉ kind/schema/app_api, no-store (C2)", async ({ request }) => {
  const r = await request.get(`${ORIGIN}/.well-known/cfharness.json`);
  expect(r.status()).toBe(200);
  expect(r.headers()["cache-control"]).toContain("no-store");
  expect(await r.json()).toEqual({ kind: "cfharness.hub", schema: 1, app_api: 1 });
});

test("[badge] /app/badge: 401 khi chưa đăng nhập, chỉ số/bool/version khi có phiên (C3)", async ({ page, request }) => {
  const anon = await request.get(`${ORIGIN}/app/badge`);
  expect(anon.status()).toBe(401);
  await login(page);
  const b = await page.evaluate(async () => {
    const r = await fetch("/app/badge", { credentials: "same-origin" });
    return { status: r.status, body: await r.json() };
  });
  expect(b.status).toBe(200);
  expect(Object.keys(b.body).sort()).toEqual(["approvals", "estop", "questions", "running", "version"]);
  for (const k of ["approvals", "questions", "running"]) expect(Number.isInteger(b.body[k])).toBe(true);
  expect(typeof b.body.estop).toBe("boolean");
  expect(b.body.approvals).toBeGreaterThanOrEqual(1); // the demo seeds one approval waiting for a human
});

test("[directory] /.well-known/cfharness-directory: token sai/thiếu → 404 JSON (C5)", async ({ request }) => {
  for (const q of ["", "?t=sai-token"]) {
    const r = await request.get(`${ORIGIN}/.well-known/cfharness-directory${q}`);
    expect(r.status()).toBe(404);
    expect(r.headers()["content-type"]).toContain("application/json");
  }
});

// Trang GET không tham số: mở được với phiên, không bị đẩy về /login, có tiêu đề hub, tài nguyên /static tải được.
const PAGES = [
  "/", "/agents", "/agents/new", "/runs", "/inbox", "/approvals", "/tasks", "/teams", "/learnings", "/mcp", "/mcp/browse",
  "/costs", "/audit", "/settings", "/manager", "/schedules", "/search", "/permissions", "/skills", "/skills/new", "/memory",
  "/memory/graph", "/memory/graph/view", "/update", "/machine", "/admin/overview", "/admin/users", "/admin/alerts", "/admin/audit",
];

test("[pages] mọi trang chính mở được trong WebKit với phiên đăng nhập", async ({ page }) => {
  await login(page);
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`${page.url()}: ${e.message}`));
  let staticOk = 0;
  page.on("response", (r) => {
    if (r.url().startsWith(`${ORIGIN}/static/`) && r.status() < 400) staticOk++;
  });
  for (const p of PAGES) {
    const r = await page.goto(ORIGIN + p);
    // /admin/* chỉ dành cho user OS thuộc nhóm `cfharness-admin` (hub trả 403 có trang lỗi khi không thuộc nhóm — runner CI không thuộc nhóm).
    if (p.startsWith("/admin/") && r?.status() === 403) {
      await expect(page, p).toHaveTitle(/cfharness/);
      continue;
    }
    expect(r?.status(), p).toBeLessThan(400);
    expect(page.url().replace(ORIGIN, "").split("?")[0], p).toBe(p);
    await expect(page, p).toHaveTitle(/cfharness/);
  }
  expect(staticOk).toBeGreaterThan(0);
  expect(errors).toEqual([]);
});

test("[sse] trang Điều phối mở luồng SSE; /sse/firehose trả text/event-stream", async ({ page }) => {
  await login(page);
  const sse = page.waitForResponse((r) => r.url().startsWith(`${ORIGIN}/sse/manager`));
  await page.goto(`${ORIGIN}/manager`);
  const r = await sse;
  expect(r.status()).toBe(200);
  expect(r.headers()["content-type"]).toContain("text/event-stream");
  const first = await page.evaluate(async () => {
    const ctl = new AbortController();
    const res = await fetch("/sse/firehose", { signal: ctl.signal });
    const reader = res.body!.getReader();
    const { value } = await reader.read();
    ctl.abort();
    return { ct: res.headers.get("content-type"), chunk: new TextDecoder().decode(value) };
  });
  expect(first.ct).toContain("text/event-stream");
  expect(first.chunk.length).toBeGreaterThan(0);
});

test("[office] /office/state: 401 khi chưa đăng nhập, JSON khi có phiên; /office là trang 3D độc lập", async ({ page, request }) => {
  expect((await request.get(`${ORIGIN}/office/state`)).status()).toBe(401);
  await login(page);
  const s = await page.evaluate(async () => (await fetch("/office/state")).status);
  expect(s).toBe(200);
  const r = await page.goto(`${ORIGIN}/office`);
  expect(r?.status()).toBe(200);
  await expect(page.locator("canvas").first()).toBeAttached({ timeout: 15_000 });
});

test("[confirm-page] không có hộp <dialog>: server hiện trang xác nhận; Huỷ thì không làm gì (G11)", async ({ page }) => {
  // Hub bắt buộc khoá phiên (JS) cho mọi POST (WT-1) nên không có đường "tắt JS"; thay vào đó bỏ `data-confirm` để form đi
  // thẳng lên server (như WebView không có <dialog>) → server phải trả trang xác nhận thay vì làm luôn.
  await login(page);
  await page.goto(`${ORIGIN}/settings`);
  const estop = page.locator('form[action="/do/estop"]').first();
  await estop.evaluate((f) => f.removeAttribute("data-confirm"));
  await estop.locator("button[type=submit]").click();
  await expect(page.locator(".confirm-msg")).toBeVisible();
  await expect(page.locator('form input[name="confirm"][value="yes"]')).toHaveCount(1);
  await page.locator("a.btn.ghost", { hasText: "Huỷ" }).click();
  await expect(page).toHaveURL(/\/settings/);
  const badge = await page.evaluate(async () => (await (await fetch("/app/badge")).json()).estop);
  expect(badge).toBe(false); // Huỷ → ESTOP vẫn tắt
});

test("[confirm] duyệt thao tác: hộp xác nhận (dialog) → bước 2 nếu là thao tác nặng → đã cho phép", async ({ page }) => {
  await login(page);
  await page.goto(`${ORIGIN}/approvals`);
  const form = page.locator('form[action="/do/approve"]').first();
  await expect(form).toBeVisible();
  await form.locator("button[type=submit]").click();
  const dlg = page.locator("#confirm-dlg");
  await expect(dlg).toBeVisible();
  await expect(page.locator("#confirm-msg")).toContainText("rm -rf /srv/data/tmp-2025");
  await page.locator("#confirm-ok").click();
  await page.waitForLoadState("load");
  const step2 = page.locator('form:has(input[name="confirm2"][value="yes"])');
  if (await step2.count()) await step2.first().locator("button[type=submit]").click();
  await expect(page.getByText("Đã cho phép thao tác")).toBeVisible();
});

test("[download] tải tệp (Content-Disposition: attachment) từ trình xem file → sự kiện download đúng tên + nội dung (G3)", async ({ page }) => {
  const name = "bao-cao-e2e.csv";
  const content = "cot,gia_tri\nmot,1\nhai,2\n";
  writeAgentFile("lap-trinh", name, content);
  await login(page);
  await page.goto(`${ORIGIN}/files/view?agent=lap-trinh&path=${name}`);
  await expect(page.getByText("mot,1")).toBeVisible();
  const [dl] = await Promise.all([page.waitForEvent("download"), page.locator('a[href*="/files/raw?"][href*="dl=1"]').first().click()]);
  expect(dl.suggestedFilename()).toBe(name);
  const path = await dl.path();
  const { readFileSync } = await import("node:fs");
  expect(readFileSync(path, "utf8")).toBe(content);
});

test("[securectx] cfharness.localhost là secure context trong WebKit: navigator.clipboard có sẵn (G5, docs/spikes/P0.md)", async ({ page }) => {
  await login(page);
  const probe = await page.evaluate(() => ({
    secure: window.isSecureContext,
    clipboard: typeof navigator.clipboard?.writeText === "function",
  }));
  expect(probe.secure).toBe(true);
  expect(probe.clipboard).toBe(true);
});
