import { expect, test } from "./fixtures";

test("signs in to the web app and syncs the library once turned on", async ({ page, backend }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+,");
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("button", { name: "Sync" }).click();

  await dialog.getByLabel("Web app address").fill("https://web.example");
  await dialog.getByRole("button", { name: "Sign in" }).click();
  await expect(dialog.getByText("WDJBMJHT")).toBeVisible();
  backend.sync.approved = true;
  await expect(dialog.getByText("Signed in")).toBeVisible();
  await expect(dialog.getByRole("combobox", { name: "Workspace" })).toHaveValue("ws1");

  // Local-first: nothing syncs until it's turned on.
  expect(backend.sync.pushes).toEqual([]);
  await expect(page.getByRole("img", { name: /Synced with the web/ })).toHaveCount(0);

  await dialog.getByRole("switch", { name: "Sync the whole library" }).click();
  await expect.poll(() => backend.sync.pushes.map((p) => p.guide.title)).toContain("Rotate an API key");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("img", { name: /Synced with the web/ }).first()).toBeVisible();
});
