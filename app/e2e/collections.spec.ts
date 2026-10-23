import { API_KEY, CALENDAR, expect, test } from "./fixtures";
import type { Page } from "@playwright/test";

const contents = (page: Page) => page.getByRole("list", { name: "Contents" }).locator(":scope > li");

async function newCollection(page: Page, title: string) {
  await page.getByRole("button", { name: "New collection" }).click();
  const dialog = page.getByRole("dialog", { name: "New collection" });
  await dialog.getByRole("textbox", { name: "Title" }).fill(title);
  await dialog.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("textbox", { name: "Collection title" })).toHaveValue(title);
}

test("builds a collection from guides and text, and saves it", async ({ page, backend }) => {
  await newCollection(page, "Team handbook");
  const nav = page.getByRole("navigation", { name: "Library" });
  await expect(nav.getByRole("button", { name: /Team handbook/ })).toHaveAttribute("aria-current", "true");
  await expect(page.getByText("0 guides · 0 steps")).toBeVisible();

  const add = page.getByRole("combobox", { name: "Add a guide" });
  await add.selectOption({ label: "Share a calendar event" });
  await page.getByRole("button", { name: "Add text" }).click();
  await page.getByRole("textbox", { name: "Text between guides" }).fill("Next, the security basics.");
  await add.selectOption({ label: "Rotate an API key" });
  await expect(contents(page)).toHaveCount(3);
  await expect(contents(page).nth(0)).toContainText("Share a calendar event");
  await expect(contents(page).nth(2)).toContainText("Rotate an API key");
  // Guides are numbered as chapters; texts are not.
  await expect(contents(page).nth(2)).toContainText("2");

  await page.getByRole("button", { name: "Move “Rotate an API key” up" }).click();
  await page.getByRole("button", { name: "Move “Rotate an API key” up" }).click();
  await expect(contents(page).nth(0)).toContainText("Rotate an API key");
  await page.getByRole("radio", { name: /Per guide/ }).click();
  await page.getByRole("textbox", { name: "Collection title" }).fill("Team handbook 2026");

  await expect
    .poll(() => {
      const [c] = backend.collections.values();
      return c && { title: c.title, numbering: c.numbering, entries: c.entries.map((e) => (e.type === "guide" ? e.guideId : e.body)) };
    })
    .toEqual({
      title: "Team handbook 2026",
      numbering: "per-chapter",
      entries: [API_KEY, CALENDAR, "Next, the security basics."],
    });

  // The sidebar's other views show the guides again.
  await nav.getByRole("button", { name: /All guides/ }).click();
  await expect(page.getByRole("heading", { name: "All guides" })).toBeVisible();
  await nav.getByRole("button", { name: /Team handbook 2026/ }).click();
  await expect(contents(page)).toHaveCount(3);
});

test("exports a collection as one PDF, and notes a guide that is gone", async ({ page, backend }) => {
  await newCollection(page, "Manual");
  await page.getByRole("combobox", { name: "Add a guide" }).selectOption({ label: "Rotate an API key" });
  await expect.poll(() => [...backend.collections.values()][0]?.entries.length).toBe(1);
  // A guide deleted elsewhere since; the library is read again at start.
  const [stored] = backend.collections.values();
  stored!.entries.push({ type: "guide", id: "01JC0000000000000000000099", guideId: "01JC0000000000000000000098" });
  await page.reload();
  await page.getByRole("navigation", { name: "Library" }).getByRole("button", { name: /Manual/ }).click();
  await expect(page.getByText(/no longer in the library/)).toBeVisible();

  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export collection" });
  const exportButton = dialog.getByRole("button", { name: "Export", exact: true });
  await expect(exportButton).toBeDisabled();
  backend.dialog.file = "/Users/me/Desktop/Manual";
  await dialog.getByRole("button", { name: "Choose…" }).click();
  await expect(dialog.getByText("Manual.pdf")).toBeVisible();
  await dialog.getByRole("radio", { name: /Letter/ }).click();
  await exportButton.click();

  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  await expect(dialog.getByText(/no longer in the library and were left out/)).toBeVisible();
  expect(backend.collectionExports).toEqual([
    {
      id: stored!.id,
      format: "pdf",
      out: "/Users/me/Desktop/Manual.pdf",
      overwrite: false,
      paper: "letter",
      branding: { logo: null, accent: null, preset: null, layout: null },
    },
  ]);

  // Markdown goes into a named folder.
  await dialog.getByRole("button", { name: "Export again" }).click();
  await dialog.getByRole("radio", { name: /Markdown/ }).click();
  backend.dialog.folder = "/Users/me/Exports";
  await dialog.getByRole("button", { name: "Choose…" }).click();
  await expect(dialog.getByRole("textbox", { name: "Export folder name" })).toHaveValue("Manual");
  await exportButton.click();
  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  expect(backend.collectionExports.at(-1)).toMatchObject({ format: "md", out: "/Users/me/Exports/Manual" });
});

test("deletes a collection but keeps its guides", async ({ page, backend }) => {
  await newCollection(page, "Old manual");
  await page.getByRole("combobox", { name: "Add a guide" }).selectOption({ label: "Rotate an API key" });
  await page.getByRole("button", { name: "Delete collection" }).click();
  await page.getByRole("button", { name: "Delete collection" }).click();
  await expect(page.getByRole("heading", { name: "All guides" })).toBeVisible();
  expect(backend.collections.size).toBe(0);
  await expect(page.getByText("Put guides together into one manual")).toBeVisible();
  await expect(page.getByRole("button", { name: /Rotate an API key/ }).first()).toBeVisible();
});
