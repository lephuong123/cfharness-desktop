import { spawn } from "node:child_process";
import { mkdtempSync, openSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AGENT_HUB, ORIGIN, PORT } from "./hub";

export default async function globalSetup(): Promise<void> {
  const work = mkdtempSync(join(tmpdir(), "cfh-e2e-"));
  const home = join(work, "home");
  const log = join(work, "demo.log");
  const out = openSync(log, "w");
  const child = spawn("uv", ["run", "--quiet", "--project", AGENT_HUB, "python", "-m", "cfharness.web.demo", "--home", home, "--port", String(PORT)], {
    cwd: AGENT_HUB,
    detached: true, // own process group → teardown kills the daemon + web together
    stdio: ["ignore", out, out],
    env: { ...process.env, CFHARNESS_HOME: home },
  });
  child.unref();
  const deadline = Date.now() + 180_000;
  for (;;) {
    const text = readFileSync(log, "utf8");
    if (/^LOGIN /m.test(text)) break;
    if (child.exitCode !== null) throw new Error(`hub demo thoát sớm (mã ${child.exitCode}) — xem ${log}\n${text.slice(-2000)}`);
    if (Date.now() > deadline) throw new Error(`hub demo không lên sau 180 s — xem ${log}`);
    await new Promise((r) => setTimeout(r, 500));
  }
  process.env.E2E_HUB_HOME = home;
  process.env.E2E_DEMO_PID = String(child.pid);
  process.env.E2E_DEMO_LOG = log;
  console.log(`hub demo: ${ORIGIN} (HUB_HOME ${home}, log ${log})`);
}
