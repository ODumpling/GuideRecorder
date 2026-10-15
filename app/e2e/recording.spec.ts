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
  await expect(page.getByText(/Recording the whole display/)).toBeHidden();

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
  await expect(page.getByText("Recording the whole display; window and region capture are not available yet.")).toBeVisible();

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
