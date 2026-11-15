export default async function globalTeardown(): Promise<void> {
  const pid = Number(process.env.E2E_DEMO_PID);
  if (!pid) return;
  try {
    process.kill(-pid, "SIGTERM");
  } catch {
    try {
      process.kill(pid, "SIGTERM");
    } catch {
      /* already gone */
    }
  }
}
