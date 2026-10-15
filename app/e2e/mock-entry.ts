// Test-only entry point: mocks Tauri's IPC, then starts the app. Every
// command goes to the Playwright test through `window.__guidesyaInvoke`
// (an exposed binding), where `backend.ts` serves the fixture guides.

import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

type Event = { event: string; payload: unknown };
/** `events` are emitted before the result resolves; `after` once it has (a background run's events). */
type Reply = ({ ok: unknown } | { error: { kind: string; message: string } }) & { events?: Event[]; after?: Event[] };

declare global {
  interface Window {
    __guidesyaInvoke(cmd: string, args: unknown): Promise<Reply>;
    /** Emits an app event, as the Rust side would (tests call it via `emitEvent`). */
    __guidesyaEmit(event: string, payload: unknown): Promise<void>;
  }
}

window.__guidesyaEmit = (event, payload) => emit(event, payload);

// `?page=toolbar` loads the floating recording toolbar (toolbar.html), and
// `?page=region` the region overlay (region.html), instead of the app.
const page = new URLSearchParams(location.search).get("page");
if (page === "toolbar") mockWindows("toolbar", "main");
else if (page === "region") mockWindows("region", "main");
else mockWindows("main");
mockIPC(
  async (cmd, args) => {
    // Raw-body commands (add_image_step) pass bytes; they don't survive the
    // binding's JSON, and no test uses them.
    const payload = args instanceof Uint8Array || args instanceof ArrayBuffer ? null : (args ?? null);
    const reply = await window.__guidesyaInvoke(cmd, payload);
    // Events the command emitted arrive before its result, as from Tauri.
    for (const e of reply.events ?? []) await emit(e.event, e.payload);
    if (reply.after?.length) {
      const after = reply.after;
      setTimeout(() => void (async () => {
        for (const e of after) await emit(e.event, e.payload);
      })());
    }
    if ("error" in reply) throw reply.error;
    return reply.ok;
  },
  { shouldMockEvents: true },
);

// The event API unlistens with `eventId`; the mock removes by `id`. Without
// this, listeners from StrictMode's first mount stay in the mock's list and
// every emit to them logs "Couldn't find callback id".
type Invoke = (cmd: string, args?: unknown, options?: unknown) => Promise<unknown>;
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__;
const mockedInvoke = internals.invoke;
internals.invoke = (cmd, args, options) => {
  if (cmd === "plugin:event|unlisten") args = { ...(args as object), id: (args as { eventId: number }).eventId };
  return mockedInvoke(cmd, args, options);
};

if (page === "toolbar") void import("../src/toolbar/main.tsx");
else if (page === "region") void import("../src/picker/region/main.tsx");
else void import("../src/main.tsx");
