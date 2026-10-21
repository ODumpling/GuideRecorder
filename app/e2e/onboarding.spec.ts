import { expect, test } from "./fixtures";

test.describe("on first launch", () => {
  test.use({ backendOptions: { firstRun: true } });

  test("walks through permissions, library and API key, then stays done", async ({ page, backend }) => {
    const setup = page.getByRole("dialog", { name: "Set up GuideRecorder" });
    await expect(setup).toBeVisible();

    // 1. Permissions: recording is off until all are allowed; editing isn't blocked.
    await expect(setup.getByText("Recording is off until every permission is allowed.")).toBeVisible();
    // Narration's own, optional permissions are asked for apart.
    await expect(setup.getByText("Narration (optional)")).toBeVisible();
    await expect(setup.getByText("Microphone", { exact: true })).toBeVisible();
    await setup.getByRole("button", { name: "Turn on…" }).first().click();
    await setup.getByRole("button", { name: "Turn on…" }).first().click();
    await expect(setup.getByText("All set: recording is available.")).toBeVisible();
    expect(backend.calls.filter((c) => c.cmd === "request_permission")).toHaveLength(2);
    await setup.getByRole("button", { name: "Continue", exact: true }).click();

    // 2. Library: another folder.
    await expect(setup.getByText("/Users/me/Documents/GuideRecorder")).toBeVisible();
    backend.nextLibrary = "/Users/me/Team Guides";
    await setup.getByRole("button", { name: "Choose a folder…" }).click();
    await expect(setup.getByText("/Users/me/Team Guides")).toBeVisible();
    expect(backend.settings.general.library).toBe("/Users/me/Team Guides");
    await setup.getByRole("button", { name: "Continue" }).click();

    // 3. API key, optional.
    await setup.getByLabel("API key").fill("sk-ant-test");
    await setup.getByRole("button", { name: "Save and finish" }).click();
    await expect(setup).toBeHidden();
    expect(backend.apiKeyStored).toBe(true);
    await expect.poll(() => backend.settings.general.onboarded).toBe(true);

    // Done: a reload doesn't show it again.
    await page.reload();
    await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
    await expect(setup).toBeHidden();
  });

  test("can be skipped through without recording or a key", async ({ page, backend }) => {
    const setup = page.getByRole("dialog", { name: "Set up GuideRecorder" });
    await setup.getByRole("button", { name: "Continue without recording" }).click();
    await setup.getByRole("button", { name: "Continue" }).click();
    await setup.getByRole("button", { name: "Skip" }).click();
    await expect(setup).toBeHidden();
    await expect.poll(() => backend.settings.general.onboarded).toBe(true);
    expect(backend.apiKeyStored).toBe(false);
    expect(backend.calls.some((c) => c.cmd === "request_permission")).toBe(false);
  });
});

test.describe("with a missing library", () => {
  test.use({ backendOptions: { libraryError: "the library folder /Volumes/Team/Guides is missing (is its disk connected?)" } });

  test("opens on the library step and needs a working library to continue", async ({ page, backend }) => {
    const setup = page.getByRole("dialog", { name: "Set up GuideRecorder" });
    await expect(setup.getByRole("button", { name: /Library folder/ })).toHaveAttribute("aria-current", "step");
    await expect(setup.getByText(/is missing/)).toBeVisible();
    await expect(setup.getByRole("button", { name: "Continue" })).toBeDisabled();
    await setup.getByRole("button", { name: "Use ~/Documents/GuideRecorder" }).click();
    await expect(setup.getByText(/is missing/)).toBeHidden();
    await setup.getByRole("button", { name: "Continue" }).click();
    expect(backend.settings.general.library).toBeNull();
    await setup.getByRole("button", { name: "Skip" }).click();
    await expect(setup).toBeHidden();
  });
});

test("Settings reopens setup", async ({ page, backend }) => {
  void backend;
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Run setup again" }).click();
  await expect(page.getByRole("dialog", { name: "Settings" })).toBeHidden();
  await expect(page.getByRole("dialog", { name: "Set up GuideRecorder" })).toBeVisible();
});
