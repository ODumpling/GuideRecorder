import type { Page } from "@playwright/test";
import { emitEvent, expect, test } from "./fixtures";

/** Opens Settings once the app is up (its shortcut listener too). */
async function openSettings(page: Page) {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+,");
  return page.getByRole("dialog", { name: "Settings" });
}

test.describe("with an update available", () => {
  test.use({ backendOptions: { updateVersion: "0.2.0" } });

  test("checks from Settings, installs, then restarts", async ({ page, backend }) => {
    const dialog = await openSettings(page);
    await expect(dialog.getByText(/You have version 0\.1\.0/)).toBeVisible();
    await dialog.getByRole("button", { name: "Check now" }).click();
    await expect(dialog.getByText("Version 0.2.0 is available.")).toBeVisible();
    await page.keyboard.press("Escape");

    const banner = page.getByRole("status", { name: "Update" });
    await expect(banner).toContainText("GuideRecorder 0.2.0 is available.");
    await banner.getByRole("button", { name: "Install update" }).click();
    await expect(banner).toContainText("GuideRecorder 0.2.0 is installed");
    await banner.getByRole("button", { name: "Restart now" }).click();
    await expect.poll(() => backend.restartedToUpdate).toBe(true);
  });
});

test("Later hides a background check's update until a check is asked for", async ({ page, backend }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  backend.updateStatus = { state: "available", version: "0.3.0", notes: null };
  await emitEvent(page, "updater://status", backend.updateStatus);
  const banner = page.getByRole("status", { name: "Update" });
  await expect(banner).toContainText("GuideRecorder 0.3.0 is available.");
  await banner.getByRole("button", { name: "Later" }).click();
  await expect(banner).toBeHidden();
  // The same version found again in the background stays hidden.
  await emitEvent(page, "updater://status", backend.updateStatus);
  await expect(banner).toBeHidden();
  // A background check's result is not shown; a failed one neither.
  await emitEvent(page, "updater://status", { state: "upToDate", manual: false });
  await emitEvent(page, "updater://status", { state: "failed", message: "offline", manual: false });
  await expect(page.getByRole("alert")).toBeHidden();
  await expect(banner).toBeHidden();
});

test("a check asked for says when there is nothing new, and can be turned off", async ({ page, backend }) => {
  const dialog = await openSettings(page);
  await dialog.getByRole("switch", { name: "Check for updates automatically" }).click();
  await expect.poll(() => backend.settings.general.checkForUpdates).toBe(false);
  await dialog.getByRole("button", { name: "Check now" }).click();
  await expect(dialog.getByText("Up to date.")).toBeVisible();
  await page.keyboard.press("Escape");
  const banner = page.getByRole("status", { name: "Update" });
  await expect(banner).toContainText("GuideRecorder is up to date.");
  await banner.getByRole("button", { name: "Dismiss" }).click();
  await expect(banner).toBeHidden();
});
