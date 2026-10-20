import type { Page } from "@playwright/test";
import { expect, openGuide, test } from "./fixtures";

const palette = (page: Page) => page.getByRole("dialog", { name: "Command palette" });
const search = (page: Page) => palette(page).getByRole("combobox", { name: "Search guides and commands" });
const options = (page: Page) => palette(page).getByRole("option");

async function openPalette(page: Page) {
  await page.keyboard.press("ControlOrMeta+k");
  await expect(search(page)).toBeFocused();
}

test("opens a guide from the palette, searched by title", async ({ page, backend: _ }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await openPalette(page);
  // Recent guides and commands before anything is typed, with shortcuts.
  await expect(options(page).filter({ hasText: "Settings" })).toContainText("⌘,");

  await search(page).fill("calndr");
  await expect(options(page).first()).toContainText("Share a calendar event");
  await page.keyboard.press("Enter");
  await expect(palette(page)).toBeHidden();
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue("Share a calendar event");

  // ⌘K closes it again.
  await openPalette(page);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(palette(page)).toBeHidden();
});

test("narrows to commands with > and to tags with #", async ({ page, backend: _ }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await openPalette(page);
  await search(page).fill(">rotate");
  await expect(options(page)).toHaveCount(0);
  await search(page).fill(">record");
  await expect(options(page).first()).toContainText("Record screen");

  await search(page).fill("#secur");
  await expect(options(page)).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: "#security" })).toBeVisible();
});

test("runs editor commands: jump to a step, export as a format, switch theme", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  await openPalette(page);
  await search(page).fill("step 2");
  await expect(options(page).first()).toContainText("Step 2:");
  await page.keyboard.press("Enter");
  await expect(page.getByText(/step 2 of/i)).toBeVisible();

  await openPalette(page);
  await search(page).fill("export as html");
  await page.keyboard.press("Enter");
  const exportDialog = page.getByRole("dialog", { name: "Export guide" });
  await expect(exportDialog.getByRole("radio", { name: /HTML/ })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Escape");
  await expect(exportDialog).toBeHidden();

  // A list to pick from: Backspace in the empty search goes back.
  await openPalette(page);
  await search(page).fill("switch theme");
  await page.keyboard.press("Enter");
  await expect(search(page)).toHaveValue("");
  await expect(options(page).filter({ hasText: "Theme: Arctic" })).toContainText("Current");
  await page.keyboard.press("Backspace");
  await expect(options(page).filter({ hasText: "Switch theme…" })).toBeVisible();
  await search(page).fill("nebula");
  await page.keyboard.press("Enter");
  await expect.poll(() => backend.settings.general.theme).toBe("nebula");
});

test("keeps the library search on ⌘F and from the palette", async ({ page, backend: _ }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+f");
  await expect(page.getByRole("textbox", { name: "Search guides" })).toBeFocused();

  await openPalette(page);
  await search(page).fill("drucker");
  await options(page).filter({ hasText: "Search the library for" }).click();
  await expect(page.getByRole("textbox", { name: "Search guides" })).toHaveValue("drucker");
  await expect(page.getByRole("heading", { name: "Search" })).toBeVisible();
});
