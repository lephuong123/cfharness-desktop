// Hub demo của agent-hub cho e2e (FakeRuntime — không gọi claude thật, không tốn tiền).
// global-setup.ts khởi động `python -m cfharness.web.demo` trong một HUB_HOME tạm và để lại thông tin qua biến môi trường;
// mỗi ca xin link đăng nhập MỚI bằng chính hợp đồng C1 của app (`cfharness web --json`), giống app desktop làm với hub local.
import { execFile } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const run = promisify(execFile);
const HERE = dirname(fileURLToPath(import.meta.url));

export const AGENT_HUB = resolve(process.env.AGENT_HUB ?? join(HERE, "..", "..", "..", "agent-hub"));
export const PORT = Number(process.env.E2E_PORT ?? 8799);
export const ORIGIN = `http://cfharness.localhost:${PORT}`;

export function hubHome(): string {
  const h = process.env.E2E_HUB_HOME;
  if (!h) throw new Error("E2E_HUB_HOME chưa đặt — chạy qua `npm run e2e` (global-setup khởi động hub demo)");
  return h;
}

export interface WebJson {
  ok: boolean;
  origin?: string;
  login_url?: string;
  hub_id?: string;
  version?: string;
  name?: string;
  started?: boolean;
  error?: string;
}

/** C1 without --ensure: the demo's web server is already running. */
export async function webJson(): Promise<WebJson> {
  const { stdout } = await run("uv", ["run", "--quiet", "--project", AGENT_HUB, "cfharness", "web", "--json"], {
    env: { ...process.env, CFHARNESS_HOME: hubHome() },
    timeout: 60_000,
  });
  const lines = stdout.trim().split("\n");
  if (lines.length !== 1) throw new Error(`C1 phải in đúng MỘT dòng JSON, nhận ${lines.length}`);
  return JSON.parse(lines[0]) as WebJson;
}

export async function loginUrl(): Promise<string> {
  const j = await webJson();
  if (!j.ok || !j.login_url) throw new Error(`cfharness web --json lỗi: ${j.error}`);
  return j.login_url;
}

/** A file in the agent's own workspace root (`<home agent>/workspaces/`) — what `file.read` serves for `agent=<id>`. */
export function writeAgentFile(agent: string, name: string, content: string): void {
  const dir = join(hubHome(), "agents", agent, "workspaces");
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, name), content, "utf8");
}
