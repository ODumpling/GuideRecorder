import type { Page } from "@playwright/test";
import { CALENDAR, expect, openGuide, saved, test } from "./fixtures";

const history = (page: Page) => page.getByRole("dialog", { name: "Version history" });
const versions = (page: Page) => history(page).getByRole("list", { name: "Versions" }).getByRole("button");

async function retitle(page: Page, title: string) {
  const field = page.getByRole("textbox", { name: "Guide title" });
  await field.fill(title);
  await field.blur();
  await saved(page);
}

test("saves a named version, shows what changed since and restores it", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  await page.getByRole("button", { name: "Version history" }).click();
  await expect(history(page).getByText("No versions yet.")).toBeVisible();

  await history(page).getByRole("textbox", { name: "Version name" }).fill("Before review");
  await history(page).getByRole("button", { name: "Save version" }).click();
  await expect(versions(page)).toHaveCount(1);
  await expect(versions(page).first()).toContainText("Before review");
  await page.keyboard.press("Escape");

  await retitle(page, "Share an event");
  await page.getByRole("button", { name: "Version history" }).click();
  await versions(page).first().click();
  const changes = history(page).getByLabel("Changes");
  await expect(changes.locator('[data-kind="removed"]')).toHaveText(["− Title: Share an event"]);
  await expect(changes.locator('[data-kind="added"]')).toHaveText(["+ Title: Share a calendar event"]);

  await history(page).getByRole("tab", { name: "Preview" }).click();
  await expect(history(page).getByRole("heading", { name: "Share a calendar event" })).toBeVisible();
  await expect(history(page).locator("img").first()).toHaveAttribute("src", /^blob:/);

  await history(page).getByRole("button", { name: "Restore this version" }).click();
  await expect(history(page)).toBeHidden();
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue("Share a calendar event");
  expect(backend.guide(CALENDAR).title).toBe("Share a calendar event");
  // The title restored over was kept as a version first.
  await page.getByRole("button", { name: "Version history" }).click();
  await expect(versions(page)).toHaveCount(2);
  await expect(versions(page).first()).toContainText("Before a restore");
  // A restore starts the undo history over.
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
});

test("closing a guide saves a version once", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  await retitle(page, "Share an event");
  expect(backend.guide(CALENDAR).title).toBe("Share an event");
  await page.getByRole("button", { name: "Library" }).click();
  await openGuide(page, "Share an event");
  await page.getByRole("button", { name: "Library" }).click();
  await openGuide(page, "Share an event");
  await page.getByRole("button", { name: "Version history" }).click();
  // The second close changed nothing, so it saved nothing.
  await expect(versions(page)).toHaveCount(1);
  await expect(versions(page).first()).toContainText("Closed");
  await history(page).getByRole("combobox", { name: "Compare with" }).selectOption("current");
  await expect(history(page).getByText("No text changes.")).toBeVisible();
});
