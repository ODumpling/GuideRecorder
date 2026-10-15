import type { Backend } from "./backend";
import { emitEvent, expect, openToolbar, test } from "./fixtures";

/** Starts a recording on the backend, as the main window or a hotkey would. */
async function startRecording(backend: Backend) {
  for (const p of Object.keys(backend.permissions)) backend.permissions[p] = "granted";
  const reply = await backend.invoke("start_recording", { title: null });
  if ("error" in reply) throw new Error(reply.error.message);
}

test("toolbar pauses and resumes, with the clock", async ({ page, backend }) => {
  await startRecording(backend);
  const toolbar = await openToolbar(page);
  await expect(toolbar.getByRole("status")).toHaveText("Recording");
  await expect(toolbar.getByLabel("Recording time")).toHaveText(/^0:0\d$/);
  await expect(toolbar.getByLabel("Screenshots")).toHaveText("0");

  await toolbar.getByRole("button", { name: "Pause recording" }).click();
  await expect(toolbar.getByRole("status")).toHaveText("Paused");
  await expect.poll(() => backend.recording.state).toBe("paused");
  await expect(toolbar.getByRole("button", { name: "Add manual step" })).toBeDisabled();

  await toolbar.getByRole("button", { name: "Resume recording" }).click();
  await expect(toolbar.getByRole("status")).toHaveText("Recording");
  await expect.poll(() => backend.recording.state).toBe("recording");
  await expect(toolbar.getByRole("button", { name: "Add manual step" })).toBeEnabled();
});

test("toolbar counts screenshots and adds a manual step", async ({ page, backend }) => {
  await startRecording(backend);
  const toolbar = await openToolbar(page);
  const status = backend.recording;
  if (status.state !== "recording") throw new Error("not recording");

  await emitEvent(page, "recording://snapshot", { id: "0-1", ts: 0, width: 16, height: 10, point: null });
  await emitEvent(page, "recording://snapshot", { id: "0-2", ts: 1, width: 16, height: 10, point: null });
  await expect(toolbar.getByLabel("Screenshots")).toHaveText("2");

  await toolbar.getByRole("button", { name: "Add manual step" }).click();
  await expect(page.getByRole("alert")).toHaveText("Step added");
  expect(backend.manualSteps).toBe(1);
  await expect(page.getByRole("alert")).toHaveText("");

  // A rejection shows quietly; the toolbar carries on.
  backend.manualStepError = { kind: "io", message: "the screenshot failed" };
  await toolbar.getByRole("button", { name: "Add manual step" }).click();
  await expect(page.getByRole("alert")).toHaveText("Couldn't add a step: the screenshot failed");
  expect(backend.manualSteps).toBe(1);
  await expect(toolbar.getByRole("status")).toHaveText("Recording");

  // A new run starts the count again.
  await emitEvent(page, "recording://status", { ...status, startedAt: new Date().toISOString(), seq: 100 });
  await expect(toolbar.getByLabel("Screenshots")).toHaveText("0");
});

test("toolbar names the excluded app in front and holds manual steps", async ({ page, backend }) => {
  await startRecording(backend);
  const toolbar = await openToolbar(page);
  await emitEvent(page, "recording://status", backend.frontmost("com.apple.keychainaccess"));
  await expect(toolbar.getByRole("status")).toHaveText("Not capturing: Keychain Access is in front");
  await expect(toolbar.getByRole("button", { name: "Add manual step" })).toBeDisabled();
  await expect(toolbar.getByRole("button", { name: "Pause recording" })).toBeEnabled();

  await emitEvent(page, "recording://status", backend.frontmost(null));
  await expect(toolbar.getByRole("status")).toHaveText("Recording");
  await expect(toolbar.getByRole("button", { name: "Add manual step" })).toBeEnabled();
});

test("toolbar stops the recording and leaves the guide to the main window", async ({ page, backend }) => {
  await startRecording(backend);
  const toolbar = await openToolbar(page);
  await toolbar.getByRole("button", { name: "Stop recording" }).click();
  await expect.poll(() => backend.recordings.length).toBe(1);
  expect(backend.recording).toEqual({ state: "idle" });
  await expect(toolbar.getByRole("status")).toHaveText("Not recording");
  await expect(toolbar.getByRole("button", { name: "Stop recording" })).toBeDisabled();
  // The toolbar opens nothing itself.
  expect(backend.calls.filter((c) => c.cmd === "open_guide")).toHaveLength(0);
});
