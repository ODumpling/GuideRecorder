import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import react from "@vitejs/plugin-react";
import { nitro } from "nitro/vite";
import { defineConfig } from "vite";

/** The local development domain (scripts/dev-domain.sh). */
const DEV_DOMAIN = "app.guiderecorder.dev";

/** Its certificate, once scripts/dev-domain.sh made it; the dev server serves HTTPS with it. */
function devCertificate() {
  const dir = process.env.GUIDERECORDER_DEV_CERTS ?? join(homedir(), ".guiderecorder-dev");
  const cert = join(dir, `${DEV_DOMAIN}.pem`);
  const key = join(dir, `${DEV_DOMAIN}-key.pem`);
  return existsSync(cert) && existsSync(key) ? { cert: readFileSync(cert), key: readFileSync(key) } : undefined;
}

export default defineConfig(({ command }) => ({
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  plugins: [tanstackStart(), react(), tailwindcss(), nitro()],
  server: command === "serve" ? { https: devCertificate(), allowedHosts: [DEV_DOMAIN] } : undefined,
}));
