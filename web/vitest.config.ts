import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
      // Start resolves this per environment; tests run server code directly.
      "@tanstack/react-start/server-only": fileURLToPath(new URL("./test/empty.ts", import.meta.url)),
    },
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
    // Each test migrates a fresh PGlite, which is slow on a busy machine.
    testTimeout: 30_000,
    hookTimeout: 30_000,
  },
});
