import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Vite options tailored for Tauri development.
// See https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  // Prevent Vite from obscuring Rust errors.
  clearScreen: false,
  server: {
    // Tauri expects a fixed port; fail if it is taken.
    port: 1430,
    strictPort: true,
    watch: {
      // Don't watch the Rust side of the app.
      ignored: ["**/src-tauri/**"],
    },
  },
  // Env variables starting with TAURI_ENV_* are exposed to the frontend.
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // Safari is the WebView engine on macOS.
    target: "safari15",
    rollupOptions: {
      // The main window, the floating toolbar shown while recording, the
      // overlay a region is drawn on, and the click ring.
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        toolbar: fileURLToPath(new URL("./toolbar.html", import.meta.url)),
        region: fileURLToPath(new URL("./region.html", import.meta.url)),
        ring: fileURLToPath(new URL("./ring.html", import.meta.url)),
      },
    },
  },
});
