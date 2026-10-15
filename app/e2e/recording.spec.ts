import { CALENDAR, emitEvent, expect, outlineItems, test } from "./fixtures";

const grantAll = (permissions: Record<string, string>) => {
  for (const p of Object.keys(permissions)) permissions[p] = "granted";
};

test("records a guide: start, pause, resume, stop, then edit it", async ({ page, backend }) => {
  grantAll(backend.permissions);
  backend.recordingSkipped = 2;
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();

  await page.getByRole("button", { name: "Record" }).click();
  const controls = page.getByRole("group", { name: "Recording" });
  await expect(controls.getByText("Recording", { exact: true })).toBeVisible();
  await expect(controls.getByLabel("Recording time")).toHaveText(/^0:0\d$/);
  expect(backend.calls.find((c) => c.cmd === "start_recording")?.args).toEqual({ title: null });
  await expect(page.getByText(/Recording all displays/)).toBeHidden();

  await controls.getByRole("button", { name: "Pause recording" }).click();
  await expect(controls.getByText("Paused", { exact: true })).toBeVisible();
  await expect.poll(() => backend.recording.state).toBe("paused");
  await controls.getByRole("button", { name: "Resume recording" }).click();
  await expect(controls.getByText("Recording", { exact: true })).toBeVisible();
  await expect.poll(() => backend.recording.state).toBe("recording");

  await controls.getByRole("button", { name: "Stop recording" }).click();
  // The new guide opens in the editor, and the library lists it.
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue("Recording 2026-10-15 10:01");
  const id = backend.recordings[0].guideId;
  await expect(outlineItems(page)).toHaveCount(backend.guide(id).items.length);
  await expect(page.getByRole("alert")).toHaveText(/2 steps had no screenshot and were left out\./);
  await expect(page.getByRole("button", { name: "Record" })).toBeEnabled();
  expect(backend.recording).toEqual({ state: "idle" });
  // Opening the guide happened once, though both the result and the event announce it.
  expect(backend.calls.filter((c) => c.cmd === "open_guide")).toHaveLength(1);
  expect(backend.calls.filter((c) => c.cmd === "list_guides").length).toBeGreaterThan(1);
});

test("a missing permission is explained, with a way to Settings", async ({ page, backend }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.getByRole("button", { name: "Record" }).click();
  const alert = page.getByRole("alert");
  await expect(alert).toHaveText(/recording needs Accessibility, Input Monitoring permission/);
  expect(backend.recording).toEqual({ state: "idle" });
  await expect(page.getByRole("button", { name: "Record" })).toBeEnabled();

  await alert.getByRole("button", { name: "Open Permissions" }).click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await expect(dialog.getByRole("button", { name: "Permissions" })).toHaveAttribute("aria-current", "page");
  await expect(dialog.getByText("Not allowed")).toBeVisible();
  await expect(alert).toBeHidden();
});

test("follows the recorder's events and shows the capture fallback", async ({ page, backend }) => {
  grantAll(backend.permissions);
  backend.captureAreaFallback = true;
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.getByRole("button", { name: "Record" }).click();
  await expect(page.getByText("Recording all displays; window and region capture are not available yet.")).toBeVisible();

  // A stop from elsewhere (a hotkey, later): saving, then the saved guide opens.
  const status = backend.recording;
  if (status.state !== "recording") throw new Error("not recording");
  await emitEvent(page, "recording://status", { state: "saving", startedAt: status.startedAt, title: null });
  await expect(page.getByRole("status")).toHaveText("Saving recording…");
  await emitEvent(page, "recording://status", { state: "idle" });
  await emitEvent(page, "recording://saved", { guideId: CALENDAR, steps: 3, skipped: 0 });
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue("Share a calendar event");
  await expect(page.getByRole("button", { name: "Record" })).toBeEnabled();
});

test("shows the screenshots taken so far while recording", async ({ page, backend }) => {
  grantAll(backend.permissions);
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.getByRole("button", { name: "Record" }).click();
  const controls = page.getByRole("group", { name: "Recording" });
  await expect(controls).toBeVisible();
  const strip = page.getByRole("region", { name: "Screenshots so far" });
  await expect(strip).toBeHidden();

  // Two screenshots, announced as the Rust side would (with the next reply).
  backend.takeSnapshot("0-5000000", { x: 400, y: 250 });
  backend.takeSnapshot("0-9000000");
  await controls.getByRole("button", { name: "Pause recording" }).click();
  await expect(strip).toContainText("2 screenshots so far");
  await expect(strip.getByRole("img", { name: "Screenshot 1" })).toBeVisible();
  await expect(strip.getByRole("img", { name: "Screenshot 2" })).toBeVisible();
  // The same screenshot again: not listed twice.
  backend.takeSnapshot("0-9000000");
  await controls.getByRole("button", { name: "Resume recording" }).click();
  await expect(strip).toContainText("2 screenshots so far");
  // Only the two images were fetched (StrictMode's second mount repeats a fetch in dev).
  const fetched = backend.calls.filter((c) => c.cmd === "recording_snapshot").map((c) => (c.args as { id: string }).id);
  expect([...new Set(fetched)]).toEqual(["0-5000000", "0-9000000"]);

  // Gone once the recording is over.
  await controls.getByRole("button", { name: "Stop recording" }).click();
  await expect(page.getByRole("textbox", { name: "Guide title" })).toBeVisible();
  await expect(strip).toBeHidden();
});

test("cancelling asks first and discards the recording", async ({ page, backend }) => {
  grantAll(backend.permissions);
  await page.getByRole("button", { name: "Record" }).click();
  const controls = page.getByRole("group", { name: "Recording" });
  await controls.getByRole("button", { name: "Cancel recording" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Keep recording" }).click();
  expect(backend.recording.state).toBe("recording");

  await controls.getByRole("button", { name: "Cancel recording" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Discard recording" }).click();
  await expect(page.getByRole("button", { name: "Record" })).toBeVisible();
  expect(backend.recording).toEqual({ state: "idle" });
  expect(backend.recordings).toHaveLength(0);
});

test("quitting while recording saves the recording first", async ({ page, backend }) => {
  grantAll(backend.permissions);
  await page.getByRole("button", { name: "Record" }).click();
  await expect(page.getByRole("group", { name: "Recording" })).toBeVisible();

  await emitEvent(page, "guidesya://quit-requested", null);
  await expect.poll(() => backend.calls.map((c) => c.cmd).filter((c) => c.endsWith("_recording") || c.startsWith("quit"))).toEqual([
    "start_recording",
    "quit_ack",
    "stop_recording",
    "quit_app",
  ]);
  expect(backend.recordings).toHaveLength(1);
  // Not opened: the app is quitting.
  expect(backend.calls.some((c) => c.cmd === "open_guide")).toBe(false);
});

test("shows when an excluded app is in front", async ({ page, backend }) => {
  grantAll(backend.permissions);
  await page.getByRole("button", { name: "Record" }).click();
  const controls = page.getByRole("group", { name: "Recording" });
  const label = controls.getByRole("status");
  await expect(label).toHaveText("Recording");

  await emitEvent(page, "recording://status", backend.frontmost("com.1password.1password"));
  await expect(label).toHaveText("Not capturing: 1Password is in front");
  // Still a running recording: it can be paused or stopped as usual.
  await expect(controls.getByRole("button", { name: "Pause recording" })).toBeEnabled();
  // An app without a known name shows its bundle id.
  await emitEvent(page, "recording://status", backend.frontmost("com.example.vault"));
  await expect(label).toHaveText("Not capturing: com.example.vault is in front");

  // A pause by the user reads as a pause, whatever is in front.
  await controls.getByRole("button", { name: "Pause recording" }).click();
  await expect(label).toHaveText("Paused");
  await controls.getByRole("button", { name: "Resume recording" }).click();
  await expect(label).toHaveText("Not capturing: com.example.vault is in front");

  await emitEvent(page, "recording://status", backend.frontmost(null));
  await expect(label).toHaveText("Recording");

  // A sink event that arrives after a newer status (here a pause) is dropped.
  const stale = backend.frontmost("com.1password.1password");
  await controls.getByRole("button", { name: "Pause recording" }).click();
  await expect(label).toHaveText("Paused");
  await emitEvent(page, "recording://status", stale);
  await expect(controls.getByRole("button", { name: "Resume recording" })).toBeVisible();
  await expect(label).toHaveText("Paused");
});

test("the recording clock survives a reload, paused time included", async ({ page, backend }) => {
  grantAll(backend.permissions);
  await page.getByRole("button", { name: "Record" }).click();
  await expect(page.getByRole("group", { name: "Recording" })).toBeVisible();

  // Started 125 s ago, paused 5 s ago after 20 s of earlier pauses: 1:40 recorded.
  const now = Date.now();
  const status = backend.recording;
  if (status.state !== "recording") throw new Error("not recording");
  backend.setActive({
    ...status,
    state: "paused",
    startedAt: new Date(now - 125_000).toISOString(),
    pausedMs: 20_000,
    pausedAt: new Date(now - 5_000).toISOString(),
  });

  await page.reload();
  const controls = page.getByRole("group", { name: "Recording" });
  await expect(controls.getByRole("status")).toHaveText("Paused");
  await expect(controls.getByLabel("Recording time")).toHaveText("1:40");

  // Resuming counts this pause too, and the clock runs on from 1:40.
  await controls.getByRole("button", { name: "Resume recording" }).click();
  await expect(controls.getByRole("status")).toHaveText("Recording");
  const resumed = backend.recording;
  expect(resumed.state === "recording" && resumed.pausedMs).toBeGreaterThanOrEqual(25_000);
  await expect(controls.getByLabel("Recording time")).toHaveText(/^1:4\d$/);
  await page.reload();
  await expect(page.getByRole("group", { name: "Recording" }).getByLabel("Recording time")).toHaveText(/^1:4\d$/);
});

test("excluded apps default to the suggested list and can be restored", async ({ page, backend }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+,");
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("button", { name: "Recording" }).click();
  await expect(dialog.getByText("Keychain Access", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Restore defaults" })).toBeHidden();

  await dialog.getByRole("button", { name: "Remove com.apple.keychainaccess" }).click();
  await expect(dialog.getByText("Keychain Access", { exact: true })).toBeHidden();
  expect(backend.settings.recording.excludedApps).not.toContain("com.apple.keychainaccess");
  await dialog.getByRole("button", { name: "Restore defaults" }).click();
  await expect(dialog.getByText("Keychain Access", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Restore defaults" })).toBeHidden();
  await expect.poll(() => backend.settings.recording.excludedApps).toContain("com.apple.keychainaccess");
});
