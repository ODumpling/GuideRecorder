import { expect, test } from "./fixtures";

test("signs in to the web app and syncs the library once turned on", async ({ page, backend }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+,");
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("button", { name: "Sync" }).click();

  await dialog.getByRole("button", { name: "Sign in…" }).click();
  const signIn = page.getByRole("dialog", { name: "Sign in to sync" });
  await expect(signIn.getByText("WDJBMJHT", { exact: true })).toBeVisible();
  await expect(signIn.getByText("https://app.guiderecorder.dev:3000/device?user_code=WDJBMJHT")).toBeVisible();
  backend.sync.approved = true;
  await expect(page.getByRole("dialog", { name: "Signed in" })).toBeVisible();
  await page.getByRole("button", { name: "Done" }).click();
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

test("signs in from the sidebar", async ({ page, backend }) => {
  await page.getByRole("button", { name: "Sign in to sync" }).click();
  const dialog = page.getByRole("dialog", { name: "Sign in to sync" });
  await expect(dialog.getByText("WDJBMJHT", { exact: true })).toBeVisible();
  // The web app from app/.env.development; nothing to type.
  expect(backend.sync.server).toBe("https://app.guiderecorder.dev:3000");
  backend.sync.approved = true;

  const signedIn = page.getByRole("dialog", { name: "Signed in" });
  await expect(signedIn.getByRole("combobox", { name: "Workspace" })).toHaveValue("ws1");
  await signedIn.getByRole("switch", { name: "Sync the whole library" }).click();
  await expect.poll(() => backend.sync.pushes.length).toBeGreaterThan(0);
  await signedIn.getByRole("button", { name: "Done" }).click();
  await expect(signedIn).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^(Synced|Syncing)/ })).toBeVisible();
});

test("closing the sign-in dialog cancels signing in; opening it again starts over", async ({ page, backend }) => {
  void backend;
  await page.getByRole("button", { name: "Sign in to sync" }).click();
  const dialog = page.getByRole("dialog", { name: "Sign in to sync" });
  await expect(dialog.getByText("WDJBMJHT", { exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);

  await page.getByRole("button", { name: "Sign in to sync" }).click();
  await expect(dialog.getByText("WDJBMJHT", { exact: true })).toBeVisible();

  await dialog.getByRole("button", { name: "Cancel" }).click();
  await dialog.getByRole("button", { name: "Open browser" }).click();
  await expect(dialog.getByText("WDJBMJHT", { exact: true })).toBeVisible();
});
