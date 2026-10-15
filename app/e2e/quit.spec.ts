import { emitEvent, expect, openGuide, test } from "./fixtures";
import type { Backend } from "./backend";

const QUIT = "guidesya://quit-requested";

/** Holds `cmd` (before the backend sees it) until `release` is called. */
function hold(backend: Backend, cmd: string) {
  const invoke = backend.invoke.bind(backend);
  let release!: () => void;
  const released = new Promise<void>((resolve) => (release = resolve));
  let held = false;
  backend.invoke = async (c, args) => {
    if (c === cmd) {
      held = true;
      await released;
    }
    return invoke(c, args);
  };
  return { release, held: () => held };
}

const quitCalls = (backend: Backend) =>
  backend.calls.map((c) => c.cmd).filter((c) => c.endsWith("_recording") || c.startsWith("quit"));

test("a rejected quit shows an error and the editor stays usable", async ({ page, backend }) => {
  await openGuide(page, "Rotate an API key");
  const invoke = backend.invoke.bind(backend);
  let failQuit = true;
  backend.invoke = async (cmd, args) => {
    if (cmd === "quit_app" && failQuit) {
      failQuit = false;
      backend.calls.push({ cmd, args });
      return { error: { kind: "internal", message: "quit failed" }, events: [] };
    }
    return invoke(cmd, args);
  };

  await emitEvent(page, QUIT, null);
  await expect(page.getByRole("alert")).toHaveText(/Could not quit: quit failed/);
  const title = page.getByRole("textbox", { name: "Guide title" });
  await expect(title).toHaveValue("Rotate an API key");
  await title.fill("Still editing");
  await expect(title).toHaveValue("Still editing");

  await emitEvent(page, QUIT, null);
  await expect.poll(() => backend.calls.filter((c) => c.cmd === "quit_app")).toHaveLength(2);
});

test("a quit while a recording command runs waits for it, then quits once", async ({ page, backend }) => {
  for (const p of Object.keys(backend.permissions)) backend.permissions[p] = "granted";
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  const start = hold(backend, "start_recording");
  await page.getByRole("button", { name: "Record" }).click();
  await expect.poll(start.held).toBe(true);

  await emitEvent(page, QUIT, null);
  await emitEvent(page, QUIT, null);
  await expect(page.getByRole("alert")).toHaveText(/Quitting once the recording command finishes/);
  expect(quitCalls(backend)).toEqual(["quit_ack", "quit_ack"]);

  start.release();
  await expect.poll(() => quitCalls(backend)).toEqual([
    "quit_ack",
    "quit_ack",
    "start_recording",
    "stop_recording",
    "quit_app",
  ]);
  expect(backend.recordings).toHaveLength(1);
});
