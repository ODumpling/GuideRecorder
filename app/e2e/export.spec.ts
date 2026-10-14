import { API_KEY, expect, openGuide, test } from "./fixtures";

test("exports the open guide as PDF, then replaces it", async ({ page, backend }) => {
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export guide" });
  await expect(dialog).toBeVisible();

  await dialog.getByRole("radio", { name: /PDF/ }).click();
  const exportButton = dialog.getByRole("button", { name: "Export", exact: true });
  // No destination yet.
  await expect(exportButton).toBeDisabled();
  backend.dialog.file = "/Users/me/Desktop/Rotate an API key";
  await dialog.getByRole("button", { name: "Choose…" }).click();
  await expect(dialog.getByText("Rotate an API key.pdf")).toBeVisible();

  await dialog.getByPlaceholder(/Accent colour/).fill("#zz");
  await expect(exportButton).toBeDisabled();
  await dialog.getByPlaceholder(/Accent colour/).fill("#336699");
  await exportButton.click();

  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  expect(backend.exports).toEqual([
    {
      guideId: API_KEY,
      format: "pdf",
      out: "/Users/me/Desktop/Rotate an API key.pdf",
      overwrite: false,
      branding: { logo: null, accent: "#336699" },
    },
  ]);
  // The editor saved (or had nothing to save) before exporting.
  await dialog.getByRole("button", { name: "Reveal in Finder" }).click();
  await expect.poll(() => backend.revealed).toEqual(["/Users/me/Desktop/Rotate an API key.pdf"]);

  // Again to the same file: refused until Replace is ticked.
  await dialog.getByRole("button", { name: "Export again" }).click();
  await exportButton.click();
  await expect(dialog.getByRole("alert")).toContainText("already exists");
  await expect(dialog.getByRole("alert")).toContainText("Replace an earlier export there");
  await dialog.getByLabel("Replace an earlier export there").check();
  await exportButton.click();
  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  expect(backend.exports.at(-1)?.overwrite).toBe(true);

  // Replace is not remembered for the next export.
  await dialog.getByRole("button", { name: "Done" }).click();
  await page.getByRole("button", { name: "Export…" }).click();
  await expect(page.getByLabel("Replace an earlier export there")).not.toBeChecked();
});

test("exports Markdown into a named folder with a logo", async ({ page, backend }) => {
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export guide" });
  await dialog.getByRole("radio", { name: /Markdown/ }).click();
  backend.dialog.folder = "/Users/me/Exports";
  await dialog.getByRole("button", { name: "Choose…" }).click();
  await expect(dialog.getByText("Exports", { exact: true })).toBeVisible();
  await dialog.getByRole("textbox").first().fill("API key guide");
  backend.dialog.logo = "/Users/me/logo.png";
  await dialog.getByRole("button", { name: "Choose logo…" }).click();
  await expect(dialog.getByText("logo.png")).toBeVisible();
  await dialog.getByRole("button", { name: "Export", exact: true }).click();

  await expect(dialog.getByText(/Exported to/)).toContainText("guide.md");
  expect(backend.exports.at(-1)).toMatchObject({
    format: "md",
    out: "/Users/me/Exports/API key guide",
    branding: { logo: "/Users/me/logo.png", accent: null },
  });
});
