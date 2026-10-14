// Test-only entry point: mocks Tauri's IPC, then starts the app. Every
// command goes to the Playwright test through `window.__guidesyaInvoke`
// (an exposed binding), where `backend.ts` serves the fixture guides.

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

type Reply = { ok: unknown } | { error: { kind: string; message: string } };

declare global {
  interface Window {
    __guidesyaInvoke(cmd: string, args: unknown): Promise<Reply>;
  }
}

mockWindows("main");
mockIPC(
  async (cmd, args) => {
    // Raw-body commands (add_image_step) pass bytes; they don't survive the
    // binding's JSON, and no test uses them.
    const payload = args instanceof Uint8Array || args instanceof ArrayBuffer ? null : (args ?? null);
    const reply = await window.__guidesyaInvoke(cmd, payload);
    if ("error" in reply) throw reply.error;
    return reply.ok;
  },
  { shouldMockEvents: true },
);

void import("../src/main.tsx");
