import { expect, openGuide, test } from "./fixtures";

test("changes settings, which the Export dialog then starts from", async ({ page, backend }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+,");
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTitle("/Users/me/Documents/GuideRecorder")).toHaveText("GuideRecorder");

  // Recording: a shortcut is recorded from a key press; a bad pause is refused.
  backend.hotkeyErrors.record = "This shortcut can't be used: another app or macOS may have it.";
  await dialog.getByRole("button", { name: "Recording" }).click();
  await expect(dialog.getByText("This shortcut can't be used")).toBeVisible();
  await dialog.getByRole("button", { name: "Pause shortcut" }).click();
  await page.keyboard.press("Control+Alt+KeyK");
  await expect(dialog.getByRole("button", { name: "Pause shortcut" })).toHaveText("⌃⌥K");
  await expect.poll(() => backend.settings.recording.hotkeys.pause).toBe("Ctrl+Alt+K");
  const pause = dialog.getByRole("spinbutton", { name: "Typing pause (ms)" });
  await pause.fill("20");
  await expect(pause).toHaveAttribute("aria-invalid", "true");
  // Committing an invalid value puts the stored one back.
  await pause.press("Enter");
  await expect(pause).toHaveValue("1500");
  expect(backend.settings.recording.typingIdleMs).toBe(1500);
  await pause.fill("2000");
  await pause.press("Enter");
  await expect.poll(() => backend.settings.recording.typingIdleMs).toBe(2000);

  // Excluded apps: a vendor prefix is refused with a reason; a full id is added.
  const appId = dialog.getByLabel("App bundle id");
  await appId.fill("com.apple");
  await expect(appId).toHaveAttribute("aria-invalid", "true");
  await expect(dialog.getByText(/com\.apple is too broad/)).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Add", exact: true })).toBeDisabled();
  await appId.press("Enter");
  expect(backend.settings.recording.excludedApps).not.toContain("com.apple");
  await appId.fill("com.apple.Safari");
  await expect(dialog.getByText(/too broad/)).toBeHidden();
  await dialog.getByRole("button", { name: "Add", exact: true }).click();
  await expect.poll(() => backend.settings.recording.excludedApps).toContain("com.apple.Safari");

  // AI: the key goes to the Keychain and is never shown back.
  await dialog.getByRole("button", { name: "AI", exact: true }).click();
  await dialog.getByLabel("API key").fill("sk-ant-test");
  await dialog.getByRole("button", { name: "Save key" }).click();
  await expect(dialog.getByText(/A key is stored in your Keychain/)).toBeVisible();
  await expect(dialog.getByLabel("API key")).toHaveValue("");
  expect(backend.apiKeyStored).toBe(true);

  // A local provider: no key, its own models, and text only.
  await dialog.getByLabel("Provider").selectOption("ollama");
  await expect.poll(() => backend.settings.ai.provider).toBe("ollama");
  await expect(dialog.getByText(/Nothing leaves your Mac/)).toBeVisible();
  await expect(dialog.getByLabel("API key")).toHaveCount(0);
  await expect(dialog.getByLabel("Base URL")).toHaveAttribute("placeholder", "http://localhost:11434/v1");
  await dialog.getByRole("textbox", { name: "Step model" }).fill("qwen3-vl:8b");
  await dialog.getByRole("textbox", { name: "Step model" }).press("Enter");
  await dialog.getByRole("switch", { name: "Send screenshots" }).click();
  await expect
    .poll(() => backend.settings.ai.ollama)
    .toMatchObject({ stepModel: "qwen3-vl:8b", images: false });
  // Anthropic's models are kept.
  expect(backend.settings.ai.stepModel).toBe("claude-haiku-5-5");
  await dialog.getByLabel("Provider").selectOption("anthropic");
  await expect(dialog.getByText(/A key is stored in your Keychain/)).toBeVisible();

  // Export defaults.
  await dialog.getByRole("button", { name: "Export", exact: true }).click();
  await dialog.getByLabel("Default format").selectOption("html");
  backend.dialog.folder = "/Users/me/Exports";
  await dialog.getByRole("button", { name: "Choose…" }).first().click();
  await expect.poll(() => backend.settings.export).toMatchObject({ format: "html", folder: "/Users/me/Exports" });

  // Permissions and MCP render.
  await dialog.getByRole("button", { name: "Permissions" }).click();
  await expect(dialog.getByText("Not allowed")).toBeVisible();
  await dialog.getByRole("button", { name: "MCP" }).click();
  await expect(dialog.getByText(/claude mcp add guiderecorder --/)).toBeVisible();
  await expect(dialog.locator("pre")).not.toContainText("--writable");
  await dialog.getByRole("switch", { name: "Allow MCP to edit guides" }).click();
  await expect.poll(() => backend.settings.mcp.writable).toBe(true);
  await expect(dialog.locator("pre")).toContainText("--library /Users/me/Documents/GuideRecorder --writable");

  await dialog.getByRole("button", { name: "Close settings" }).click();
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Export…" }).click();
  const exportDialog = page.getByRole("dialog", { name: "Export guide" });
  await expect(exportDialog.getByRole("radio", { name: /HTML/ })).toHaveAttribute("aria-checked", "true");
  await expect(exportDialog.getByText("Exports", { exact: true })).toBeVisible();
});

test("switches the theme, starting from Arctic", async ({ page, backend }) => {
  const html = page.locator("html");
  await expect(html).toHaveAttribute("data-theme", "arctic");
  await expect(html).not.toHaveClass(/\bdark\b/);
  await page.keyboard.press("ControlOrMeta+,");
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("button", { name: "Appearance" }).click();
  await expect(dialog.getByRole("radio", { name: "Arctic" })).toHaveAttribute("aria-checked", "true");

  await dialog.getByRole("radio", { name: "Nebula" }).click();
  await expect.poll(() => backend.settings.general.theme).toBe("nebula");
  await expect(html).toHaveAttribute("data-theme", "nebula");
  await expect(html).toHaveClass(/\bdark\b/);

  await dialog.getByRole("radio", { name: "Sage & Linen" }).click();
  await expect.poll(() => backend.settings.general.theme).toBe("sage");
  await expect(html).toHaveAttribute("data-theme", "sage");
  await expect(html).not.toHaveClass(/\bdark\b/);
});
