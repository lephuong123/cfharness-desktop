// Shell UI: hub rail + panels shown when no hub webview covers the content area, and the dialogs of P2
// (import URL / directory, quick switcher, hub management, origin-change confirmation).
// All text is set via textContent (hub names come from disk, the hub CLI or a remote directory — never HTML).
// While a dialog or popup menu is open the shell asks Rust to hide the hub webview (`set_overlay`), because the hub
// webview is drawn above the shell.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Status =
  | { state: "idle" }
  | { state: "starting" }
  | { state: "online" }
  | { state: "login" }
  | { state: "error"; message: string };

type Reach = "online" | "offline" | "tls" | "bad";

interface Hub {
  id: string;
  name: string;
  kind: "local" | "remote";
  color: string;
  initial: string;
  status: Status;
  badge: number | null;
  home: string | null;
  origin: string | null;
  hidden: boolean;
  group: string | null;
  stale: boolean;
  user: string | null;
  reach: Reach | null;
}

interface Dir {
  id: string;
  name: string;
  url: string;
  synced_at: string | null;
  syncing: boolean;
  error: string | null;
  has_token: boolean;
  dismissed: number;
}

interface OriginChange {
  hub: string;
  directory: string;
  name: string;
  from: string;
  to: string;
}

interface ShellState {
  hubs: Hub[];
  directories: Dir[];
  origin_changes: OriginChange[];
  active: string | null;
  cfharness: string | null;
  read_only: boolean;
  find_open: boolean;
  version: string;
}

interface PreviewHub {
  id: string;
  name: string;
  user: string;
  origin: string;
  state: "new" | "exists" | "adopt" | "skip";
  reason: string | null;
}

interface Preview {
  nonce: number;
  kind: "hub" | "directory";
  name: string;
  origin: string | null;
  url: string | null;
  has_token: boolean;
  hubs: PreviewHub[];
  warnings: { index: number; reason: string }[];
  existing: string | null;
  verified: boolean;
}

interface SyncSummary {
  added: number;
  adopted: number;
  updated: number;
  stale: number;
  skipped: number;
  origin_changes: number;
  not_modified: boolean;
  select: string | null;
}

// Same palette as `hubs::PALETTE`.
const PALETTE = ["#d97757", "#5b8def", "#3fa47a", "#b763d1", "#e0a43a", "#4fb3bf", "#e05d7c", "#7a8699"];

// Same source as the hub's own updater (`updater.REPO`).
const HUB_REPO = "lephuong123/agent-hub";
const INSTALL_CMD_MAC = `curl -LsSf https://astral.sh/uv/install.sh | sh\nuv tool install "cfharness @ git+https://github.com/${HUB_REPO}"`;
const INSTALL_CMD_WIN = `powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"\nuv tool install "cfharness @ git+https://github.com/${HUB_REPO}"`;

const IS_MAC = navigator.userAgent.includes("Mac");
const MOD = IS_MAC ? "⌘" : "Ctrl+";

let state: ShellState | null = null;

// ------------------------------------------------------------------------------------------------ helpers

function $(id: string): HTMLElement {
  const e = document.getElementById(id);
  if (!e) throw new Error(`#${id} missing`);
  return e;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: (Node | string)[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  Object.assign(e, props);
  for (const c of children) e.append(c);
  return e;
}

function isColor(c: string): boolean {
  return /^#[0-9a-fA-F]{6}$/.test(c);
}

function errText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Lỗi không xác định";
}

async function call(cmd: string, args?: Record<string, unknown>): Promise<void> {
  try {
    await invoke(cmd, args);
  } catch (e) {
    console.warn(cmd, e);
  }
}

/// Fold case + Vietnamese diacritics for the quick switcher search.
function fold(s: string): string {
  return s.normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/đ/g, "d").replace(/Đ/g, "D").toLowerCase();
}

function fmtTime(iso: string | null): string {
  if (!iso) return "chưa đồng bộ";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "chưa đồng bộ";
  return d.toLocaleString("vi-VN", { dateStyle: "short", timeStyle: "short" });
}

function visibleHubs(s: ShellState): Hub[] {
  return s.hubs.filter((h) => !h.hidden);
}

function dirName(s: ShellState, id: string | null): string | null {
  return id ? (s.directories.find((d) => d.id === id)?.name ?? null) : null;
}

/// Rail status: webview status first; remote hubs not open fall back to the last reachability probe.
function railStatus(h: Hub): { cls: string; label: string } {
  switch (h.status.state) {
    case "error":
      return { cls: "error", label: "lỗi" };
    case "online":
      return { cls: "online", label: "đang kết nối" };
    case "login":
      return { cls: "login", label: "cần đăng nhập" };
    case "starting":
      return { cls: "starting", label: "đang mở…" };
  }
  switch (h.reach) {
    case "online":
      return { cls: "ready", label: "trực tuyến" };
    case "offline":
      return { cls: "offline", label: "mất kết nối" };
    case "tls":
      return { cls: "error", label: "lỗi TLS" };
    case "bad":
      return { cls: "login", label: "không xác nhận được là hub" };
  }
  return { cls: "idle", label: "chưa mở" };
}

function tileButton(h: Hub, size = 44): HTMLButtonElement {
  const btn = el("button", { type: "button", className: "tile" });
  btn.style.background = isColor(h.color) ? h.color : "#7a8699";
  if (size !== 44) {
    btn.style.width = btn.style.height = `${size}px`;
    btn.style.fontSize = `${Math.round(size * 0.4)}px`;
  }
  btn.append(h.initial || "?");
  return btn;
}

// ------------------------------------------------------------------------------------------------ overlay

let modalKind: string | null = null;
let modalRender: (() => HTMLElement) | null = null;
let menuOpen = false;

function syncOverlay(): void {
  void call("set_overlay", { open: modalKind !== null || menuOpen });
}

function closeModal(): void {
  const root = $("modal-root");
  root.hidden = true;
  root.replaceChildren();
  modalKind = null;
  modalRender = null;
  syncOverlay();
  // A pending origin change is asked as soon as nothing else is on screen.
  queueMicrotask(maybeAskOriginChange);
}

function drawModal(): void {
  if (!modalRender) return;
  const root = $("modal-root");
  const focusedId = (document.activeElement as HTMLElement | null)?.id;
  const box = el("div", { className: "dialog" });
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-modal", "true");
  box.append(modalRender());
  const backdrop = el("div", { className: "backdrop" }, box);
  backdrop.addEventListener("mousedown", (e) => {
    if (e.target === backdrop) closeModal();
  });
  root.replaceChildren(backdrop);
  if (focusedId) {
    const f = document.getElementById(focusedId);
    f?.focus();
    if (f instanceof HTMLInputElement && (f.type === "text" || f.type === "search" || f.type === "url")) {
      const n = f.value.length;
      f.setSelectionRange(n, n);
    }
  }
}

function openModal(kind: string, render: () => HTMLElement): void {
  closeMenus();
  modalKind = kind;
  modalRender = render;
  $("modal-root").hidden = false;
  drawModal();
  syncOverlay();
  const first = $("modal-root").querySelector<HTMLElement>("[autofocus], input, button.primary, button");
  first?.focus();
}

function closeMenus(): void {
  $("add-menu").hidden = true;
  $("ctx-menu").hidden = true;
  $("add").setAttribute("aria-expanded", "false");
  if (menuOpen) {
    menuOpen = false;
    syncOverlay();
  }
}

function showMenu(menu: HTMLElement): void {
  $("add-menu").hidden = true;
  $("ctx-menu").hidden = true;
  menu.hidden = false;
  if (!menuOpen) {
    menuOpen = true;
    syncOverlay();
  }
}

// ------------------------------------------------------------------------------------------------ rail

function renderRail(s: ShellState): void {
  const ul = $("hubs");
  ul.replaceChildren();
  let group: string | null | undefined = undefined;
  visibleHubs(s).forEach((h, i) => {
    if (h.group !== group) {
      group = h.group;
      const name = dirName(s, h.group);
      if (name) {
        const sep = el("li", { className: "group", title: `Directory: ${name}` });
        sep.append(el("span", { textContent: name }));
        ul.append(sep);
      } else if (i > 0) {
        ul.append(el("li", { className: "group plain" }));
      }
    }
    const li = el("li");
    if (h.id === s.active) li.classList.add("active");
    const st = railStatus(h);
    const extra = h.stale ? " — không còn trong directory" : "";
    const label = `${h.name} — ${st.label}${extra}${i < 9 ? ` (${MOD}${i + 1})` : ""}`;
    const btn = tileButton(h);
    btn.title = label;
    btn.setAttribute("aria-label", label);
    if (h.stale) btn.classList.add("stale");
    if (h.id === s.active) btn.setAttribute("aria-current", "page");
    const dot = el("span", { className: `dot ${st.cls}` });
    dot.setAttribute("aria-hidden", "true");
    const badge = el("span", { className: "badge" });
    badge.hidden = !h.badge;
    if (h.badge) badge.textContent = h.badge > 99 ? "99+" : String(h.badge);
    btn.append(dot, badge);
    btn.addEventListener("click", () => void call("select_hub", { id: h.id }));
    btn.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      openHubMenu(h, btn);
    });
    li.append(btn);
    ul.append(li);
  });
}

function openHubMenu(h: Hub, anchor: HTMLElement): void {
  const menu = $("ctx-menu");
  const item = (text: string, fn: () => void, disabled = false) => {
    const b = el("button", { type: "button", textContent: text, disabled });
    b.setAttribute("role", "menuitem");
    b.addEventListener("click", () => {
      closeMenus();
      fn();
    });
    return b;
  };
  const info = el("div", { className: "menu-info" });
  info.append(el("strong", { textContent: h.name }));
  const sub = h.kind === "local" ? (h.home ?? "") : [h.origin ?? "", h.user ? `user: ${h.user}` : ""].filter(Boolean).join(" · ");
  info.append(el("span", { textContent: sub }));
  menu.replaceChildren(
    info,
    item("Mở", () => void call("select_hub", { id: h.id })),
    item("Đổi tên…", () => renameDialog(h)),
    item("Đổi màu…", () => colorDialog(h)),
    item("Lên trên", () => void call("move_hub", { id: h.id, delta: -1 })),
    item("Xuống dưới", () => void call("move_hub", { id: h.id, delta: 1 })),
    item("Ẩn khỏi thanh bên", () => void call("set_hub_hidden", { id: h.id, hidden: true })),
    h.kind === "remote" ? item("Xoá khỏi app…", () => removeDialog(h)) : item("Hub local chỉ ẩn được (không xoá)", () => {}, true),
  );
  const r = anchor.getBoundingClientRect();
  menu.style.top = `${Math.max(8, Math.min(r.top, window.innerHeight - 340))}px`;
  menu.style.bottom = "auto";
  showMenu(menu);
  menu.querySelector<HTMLButtonElement>("button")?.focus();
}

// ------------------------------------------------------------------------------------------------ content panels

function panel(...children: (Node | string)[]): HTMLElement {
  return el("div", { className: "panel" }, ...children);
}

function button(text: string, onClick: () => Promise<void> | void, primary = false): HTMLButtonElement {
  const b = el("button", { type: "button", className: primary ? "btn primary" : "btn", textContent: text });
  b.addEventListener("click", async () => {
    b.disabled = true;
    try {
      await onClick();
    } finally {
      b.disabled = false;
    }
  });
  return b;
}

function onboarding(): HTMLElement {
  const isWin = navigator.userAgent.includes("Windows");
  return panel(
    el("h1", { textContent: "Chào mừng đến với Cfharness Desktop" }),
    el("p", { textContent: "Máy này chưa có cfharness — phần mềm chạy hub điều phối đội agent. App cần nó để mở hub local." }),
    el(
      "ol",
      {},
      el("li", { textContent: "Cài uv (trình quản lý Python của Astral) và cfharness bằng lệnh dưới đây trong Terminal (chỉ cài vào thư mục người dùng, không cần quyền quản trị):" }),
      el("pre", { textContent: isWin ? INSTALL_CMD_WIN : INSTALL_CMD_MAC }),
      el("li", { textContent: "Đăng nhập Claude CLI (claude) nếu chưa — đó là việc của bạn, app không làm thay." }),
      el("li", { textContent: "Bấm “Kiểm tra lại”." }),
    ),
    el("p", { className: "note", textContent: "Chỉ dùng hub trên cloud? Bấm “Thêm hub cloud” — không cần cài cfharness." }),
    el("div", { className: "actions" }, button("Kiểm tra lại", () => call("refresh_local"), true), button("Thêm hub cloud / Import URL…", () => importDialog())),
  );
}

function noHubs(): HTMLElement {
  return panel(
    el("h1", { textContent: "Chưa có hub nào" }),
    el("p", { textContent: "Không tìm thấy hub trong ~/.cfharness hoặc danh sách hub của cfharness (~/.config/cfharness/hubs.json). Hub trong thư mục tạm được bỏ qua." }),
    el(
      "div",
      { className: "actions" },
      button("Tạo hub mặc định (~/.cfharness)", () => call("create_default_hub"), true),
      button("Thêm hub cloud / Import URL…", () => importDialog()),
      button("Làm mới", () => call("refresh_local")),
    ),
  );
}

function starting(h: Hub): HTMLElement {
  return panel(
    el("div", { className: "spinner" }),
    el("h1", { textContent: `Đang mở ${h.name}…` }),
    el("p", { textContent: h.kind === "local" ? "Khởi động web của hub và đăng nhập tự động." : "Kiểm tra kết nối rồi tải trang của hub. Hub cloud yêu cầu đăng nhập (mã TOTP) ngay trong trang của hub." }),
  );
}

function errorPanel(h: Hub, message: string): HTMLElement {
  return panel(
    el("h1", { textContent: `Không mở được ${h.name}` }),
    el("p", { className: "error-msg", textContent: message }),
    h.home ? el("p", { className: "note", textContent: `HUB_HOME: ${h.home}` }) : "",
    h.origin && h.kind === "remote" ? el("p", { className: "note", textContent: `Địa chỉ: ${h.origin}` }) : "",
    el(
      "div",
      { className: "actions" },
      button("Thử lại", () => call("retry_hub", { id: h.id }), true),
      h.kind === "local" ? button("Làm mới danh sách", () => call("refresh_local")) : button("Quản lý hub…", () => manageDialog()),
    ),
  );
}

function renderContent(s: ShellState): void {
  const c = $("content");
  let node: HTMLElement;
  const active = s.hubs.find((h) => h.id === s.active) ?? null;
  if (!s.cfharness && (!active || active.kind === "local") && !s.hubs.some((h) => h.kind === "remote")) {
    node = onboarding();
  } else if (!s.cfharness && active?.kind === "local") {
    node = onboarding();
  } else if (!active) {
    node = s.hubs.length === 0 ? noHubs() : panel(el("h1", { textContent: "Chọn một hub ở thanh bên trái" }), el("p", { textContent: `${MOD}K: chuyển nhanh tới hub.` }));
  } else if (active.status.state === "error") {
    node = errorPanel(active, active.status.message);
  } else if (active.status.state === "idle" || active.status.state === "starting") {
    node = starting(active);
  } else {
    node = panel(); // covered by the hub webview
  }
  c.replaceChildren(node);
}

function renderFind(s: ShellState): void {
  const f = $("find") as HTMLFormElement;
  const wasHidden = f.hidden;
  f.hidden = !s.find_open;
  document.body.classList.toggle("find-open", s.find_open);
  if (s.find_open && wasHidden) {
    const q = $("find-q") as HTMLInputElement;
    q.focus();
    q.select();
  }
}

function render(s: ShellState): void {
  state = s;
  renderRail(s);
  renderContent(s);
  renderFind(s);
  if (modalKind === "manage" || modalKind === "switcher") drawModal();
  maybeAskOriginChange();
}

// ------------------------------------------------------------------------------------------------ dialogs

function dialogHead(title: string, close = true): HTMLElement {
  const h = el("div", { className: "dialog-head" }, el("h2", { textContent: title }));
  if (close) {
    const x = el("button", { type: "button", className: "icon", textContent: "✕", title: "Đóng (Esc)" });
    x.addEventListener("click", closeModal);
    h.append(x);
  }
  return h;
}

function importDialog(): void {
  let preview: Preview | null = null;
  let error = "";
  let done: string | null = null;
  let busy = false;
  let input = "";
  let hubName = "";

  const inspect = async () => {
    busy = true;
    error = "";
    preview = null;
    drawModal();
    try {
      preview = await invoke<Preview>("inspect_url", { url: input });
      hubName = preview.name;
    } catch (e) {
      error = errText(e);
    } finally {
      busy = false;
      drawModal();
      document.getElementById("imp-url")?.focus();
    }
  };

  const confirm = async () => {
    if (!preview) return;
    busy = true;
    drawModal();
    try {
      const r = await invoke<SyncSummary>("confirm_import", { nonce: preview.nonce, name: preview.kind === "hub" ? hubName : null });
      if (r.select) {
        closeModal();
        return;
      }
      const parts = [`đã thêm ${r.added} hub`];
      if (r.adopted) parts.push(`gộp ${r.adopted} hub đã có`);
      if (r.skipped) parts.push(`bỏ qua ${r.skipped}`);
      if (r.origin_changes) parts.push(`${r.origin_changes} hub đổi địa chỉ cần bạn xác nhận`);
      done = `Import xong: ${parts.join(", ")}. Bấm vào từng hub ở thanh bên để đăng nhập (mã TOTP của hub đó).`;
      preview = null;
    } catch (e) {
      error = errText(e);
    } finally {
      busy = false;
      drawModal();
    }
  };

  openModal("import", () => {
    const form = el("form", { className: "stack" });
    const field = el("input", { id: "imp-url", type: "url", value: input, placeholder: "https://an.example.com  hoặc  https://admin…/.well-known/cfharness-directory?t=…", autocomplete: "off", spellcheck: false, disabled: busy });
    field.setAttribute("aria-label", "URL của hub hoặc directory");
    field.addEventListener("input", () => {
      input = field.value;
    });
    const go = el("button", { type: "submit", className: "btn primary", textContent: busy ? "Đang kiểm tra…" : "Kiểm tra", disabled: busy });
    form.append(
      el("p", { className: "note", textContent: "Dán origin của một hub (https://ten.mien) để thêm một hub, hoặc URL directory do quản trị máy cloud cấp để thêm cả danh sách. Chỉ chấp nhận https. App không lưu mật khẩu hay mã TOTP — bạn đăng nhập trong trang của từng hub." }),
      el("div", { className: "row" }, field, go),
    );
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      if (!busy) void inspect();
    });
    const body = el("div", { className: "stack" }, dialogHead("Thêm hub / Import URL"), form);
    if (error) body.append(el("p", { className: "error-msg", textContent: error }));
    if (done) {
      body.append(el("p", { className: "ok-msg", textContent: done }), el("div", { className: "actions" }, button("Xong", () => closeModal(), true)));
    }
    if (preview?.kind === "hub") {
      const p = preview;
      const box = el("div", { className: "preview" });
      box.append(el("h3", { textContent: "Hub cfharness" }), el("p", { className: "mono", textContent: p.origin ?? "" }));
      if (!p.verified) box.append(el("p", { className: "warn-msg", textContent: "Không xác nhận được qua /.well-known/cfharness.json — có thể là hub phiên bản cũ. Vẫn thêm được." }));
      if (p.existing) {
        box.append(el("p", { textContent: "Hub này đã có trong app." }), el("div", { className: "actions" }, button("Mở hub", () => confirm(), true)));
      } else {
        const name = el("input", { id: "imp-name", type: "text", value: hubName, maxLength: 60 });
        name.setAttribute("aria-label", "Tên hiển thị");
        name.addEventListener("input", () => {
          hubName = name.value;
        });
        box.append(el("label", { className: "field" }, el("span", { textContent: "Tên hiển thị" }), name), el("div", { className: "actions" }, button("Thêm hub", () => confirm(), true)));
      }
      body.append(box);
    }
    if (preview?.kind === "directory") {
      const p = preview;
      const box = el("div", { className: "preview" });
      const fresh = p.hubs.filter((h) => h.state === "new" || h.state === "adopt").length;
      box.append(el("h3", { textContent: `Directory “${p.name}”` }), el("p", { className: "mono", textContent: p.url ?? "" }));
      if (p.has_token) box.append(el("p", { className: "note", textContent: "Token trong URL được lưu vào kho bí mật của hệ điều hành (Keychain / Credential Manager), không ghi vào hubs.json." }));
      if (p.existing) box.append(el("p", { className: "note", textContent: "Directory này đã có — import lại sẽ cập nhật token và hiện lại các hub bạn đã xoá." }));
      const list = el("ul", { className: "hub-list" });
      const label: Record<PreviewHub["state"], string> = { new: "mới", exists: "đã có", adopt: "gộp với hub đã thêm", skip: "bỏ qua" };
      for (const h of p.hubs) {
        const li = el("li");
        li.append(
          el("span", { className: `tag ${h.state}`, textContent: label[h.state] }),
          el("span", { className: "name", textContent: h.name }),
          el("span", { className: "mono small", textContent: `${h.origin}${h.user ? ` · ${h.user}` : ""}` }),
        );
        if (h.reason) li.append(el("span", { className: "small muted", textContent: h.reason }));
        list.append(li);
      }
      box.append(list);
      if (p.hubs.length === 0) box.append(el("p", { textContent: "Directory không có hub nào." }));
      if (p.warnings.length) {
        const w = el("details", { className: "warnings" });
        w.append(el("summary", { textContent: `${p.warnings.length} mục không hợp lệ đã bị bỏ` }));
        const ul = el("ul");
        for (const x of p.warnings) ul.append(el("li", { textContent: `Mục #${x.index + 1}: ${x.reason}` }));
        w.append(ul);
        box.append(w);
      }
      box.append(el("div", { className: "actions" }, button(fresh ? `Import ${fresh} hub` : "Lưu directory", () => confirm(), true)));
      body.append(box);
    }
    return body;
  });
}

function renameDialog(h: Hub): void {
  openModal("rename", () => {
    const form = el("form", { className: "stack" });
    const input = el("input", { id: "ren-name", type: "text", value: h.name, maxLength: 60 });
    input.setAttribute("aria-label", "Tên mới");
    form.append(
      dialogHead(`Đổi tên “${h.name}”`),
      input,
      el("p", { className: "note", textContent: "Để trống = về tên mặc định (tên do directory đặt, hoặc tên miền)." }),
      el("div", { className: "actions" }, el("button", { type: "submit", className: "btn primary", textContent: "Lưu" }), button("Huỷ", () => closeModal())),
    );
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      void invoke("rename_hub", { id: h.id, name: input.value }).then(closeModal, (err) => alertIn(form, errText(err)));
    });
    queueMicrotask(() => input.select());
    return form;
  });
}

function alertIn(parent: HTMLElement, msg: string): void {
  parent.querySelector(".error-msg")?.remove();
  parent.append(el("p", { className: "error-msg", textContent: msg }));
}

function colorDialog(h: Hub): void {
  openModal("color", () => {
    const body = el("div", { className: "stack" }, dialogHead(`Màu của “${h.name}”`));
    const row = el("div", { className: "swatches" });
    const pick = (c: string) => void invoke("set_hub_color", { id: h.id, color: c }).then(closeModal, (err) => alertIn(body, errText(err)));
    for (const c of PALETTE) {
      const b = el("button", { type: "button", className: "swatch", title: c });
      b.style.background = c;
      if (c === h.color.toLowerCase()) b.classList.add("current");
      b.addEventListener("click", () => pick(c));
      row.append(b);
    }
    const custom = el("input", { type: "color", value: isColor(h.color) ? h.color : "#7a8699", title: "Màu khác" });
    custom.addEventListener("change", () => pick(custom.value));
    row.append(custom);
    body.append(row, el("div", { className: "actions" }, button("Đóng", () => closeModal())));
    return body;
  });
}

function removeDialog(h: Hub): void {
  openModal("remove", () => {
    const body = el("div", { className: "stack" }, dialogHead(`Xoá “${h.name}” khỏi app?`));
    const wipe = el("input", { type: "checkbox", checked: true, id: "rm-wipe" });
    body.append(
      el("p", { textContent: "Hub trên máy chủ không bị ảnh hưởng — chỉ app này bỏ nó khỏi danh sách." + (h.group ? " Hub thuộc directory sẽ không tự thêm lại khi đồng bộ (import lại URL directory để hiện lại)." : "") }),
      el("label", { className: "check" }, wipe, el("span", { textContent: "Xoá luôn dữ liệu đăng nhập của hub này trong app (cookie, phiên, thiết bị tin cậy)" })),
      el(
        "div",
        { className: "actions" },
        button(
          "Xoá",
          async () => {
            try {
              await invoke("remove_hub", { id: h.id, wipe: wipe.checked });
              closeModal();
            } catch (e) {
              alertIn(body, errText(e));
            }
          },
          true,
        ),
        button("Huỷ", () => closeModal()),
      ),
    );
    return body;
  });
}

function removeDirDialog(d: Dir): void {
  const count = state?.hubs.filter((h) => h.group === d.id).length ?? 0;
  openModal("remove-dir", () => {
    const body = el("div", { className: "stack" }, dialogHead(`Xoá directory “${d.name}”?`));
    const rmHubs = el("input", { type: "checkbox", checked: true });
    const wipe = el("input", { type: "checkbox", checked: true });
    body.append(
      el("p", { textContent: "Token của directory sẽ bị xoá khỏi kho bí mật của hệ điều hành." }),
      el("label", { className: "check" }, rmHubs, el("span", { textContent: `Xoá cả ${count} hub của directory (bỏ chọn = giữ lại thành hub riêng lẻ)` })),
      el("label", { className: "check" }, wipe, el("span", { textContent: "Xoá dữ liệu đăng nhập của các hub bị xoá" })),
      el(
        "div",
        { className: "actions" },
        button(
          "Xoá directory",
          async () => {
            try {
              await invoke("remove_directory", { id: d.id, removeHubs: rmHubs.checked, wipe: rmHubs.checked && wipe.checked });
              closeModal();
            } catch (e) {
              alertIn(body, errText(e));
            }
          },
          true,
        ),
        button("Huỷ", () => closeModal()),
      ),
    );
    return body;
  });
}

function manageDialog(): void {
  openModal("manage", () => {
    const s = state;
    const body = el("div", { className: "stack" }, dialogHead("Quản lý hub"));
    if (!s) return body;
    if (s.read_only) body.append(el("p", { className: "warn-msg", textContent: "hubs.json do bản app mới hơn ghi — bản này chỉ đọc, không sửa được." }));
    const groups: (string | null)[] = [null, ...s.directories.map((d) => d.id)];
    for (const g of groups) {
      const hubs = s.hubs.filter((h) => h.group === g);
      const dir = s.directories.find((d) => d.id === g) ?? null;
      if (!hubs.length && !dir) continue;
      const sec = el("section", { className: "manage-group" });
      if (dir) {
        const head = el("div", { className: "dir-head" });
        const meta = el("div");
        meta.append(el("h3", { textContent: `Directory “${dir.name}”` }), el("p", { className: "mono small", textContent: dir.url }));
        const status = dir.syncing ? "đang đồng bộ…" : `đồng bộ lần cuối: ${fmtTime(dir.synced_at)}`;
        meta.append(el("p", { className: "small muted", textContent: `${status}${dir.has_token ? " · có token (Keychain)" : ""}${dir.dismissed ? ` · ${dir.dismissed} hub đã xoá` : ""}` }));
        if (dir.error) meta.append(el("p", { className: "error-msg small", textContent: dir.error }));
        const acts = el("div", { className: "actions tight" });
        const sync = button("Đồng bộ lại", async () => {
          try {
            await invoke("sync_directory", { id: dir.id });
          } catch (e) {
            console.warn(e);
          }
        });
        sync.disabled = dir.syncing;
        acts.append(sync, button("Xoá…", () => removeDirDialog(dir)));
        head.append(meta, acts);
        sec.append(head);
      } else {
        sec.append(el("h3", { textContent: "Hub riêng lẻ" }));
      }
      const ul = el("ul", { className: "manage-list" });
      hubs.forEach((h, i) => {
        const li = el("li");
        if (h.hidden) li.classList.add("is-hidden");
        const info = el("div", { className: "info" });
        info.append(el("span", { className: "name", textContent: h.name + (h.hidden ? " (đang ẩn)" : "") + (h.stale ? " (không còn trong directory)" : "") }));
        info.append(el("span", { className: "mono small", textContent: h.kind === "local" ? (h.home ?? "") : `${h.origin ?? ""}${h.user ? ` · ${h.user}` : ""}` }));
        const acts = el("div", { className: "actions tight" });
        const up = button("↑", () => call("move_hub", { id: h.id, delta: -1 }));
        up.title = "Lên trên";
        up.disabled = i === 0;
        const down = button("↓", () => call("move_hub", { id: h.id, delta: 1 }));
        down.title = "Xuống dưới";
        down.disabled = i === hubs.length - 1;
        acts.append(
          up,
          down,
          button("Tên", () => renameDialog(h)),
          button("Màu", () => colorDialog(h)),
          button(h.hidden ? "Hiện" : "Ẩn", () => call("set_hub_hidden", { id: h.id, hidden: !h.hidden })),
        );
        if (h.kind === "remote") acts.append(button("Xoá", () => removeDialog(h)));
        li.append(tileButton(h, 28), info, acts);
        ul.append(li);
      });
      sec.append(ul);
      body.append(sec);
    }
    body.append(el("div", { className: "actions" }, button("Thêm hub / Import URL…", () => importDialog()), button("Đóng", () => closeModal(), true)));
    return body;
  });
}

function switcherDialog(): void {
  if (modalKind === "switcher") {
    document.getElementById("sw-q")?.focus();
    return;
  }
  let query = "";
  let sel = 0;
  const matches = (): Hub[] => {
    const s = state;
    if (!s) return [];
    const q = fold(query.trim());
    return visibleHubs(s).filter((h) => !q || fold(`${h.name} ${h.origin ?? ""} ${h.user ?? ""} ${h.home ?? ""} ${dirName(s, h.group) ?? ""}`).includes(q));
  };
  const pick = (h: Hub | undefined) => {
    if (!h) return;
    closeModal();
    void call("select_hub", { id: h.id });
  };
  openModal("switcher", () => {
    const list = matches();
    sel = Math.max(0, Math.min(sel, list.length - 1));
    const input = el("input", { id: "sw-q", type: "search", value: query, placeholder: "Tìm hub theo tên, địa chỉ, user…", autocomplete: "off", spellcheck: false });
    input.setAttribute("aria-label", "Tìm hub");
    input.addEventListener("input", () => {
      query = input.value;
      sel = 0;
      drawModal();
    });
    input.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const n = matches().length;
        if (n) sel = (sel + (e.key === "ArrowDown" ? 1 : -1) + n) % n;
        drawModal();
      } else if (e.key === "Enter") {
        e.preventDefault();
        pick(matches()[sel]);
      }
    });
    const ul = el("ul", { className: "switch-list" });
    ul.setAttribute("role", "listbox");
    const all = state ? visibleHubs(state) : [];
    list.forEach((h, i) => {
      const li = el("li");
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", String(i === sel));
      if (i === sel) li.classList.add("sel");
      const st = railStatus(h);
      const n = all.indexOf(h);
      li.append(
        tileButton(h, 24),
        el("span", { className: "name", textContent: h.name }),
        el("span", { className: `small st ${st.cls}`, textContent: st.label }),
        el("span", { className: "small muted kbd", textContent: n >= 0 && n < 9 ? `${MOD}${n + 1}` : "" }),
      );
      li.addEventListener("click", () => pick(h));
      ul.append(li);
    });
    if (!list.length) ul.append(el("li", { className: "muted", textContent: "Không có hub nào khớp." }));
    return el("div", { className: "stack switcher" }, input, ul, el("p", { className: "small muted", textContent: `↑↓ chọn · Enter mở · Esc đóng · ${MOD}⇧[ / ${MOD}⇧] hub trước/sau` }));
  });
  document.getElementById("sw-q")?.focus();
}

let asking: string | null = null;

function maybeAskOriginChange(): void {
  const s = state;
  if (!s || (modalKind !== null && modalKind !== "origin")) return;
  const c = s.origin_changes[0];
  if (!c) {
    if (modalKind === "origin") closeModal();
    return;
  }
  const key = `${c.hub}→${c.to}`;
  if (modalKind === "origin" && asking === key) return;
  asking = key;
  const dname = dirName(s, c.directory) ?? "directory";
  openModal("origin", () => {
    const body = el("div", { className: "stack" }, dialogHead("Directory báo hub đổi địa chỉ", false));
    const answer = async (accept: boolean) => {
      try {
        await invoke("resolve_origin_change", { hub: c.hub, to: c.to, accept });
      } catch (e) {
        alertIn(body, errText(e));
        return;
      }
      asking = null;
      closeModal();
    };
    body.append(
      el("p", { textContent: `Directory “${dname}” cho biết hub “${c.name}” đã chuyển sang địa chỉ khác:` }),
      el("p", { className: "mono", textContent: `Cũ:  ${c.from}` }),
      el("p", { className: "mono strong", textContent: `Mới: ${c.to}` }),
      el("p", { className: "warn-msg", textContent: "Chỉ đồng ý nếu bạn biết việc đổi này (vd quản trị đã báo). Nếu directory bị chiếm quyền, địa chỉ mới có thể là trang giả mạo để lấy mã đăng nhập của bạn." }),
      el("div", { className: "actions" }, button("Giữ địa chỉ cũ", () => answer(false), true), button("Đổi sang địa chỉ mới", () => answer(true))),
    );
    return body;
  });
}

// ------------------------------------------------------------------------------------------------ setup

function setupAddMenu(): void {
  const add = $("add");
  const menu = $("add-menu");
  add.addEventListener("click", (e) => {
    e.stopPropagation();
    if (menu.hidden) {
      menu.style.top = "";
      showMenu(menu);
      add.setAttribute("aria-expanded", "true");
    } else {
      closeMenus();
    }
  });
  document.addEventListener("click", (e) => {
    const t = e.target as Node;
    if (!menu.contains(t) && !$("ctx-menu").contains(t)) closeMenus();
  });
  const on = (act: string, fn: () => void) =>
    menu.querySelector(`[data-act="${act}"]`)?.addEventListener("click", () => {
      closeMenus();
      fn();
    });
  on("import", () => importDialog());
  on("refresh", () => void call("refresh_local"));
  on("manage", () => manageDialog());
  on("sync", () => {
    for (const d of state?.directories ?? []) void call("sync_directory", { id: d.id });
  });
}

function setupFind(): void {
  const f = $("find") as HTMLFormElement;
  const q = $("find-q") as HTMLInputElement;
  const go = (backwards: boolean) => void call("find_in_page", { query: q.value, backwards });
  f.addEventListener("submit", (e) => {
    e.preventDefault();
    go(false);
  });
  q.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.shiftKey) {
      e.preventDefault();
      go(true);
    } else if (e.key === "Escape") {
      e.preventDefault();
      void call("find_close");
    }
  });
  $("find-prev").addEventListener("click", () => go(true));
  $("find-close").addEventListener("click", () => void call("find_close"));
  void listen("shell://find-next", () => {
    if (state?.find_open && q.value) go(false);
  });
}

function setupKeys(): void {
  // Hub shortcuts (Cmd/Ctrl+1…9, K, Shift+[ ]) are menu accelerators handled in Rust, so they work whichever
  // webview has focus. The shell only handles Esc for its own dialogs/menus.
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (menuOpen) {
      e.preventDefault();
      closeMenus();
    } else if (modalKind && modalKind !== "origin") {
      e.preventDefault();
      closeModal();
    }
  });
  void listen<string>("shell://open", (e) => {
    if (modalKind === "origin") return; // answer the origin question first
    if (e.payload === "switcher") switcherDialog();
    else if (e.payload === "import") importDialog();
    else if (e.payload === "manage") manageDialog();
  });
}

async function main(): Promise<void> {
  setupAddMenu();
  setupFind();
  setupKeys();
  await listen<ShellState>("shell://state", (e) => render(e.payload));
  render(await invoke<ShellState>("get_state"));
}

void main();
