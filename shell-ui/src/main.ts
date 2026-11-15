// Shell UI: hub rail + panels shown when no hub webview covers the content area.
// All text is set via textContent (hub names come from disk / the hub CLI — never trusted as HTML).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Status =
  | { state: "idle" }
  | { state: "starting" }
  | { state: "online" }
  | { state: "login" }
  | { state: "error"; message: string };

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
}

interface ShellState {
  hubs: Hub[];
  active: string | null;
  cfharness: string | null;
  read_only: boolean;
  find_open: boolean;
  version: string;
}

const STATUS_LABEL: Record<Status["state"], string> = {
  idle: "chưa mở",
  starting: "đang mở…",
  online: "đang kết nối",
  login: "cần đăng nhập",
  error: "lỗi",
};

// Same source as the hub's own updater (`updater.REPO`).
const HUB_REPO = "lephuong123/agent-hub";
const INSTALL_CMD_MAC = `curl -LsSf https://astral.sh/uv/install.sh | sh\nuv tool install "cfharness @ git+https://github.com/${HUB_REPO}"`;
const INSTALL_CMD_WIN = `powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"\nuv tool install "cfharness @ git+https://github.com/${HUB_REPO}"`;

let state: ShellState | null = null;

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

async function call(cmd: string, args?: Record<string, unknown>): Promise<void> {
  try {
    await invoke(cmd, args);
  } catch (e) {
    console.warn(cmd, e);
  }
}

function renderRail(s: ShellState): void {
  const ul = $("hubs");
  ul.replaceChildren();
  s.hubs
    .filter((h) => !h.hidden)
    .forEach((h, i) => {
      const li = el("li");
      if (h.id === s.active) li.classList.add("active");
      const label = `${h.name} — ${STATUS_LABEL[h.status.state]}${i < 9 ? ` (${navigator.platform.startsWith("Mac") ? "⌘" : "Ctrl+"}${i + 1})` : ""}`;
      const btn = el("button", { type: "button", className: "tile", title: label });
      btn.setAttribute("aria-label", label);
      if (h.id === s.active) btn.setAttribute("aria-current", "page");
      btn.style.background = isColor(h.color) ? h.color : "#7a8699";
      btn.append(h.initial || "?");
      const dot = el("span", { className: `dot ${h.status.state}` });
      dot.setAttribute("aria-hidden", "true");
      const badge = el("span", { className: "badge" });
      badge.hidden = !h.badge;
      if (h.badge) badge.textContent = h.badge > 99 ? "99+" : String(h.badge);
      btn.append(dot, badge);
      btn.addEventListener("click", () => void call("select_hub", { id: h.id }));
      li.append(btn);
      ul.append(li);
    });
}

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
    el("p", { className: "note", textContent: "Bản sau của app sẽ tự cài uv + cfharness sau khi bạn xem kế hoạch và bấm xác nhận." }),
    el("div", { className: "actions" }, button("Kiểm tra lại", () => call("refresh_local"), true)),
  );
}

function noHubs(): HTMLElement {
  return panel(
    el("h1", { textContent: "Chưa có hub local nào" }),
    el("p", { textContent: "Không tìm thấy hub trong ~/.cfharness hoặc danh sách hub của cfharness (~/.config/cfharness/hubs.json). Hub trong thư mục tạm được bỏ qua." }),
    el(
      "div",
      { className: "actions" },
      button("Tạo hub mặc định (~/.cfharness)", () => call("create_default_hub"), true),
      button("Làm mới", () => call("refresh_local")),
    ),
  );
}

function starting(h: Hub): HTMLElement {
  return panel(
    el("div", { className: "spinner" }),
    el("h1", { textContent: `Đang mở ${h.name}…` }),
    el("p", { textContent: h.kind === "local" ? "Khởi động web của hub và đăng nhập tự động." : "Đang tải trang của hub." }),
  );
}

function errorPanel(h: Hub, message: string): HTMLElement {
  return panel(
    el("h1", { textContent: `Không mở được ${h.name}` }),
    el("p", { className: "error-msg", textContent: message }),
    h.home ? el("p", { className: "note", textContent: `HUB_HOME: ${h.home}` }) : "",
    el("div", { className: "actions" }, button("Thử lại", () => call("retry_hub", { id: h.id }), true), button("Làm mới danh sách", () => call("refresh_local"))),
  );
}

function renderContent(s: ShellState): void {
  const c = $("content");
  let node: HTMLElement;
  const active = s.hubs.find((h) => h.id === s.active) ?? null;
  if (!s.cfharness && (!active || active.kind === "local")) {
    node = onboarding();
  } else if (!active) {
    node = s.hubs.length === 0 ? noHubs() : panel(el("h1", { textContent: "Chọn một hub ở thanh bên trái" }));
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
}

function setupAddMenu(): void {
  const add = $("add");
  const menu = $("add-menu");
  const close = () => {
    menu.hidden = true;
    add.setAttribute("aria-expanded", "false");
  };
  add.addEventListener("click", (e) => {
    e.stopPropagation();
    menu.hidden = !menu.hidden;
    add.setAttribute("aria-expanded", String(!menu.hidden));
  });
  document.addEventListener("click", (e) => {
    if (!menu.contains(e.target as Node)) close();
  });
  menu.querySelector('[data-act="refresh"]')?.addEventListener("click", () => {
    close();
    void call("refresh_local");
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

function setupShortcuts(): void {
  // Cmd/Ctrl+1…9 while the shell has focus (the menu handles it when a hub page has focus in P2).
  document.addEventListener("keydown", (e) => {
    if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
    const n = Number(e.key);
    if (!Number.isInteger(n) || n < 1 || n > 9 || !state) return;
    const h = state.hubs.filter((x) => !x.hidden)[n - 1];
    if (h) {
      e.preventDefault();
      void call("select_hub", { id: h.id });
    }
  });
}

async function main(): Promise<void> {
  setupAddMenu();
  setupFind();
  setupShortcuts();
  await listen<ShellState>("shell://state", (e) => render(e.payload));
  render(await invoke<ShellState>("get_state"));
}

void main();
