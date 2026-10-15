import type { Page } from "@playwright/test";
import type { Backend } from "./backend";
import { emitEvent, expect, openRegionOverlay, test } from "./fixtures";

const grantAll = (backend: Backend) => {
  for (const p of Object.keys(backend.permissions)) backend.permissions[p] = "granted";
};

/** The app has loaded (the library lists the fixture guides). */
async function ready(page: Page) {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
}

const startCalls = (backend: Backend) => backend.calls.filter((c) => c.cmd === "start_recording");

test.describe("capture area: window", () => {
  test.use({ backendOptions: { captureArea: "window" } });

  test("picks a window by filter and keyboard, and records it", async ({ page, backend }) => {
    grantAll(backend);
    await ready(page);
    await page.getByRole("button", { name: "Record" }).click();
    const dialog = page.getByRole("dialog", { name: "Choose a window to record" });
    const options = dialog.getByRole("option");
    await expect(options).toHaveCount(3);
    await expect(options.nth(0)).toContainText("Safari");
    await expect(options.nth(0)).toContainText("Apple Support");
    await expect(options.nth(0)).toContainText("1280 × 800");
    await expect(options.nth(2)).toContainText("Untitled window");
    expect(startCalls(backend)).toHaveLength(0);

    const filter = dialog.getByRole("combobox", { name: "Filter windows" });
    await expect(filter).toBeFocused();
    await filter.fill("apple");
    // Every bundle id has it.
    await expect(options).toHaveCount(3);
    await filter.fill("com.apple.s");
    await expect(options).toHaveCount(1);
    await filter.fill("");
    await expect(options.nth(0)).toHaveAttribute("aria-selected", "true");
    await filter.press("ArrowDown");
    await expect(options.nth(1)).toHaveAttribute("aria-selected", "true");
    await filter.press("ArrowDown");
    await filter.press("ArrowDown");
    await expect(options.nth(2)).toHaveAttribute("aria-selected", "true");
    await filter.press("ArrowUp");
    await filter.press("Enter");

    await expect(dialog).toBeHidden();
    await expect(page.getByRole("group", { name: "Recording" })).toBeVisible();
    expect(backend.recordingTargets).toEqual([{ kind: "window", windowId: 102 }]);
    await expect(page.getByText(/Recording all displays/)).toBeHidden();
  });

  test("a click on a window records it", async ({ page, backend }) => {
    grantAll(backend);
    await ready(page);
    await page.getByRole("button", { name: "Record" }).click();
    const dialog = page.getByRole("dialog", { name: "Choose a window to record" });
    await dialog.getByRole("option", { name: /Finder/ }).click();
    await expect(page.getByRole("group", { name: "Recording" })).toBeVisible();
    expect(backend.recordingTargets).toEqual([{ kind: "window", windowId: 103 }]);
  });

  test("cancel starts nothing", async ({ page, backend }) => {
    grantAll(backend);
    await ready(page);
    await page.getByRole("button", { name: "Record" }).click();
    const dialog = page.getByRole("dialog", { name: "Choose a window to record" });
    await expect(dialog.getByRole("option")).toHaveCount(3);
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();

    // Esc closes it too.
    await page.getByRole("button", { name: "Record" }).click();
    await expect(dialog.getByRole("option")).toHaveCount(3);
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();

    expect(startCalls(backend)).toHaveLength(0);
    expect(backend.recording).toEqual({ state: "idle" });
    await expect(page.getByRole("button", { name: "Record" })).toBeEnabled();
  });

  test("a window that closed is explained and the list reloads", async ({ page, backend }) => {
    grantAll(backend);
    await ready(page);
    await page.getByRole("button", { name: "Record" }).click();
    const dialog = page.getByRole("dialog", { name: "Choose a window to record" });
    await expect(dialog.getByRole("option")).toHaveCount(3);
    backend.captureWindows.splice(0, 1);
    await dialog.getByRole("option", { name: /Safari/ }).click();

    await expect(dialog.getByRole("alert")).toHaveText(/That window is no longer open\./);
    await expect(dialog.getByRole("option")).toHaveCount(2);
    await expect(dialog.getByRole("option", { name: /Safari/ })).toHaveCount(0);
    expect(backend.recording).toEqual({ state: "idle" });
    // Not in the main window's banner as well.
    await expect(page.getByText(/^Recording: /)).toBeHidden();

    // The filter has the focus back, and the keys work on the new list.
    const filter = dialog.getByRole("combobox", { name: "Filter windows" });
    await expect(filter).toBeFocused();
    const options = dialog.getByRole("option");
    await expect(options.nth(0)).toHaveAttribute("aria-selected", "true");
    await page.keyboard.press("ArrowDown");
    await expect(options.nth(1)).toHaveAttribute("aria-selected", "true");
    await page.keyboard.press("ArrowUp");
    await expect(options.nth(0)).toHaveAttribute("aria-selected", "true");
    await page.keyboard.press("Enter");
    await expect(dialog).toBeHidden();
    expect(backend.recordingTargets).toEqual([{ kind: "window", windowId: 102 }]);
  });

  test("no windows: records the whole display instead", async ({ page, backend }) => {
    grantAll(backend);
    backend.captureWindows = [];
    await ready(page);
    await page.getByRole("button", { name: "Record" }).click();
    const dialog = page.getByRole("dialog", { name: "Choose a window to record" });
    await expect(dialog.getByText("No windows available.")).toBeVisible();
    await dialog.getByRole("button", { name: "Record whole display instead" }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole("group", { name: "Recording" })).toBeVisible();
    expect(backend.recordingTargets).toEqual([{ kind: "display" }]);
  });

  test("the tray or hotkey asks for a window too", async ({ page, backend }) => {
    grantAll(backend);
    await ready(page);
    await emitEvent(page, "recording://pick-area", null);
    const dialog = page.getByRole("dialog", { name: "Choose a window to record" });
    await expect(dialog.getByRole("option")).toHaveCount(3);
    await dialog.getByRole("option", { name: /Safari/ }).click();
    await expect(page.getByRole("group", { name: "Recording" })).toBeVisible();
    expect(backend.recordingTargets).toEqual([{ kind: "window", windowId: 101 }]);

    // Ignored while recording.
    await emitEvent(page, "recording://pick-area", null);
    await expect(dialog).toBeHidden();
    expect(backend.calls.filter((c) => c.cmd === "list_capture_windows")).toHaveLength(1);
  });
});

test.describe("capture area: region", () => {
  test.use({ backendOptions: { captureArea: "region" } });

  test("records the region drawn", async ({ page, backend }) => {
    grantAll(backend);
    const rect = { x: 120, y: 80, width: 640, height: 360 };
    backend.regionPick = { displayId: 2, rect };
    await ready(page);
    await page.getByRole("button", { name: "Record" }).click();
    await expect(page.getByRole("group", { name: "Recording" })).toBeVisible();
    expect(backend.calls.filter((c) => c.cmd === "pick_region")).toHaveLength(1);
    expect(backend.recordingTargets).toEqual([{ kind: "region", displayId: 2, rect }]);
  });

  test("a cancelled region starts nothing", async ({ page, backend }) => {
    grantAll(backend);
    backend.regionPick = null;
    await ready(page);
    await page.getByRole("button", { name: "Record" }).click();
    await expect.poll(() => backend.calls.filter((c) => c.cmd === "pick_region").length).toBe(1);
    await expect(page.getByRole("button", { name: "Record" })).toBeEnabled();
    expect(startCalls(backend)).toHaveLength(0);
    await expect(page.getByRole("alert")).toBeHidden();
  });

  test("the tray or hotkey draws a region too", async ({ page, backend }) => {
    grantAll(backend);
    backend.regionPick = { displayId: null, rect: { x: 0, y: 0, width: 100, height: 100 } };
    await ready(page);
    await emitEvent(page, "recording://pick-area", null);
    await expect(page.getByRole("group", { name: "Recording" })).toBeVisible();
    expect(backend.recordingTargets).toEqual([{ kind: "region", displayId: null, rect: { x: 0, y: 0, width: 100, height: 100 } }]);
  });
});

test.describe("region overlay", () => {
  test("draws a region and confirms on release", async ({ page, backend }) => {
    const overlay = await openRegionOverlay(page, 7);
    await expect(overlay.getByRole("status")).toHaveText(/Drag to select the area to record/);
    // Dragged up and to the left.
    await page.mouse.move(400, 300);
    await page.mouse.down();
    await page.mouse.move(250, 200, { steps: 4 });
    await page.mouse.move(100, 100, { steps: 4 });
    await expect(page.getByLabel("Region size")).toHaveText("300 × 200");
    await expect(page.getByLabel("Selected region")).toBeVisible();
    expect(backend.regionFinishes).toEqual([]);
    await page.mouse.up();
    await expect.poll(() => backend.regionFinishes).toEqual([{ rect: { x: 100, y: 100, width: 300, height: 200 } }]);
    // The pick id from the overlay's URL goes back with the answer.
    expect(backend.regionFinishPicks).toEqual([7]);
  });

  test("Return confirms the region being drawn", async ({ page, backend }) => {
    await openRegionOverlay(page);
    await page.mouse.move(50, 60);
    await page.mouse.down();
    await page.mouse.move(250, 160, { steps: 4 });
    await page.keyboard.press("Enter");
    await expect.poll(() => backend.regionFinishes).toEqual([{ rect: { x: 50, y: 60, width: 200, height: 100 } }]);
    // Releasing afterwards doesn't answer twice.
    await page.mouse.up();
    await page.keyboard.press("Enter");
    expect(backend.regionFinishes).toHaveLength(1);
  });

  test("a region under 40 × 40 isn't picked", async ({ page, backend }) => {
    const overlay = await openRegionOverlay(page);
    await page.mouse.move(100, 100);
    await page.mouse.down();
    await page.mouse.move(130, 300, { steps: 4 });
    await page.keyboard.press("Enter");
    await page.mouse.up();
    await expect(overlay.getByRole("status")).toHaveText(/Too small: drag at least 40 × 40/);
    await expect(page.getByLabel("Selected region")).toBeHidden();
    expect(backend.regionFinishes).toEqual([]);
  });

  test("Esc cancels", async ({ page, backend }) => {
    await openRegionOverlay(page);
    await page.mouse.move(100, 100);
    await page.mouse.down();
    await page.mouse.move(300, 300, { steps: 4 });
    await page.keyboard.press("Escape");
    await expect.poll(() => backend.regionFinishes).toEqual([null]);
    await page.mouse.up();
    expect(backend.regionFinishes).toEqual([null]);
    // No pick id in the URL: none is sent.
    expect(backend.calls.find((c) => c.cmd === "finish_region_pick")?.args).toEqual({ result: null });
    expect(backend.regionFinishPicks).toEqual([undefined]);
  });
});
