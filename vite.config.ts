import { defineConfig } from "vite";

// Shell UI (left rail, onboarding) — bundled locally, the only surface with full IPC.
export default defineConfig({
  root: "shell-ui",
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: { outDir: "../dist", emptyOutDir: true, target: "es2022" },
});
