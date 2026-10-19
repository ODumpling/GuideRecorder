import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";

async function openPermissions(page: Page) {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+,");
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("button", { name: "Permissions" }).click();
  return dialog;
}

test("picks up a switch flipped in System Settings without a click", async ({ page, backend }) => {
  const dialog = await openPermissions(page);
  const accessibility = dialog.getByText("Accessibility", { exact: true }).locator("../..");
  await expect(accessibility.getByText("Not allowed")).toBeVisible();
  // The user turns it on in System Settings; the list follows on its own.
  backend.permissions.accessibility = "granted";
  await expect(accessibility.getByText("Allowed", { exact: true })).toBeVisible({ timeout: 3_000 });
  backend.permissions.input_monitoring = "granted";
  await expect(dialog.getByText("All set: recording is available.")).toBeVisible({ timeout: 3_000 });
});

test("Turn on asks macOS once, then opens the right System Settings pane", async ({ page, backend }) => {
  backend.requestGrants = false;
  const dialog = await openPermissions(page);
  const row = dialog.getByText("Accessibility", { exact: true }).locator("../..");
  await row.getByRole("button", { name: "Turn on…" }).click();
  expect(backend.calls.filter((c) => c.cmd === "request_permission")).toHaveLength(1);
  // Still off: the next click goes straight to System Settings, with directions.
  await expect(row.getByText(/Privacy & Security → Accessibility, turn on GuideRecorder/)).toBeVisible();
  await row.getByRole("button", { name: "Open System Settings" }).click();
  await expect.poll(() => backend.openedPermissionSettings).toEqual(["accessibility"]);
  expect(backend.calls.filter((c) => c.cmd === "request_permission")).toHaveLength(1);

  // Remembered across a reload.
  await page.reload();
  const again = await openPermissions(page);
  await expect(again.getByText("Accessibility", { exact: true }).locator("../..").getByRole("button", { name: "Open System Settings" })).toBeVisible();
});

test("a Screen Recording grant waiting for a restart offers one", async ({ page, backend }) => {
  backend.permissions.screen_recording = "denied";
  backend.restartNeeded = ["screen_recording"];
  const dialog = await openPermissions(page);
  await expect(dialog.getByText("Allowed after a restart")).toBeVisible();
  await dialog.getByRole("button", { name: "Restart GuideRecorder" }).click();
  await expect.poll(() => backend.restarted).toBe(true);
});
