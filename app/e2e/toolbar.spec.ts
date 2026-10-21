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

  await emitEvent(page, "recording://snapshot", { id: "0-1", ts: 0, width: 16, height: 10, point: null, title: "Click **Save**", elapsedMs: 0 });
  await emitEvent(page, "recording://snapshot", { id: "0-2", ts: 1, width: 16, height: 10, point: null, title: "Press **Return**", elapsedMs: 1 });
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

test("toolbar says when capture stopped and holds manual steps", async ({ page, backend }) => {
  await startRecording(backend);
  const toolbar = await openToolbar(page);
  await expect(toolbar.getByRole("status")).toHaveText("Recording");

  await emitEvent(page, "recording://status", backend.loseCapture("the display was disconnected"));
  const label = toolbar.getByRole("status");
  await expect(label).toHaveText("Capture stopped: the display was disconnected");
  await expect(label).toHaveAttribute(
    "title",
    "Capture stopped: the display was disconnected. Stop to save what was recorded.",
  );
  await expect(toolbar.getByRole("button", { name: "Add manual step" })).toBeDisabled();
  await expect(toolbar.getByRole("button", { name: "Stop recording" })).toBeEnabled();

  // A resume that captures again clears it.
  await emitEvent(page, "recording://status", backend.loseCapture(null));
  await expect(label).toHaveText("Recording");
  await expect(toolbar.getByRole("button", { name: "Add manual step" })).toBeEnabled();

  await emitEvent(page, "recording://status", backend.loseCapture("the display was disconnected"));
  await toolbar.getByRole("button", { name: "Stop recording" }).click();
  await expect.poll(() => backend.recordings.length).toBe(1);
  expect(backend.manualSteps).toBe(0);
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

test("toolbar microphone narrates, says what is missing, and is remembered", async ({ page, backend }) => {
  await startRecording(backend);
  const toolbar = await openToolbar(page);
  const turnOn = toolbar.getByRole("button", { name: "Turn the microphone on" });
  await expect(turnOn).toHaveAttribute("aria-pressed", "false");

  // Without its permissions it stays off and says why; it never prompts.
  await turnOn.click();
  await expect(turnOn).toHaveAttribute(
    "title",
    "Couldn't turn the microphone on: Allow GuideRecorder to use the microphone in Settings → Permissions.",
  );
  expect(backend.narrationOn).toBe(false);
  expect(backend.calls.some((c) => c.cmd === "request_permission")).toBe(false);

  // Allowed: on for this recording, and stored for the next one.
  backend.narrationPermissions.microphone = "granted";
  backend.narrationPermissions.speech_recognition = "granted";
  await turnOn.click();
  const turnOff = toolbar.getByRole("button", { name: "Turn the microphone off" });
  await expect(turnOff).toHaveAttribute("aria-pressed", "true");
  expect(backend.narrationOn).toBe(true);
  expect(backend.settings.recording.narration.enabled).toBe(true);

  // Pausing keeps it on, paused with the recording.
  await toolbar.getByRole("button", { name: "Pause recording" }).click();
  await expect(turnOff).toHaveAttribute("title", "Microphone on, and paused with the recording");
  await toolbar.getByRole("button", { name: "Resume recording" }).click();
  await expect(toolbar.getByRole("status")).toHaveText("Recording");

  await toolbar.getByRole("button", { name: "Stop recording" }).click();
  await expect.poll(() => backend.recordings.length).toBe(1);
  expect(backend.recordings[0].narratedSteps).toBe(1);

  // The next recording starts with it on.
  await startRecording(backend);
  expect(backend.narrationOn).toBe(true);
});
