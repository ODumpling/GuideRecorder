import type { Page } from "@playwright/test";
import { test, expect } from "./fixtures";

/** The list's rows: the header, then a row per guide. */
const rows = (page: Page) => page.getByRole("grid", { name: "Guides" }).getByRole("row");

test("shows the library as a list, remembered after a reload", async ({ page, backend: _ }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.getByRole("radio", { name: "List" }).click();
  await expect(page.getByRole("columnheader", { name: "Review due" })).toBeVisible();
  await expect(rows(page)).toHaveCount(5);

  await page.getByRole("button", { name: "Title" }).click();
  await expect(rows(page).nth(1)).toContainText("Kurzanleitung Drucker");
  await page.getByRole("button", { name: "Title" }).click();
  await expect(rows(page).nth(1)).toContainText("Share a calendar event");

  await page.reload();
  await expect(page.getByRole("radio", { name: "List" })).toHaveAttribute("aria-checked", "true");
  await expect(rows(page).nth(1)).toContainText("Share a calendar event");
});

test("sets the owner of the selected guides at once", async ({ page, backend }) => {
  await page.getByRole("radio", { name: "List" }).click();
  await page.getByRole("button", { name: "Title" }).click();
  await rows(page).nth(1).click({ modifiers: ["ControlOrMeta"] });
  await rows(page).nth(3).click({ modifiers: ["Shift"] });
  const bar = page.getByRole("toolbar", { name: "Selected guides" });
  await expect(bar).toContainText("3 selected");

  await bar.getByRole("button", { name: "Owner" }).click();
  const dialog = page.getByRole("dialog", { name: "Set the owner of 3 guides" });
  await dialog.getByRole("combobox", { name: "Owner" }).fill("IT");
  await dialog.getByRole("button", { name: "Set owner" }).click();
  await expect(dialog).toBeHidden();
  expect(backend.calls.filter((c) => c.cmd === "edit_guide_details")).toHaveLength(3);
  await expect(page.getByLabel("Owner IT", { exact: true })).toHaveCount(3);

  // Arrow keys move the selection with the focus.
  await rows(page).nth(4).focus();
  await page.keyboard.press("ArrowUp");
  await expect(rows(page).nth(3)).toBeFocused();
  await expect(bar).toContainText("1 selected");
  await page.keyboard.press("Escape");
  await expect(bar).toBeHidden();
});

test("filters by status with the chips", async ({ page, backend: _ }) => {
  await page.getByRole("radio", { name: "Draft" }).click();
  await expect(page.getByText("No matching guides")).toBeVisible();
  await page.getByRole("radio", { name: "All" }).click();
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
});
