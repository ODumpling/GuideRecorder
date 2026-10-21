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

test("exports a vertical GIF without zoom, remembering the options", async ({ page, backend }) => {
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export guide" });
  await dialog.getByRole("radio", { name: /GIF/ }).click();
  // Paper and branding belong to the documents.
  await expect(dialog.getByRole("radiogroup", { name: "Paper size" })).toHaveCount(0);
  await expect(dialog.getByText("Branding (optional)")).toHaveCount(0);
  await dialog.getByRole("radio", { name: /Vertical/ }).click();
  await dialog.getByLabel("Zoom in on each click").uncheck();

  const exportButton = dialog.getByRole("button", { name: "Export", exact: true });
  backend.dialog.file = "/Users/me/Desktop/Rotate an API key";
  await dialog.getByRole("button", { name: "Choose…" }).click();
  await expect(dialog.getByText("Rotate an API key.gif")).toBeVisible();
  const seconds = dialog.getByRole("spinbutton");
  await seconds.fill("45");
  await expect(exportButton).toBeDisabled();
  await seconds.fill("2.5");
  await exportButton.click();

  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  expect(backend.exports.at(-1)).toEqual({
    guideId: API_KEY,
    format: "gif",
    out: "/Users/me/Desktop/Rotate an API key.gif",
    overwrite: false,
    branding: { logo: null, accent: null },
    motion: { secondsPerStep: 2.5, aspect: "vertical", zoom: false, captions: true },
  });

  await dialog.getByRole("button", { name: "Done" }).click();
  await page.getByRole("button", { name: "Export…" }).click();
  await expect(dialog.getByRole("radio", { name: /Vertical/ })).toHaveAttribute("aria-checked", "true");
  await expect(dialog.getByLabel("Zoom in on each click")).not.toBeChecked();
  await expect(dialog.getByRole("spinbutton")).toHaveValue("2.5");
});

test("previews the export's pages with the chosen logo", async ({ page, backend }) => {
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export guide" });
  await dialog.getByRole("radio", { name: /PDF/ }).click();
  const preview = dialog.getByLabel("Export preview");
  await expect(preview.getByText("Page 1 of 3")).toBeVisible();
  await preview.getByRole("button", { name: "Next page" }).click();
  await expect(preview.getByText("Page 2 of 3")).toBeVisible();
  await preview.getByRole("button", { name: "Page 3 of 3" }).click();
  await expect(preview.getByRole("button", { name: "Next page" })).toBeDisabled();

  backend.dialog.logo = "/Users/me/logo.png";
  await dialog.getByRole("button", { name: "Choose logo…" }).click();
  await expect
    .poll(() => backend.previews.at(-1))
    .toEqual({ format: "pdf", branding: { logo: "/Users/me/logo.png", accent: null } });
  // Typing an accent makes one preview once it is a colour.
  const before = backend.previews.length;
  await dialog.getByPlaceholder(/Accent colour/).pressSequentially("#336699");
  await expect.poll(() => backend.previews.at(-1)?.branding.accent).toBe("#336699");
  expect(backend.previews.length - before).toBeLessThanOrEqual(2);

  await dialog.getByRole("radio", { name: /Word/ }).click();
  await expect(preview.getByText(/this is the PDF’s layout/)).toBeVisible();
  await dialog.getByRole("radio", { name: /Markdown/ }).click();
  await preview.getByRole("radio", { name: "Source" }).click();
  await expect(preview.getByText("## 1. Step")).toBeVisible();
});

test("exports an agent skill with the replay script, after naming an unclear element", async ({ page, backend }) => {
  // Step 3's element was recorded as an unlabelled group.
  backend.editStep(API_KEY, "01M4WZCE2000000000000002R8", (step) => {
    step.element = { ...step.element, role: "AXGroup", label: null };
  });
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export guide" });
  await dialog.getByRole("radio", { name: /Agent skill/ }).click();
  // Skills carry no branding; the preview shows SKILL.md.
  await expect(dialog.getByText("Branding (optional)")).toHaveCount(0);
  const preview = dialog.getByLabel("Export preview");
  await expect.poll(() => backend.previews.at(-1)?.format).toBe("skill");
  await preview.getByRole("radio", { name: "Source" }).click();
  await expect(preview.getByText("name: guide")).toBeVisible();

  const unclear = dialog.getByRole("status", { name: "Unclear elements" });
  await expect(unclear).toContainText("The element of step 3 was not identified clearly");
  await unclear.getByRole("button", { name: "Show step" }).click();
  await expect(dialog).toBeHidden();

  // The inspector says why and lets the author name it.
  await expect(page.getByRole("status").filter({ hasText: "Unclear element:" })).toContainText("no label and a generic role");
  await page.getByRole("textbox", { name: "Element name" }).fill("Create key");
  await expect(page.getByRole("status").filter({ hasText: "Unclear element:" })).toContainText("a generic role");

  await page.getByRole("button", { name: "Export…" }).click();
  await expect(dialog.getByRole("radio", { name: /Agent skill/ })).toHaveAttribute("aria-checked", "true");
  await dialog.getByLabel(/Include a replay script/).check();
  backend.dialog.folder = "/Users/me/Skills";
  await dialog.getByRole("button", { name: "Choose…" }).click();
  await dialog.getByRole("button", { name: "Export", exact: true }).click();

  await expect(dialog.getByText(/Exported to/)).toContainText("SKILL.md");
  expect(backend.exports.at(-1)).toEqual({
    guideId: API_KEY,
    format: "skill",
    out: "/Users/me/Skills/Rotate an API key",
    overwrite: false,
    branding: { logo: null, accent: null },
    skill: { replay: true },
  });
  // The name was saved with the guide before the export.
  const step = backend.lastSave(API_KEY).items.find((i) => i.id === "01M4WZCE2000000000000002R8");
  expect(step?.type === "step" && step.element?.label).toBe("Create key");

  // The replay choice is remembered.
  await dialog.getByRole("button", { name: "Done" }).click();
  await page.getByRole("button", { name: "Export…" }).click();
  await expect(dialog.getByLabel(/Include a replay script/)).toBeChecked();
});
