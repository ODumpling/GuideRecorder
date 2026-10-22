import { API_KEY, expect, openGuide, test } from "./fixtures";

test("makes a brand preset in Settings, the library's default for exports", async ({ page, backend }) => {
  await openGuide(page, "Rotate an API key");
  await page.keyboard.press("ControlOrMeta+,");
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Export" }).click();
  await expect(settings.getByRole("heading", { name: "Brand presets" })).toBeVisible();

  await settings.getByRole("button", { name: "New preset" }).click();
  const editor = settings.getByRole("group", { name: "New preset" });
  const add = editor.getByRole("button", { name: "Add preset" });
  await editor.getByLabel("Preset name").fill("Acme");
  backend.dialog.logo = "/Users/me/acme.png";
  await editor.getByRole("button", { name: "Choose logo…" }).click();
  await expect(editor.getByRole("button", { name: "Remove" })).toBeVisible();
  await editor.getByLabel("Accent", { exact: true }).fill("#zz");
  await expect(add).toBeDisabled();
  await editor.getByLabel("Accent", { exact: true }).fill("#aa1122");
  await editor.getByLabel("Heading font kind").selectOption("serif");
  await editor.getByLabel("Company").fill("Acme Corp");
  await editor.getByLabel("Footer text").fill("Confidential");
  await editor.getByRole("switch", { name: "Page numbers" }).click();
  await editor.getByLabel("Preset layout").selectOption("side-by-side");
  await editor.getByLabel("Custom CSS").fill("</style>");
  await expect(add).toBeDisabled();
  await editor.getByLabel("Custom CSS").fill(".step { border-radius: 0; }");
  await add.click();

  await expect(editor).toHaveCount(0);
  await expect(settings.getByText("Side by side")).toBeVisible();
  const saved = backend.presets.get("acme");
  expect(saved?.logo).toMatch(/^data:image\/png/);
  expect(saved?.preset).toMatchObject({
    name: "Acme",
    colors: { accent: "#aa1122", heading: null, text: null },
    fonts: { body: { kind: "sans", family: null }, heading: { kind: "serif", family: null } },
    cover: { company: "Acme Corp", author: "", version: "", date: true },
    footer: "Confidential",
    pageNumbers: false,
    layout: "side-by-side",
    css: ".step { border-radius: 0; }",
  });

  await settings.getByLabel("Library default preset").selectOption("acme");
  await expect.poll(() => backend.defaultPreset).toBe("acme");
  await settings.getByRole("button", { name: "Close" }).click();

  // The Export dialog starts with the default, and sends it with the layout.
  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export guide" });
  await dialog.getByRole("radio", { name: /PDF/ }).click();
  const preset = dialog.getByLabel("Brand preset");
  await expect(preset).toHaveValue("acme");
  await expect(preset.locator("option:checked")).toHaveText("Acme (library default)");
  // A preset brings its own logo and accent.
  await expect(dialog.getByPlaceholder(/Accent colour/)).toHaveCount(0);
  await expect(dialog.getByLabel("Layout")).toHaveValue("side-by-side");
  await expect
    .poll(() => backend.previews.at(-1)?.branding)
    .toEqual({ logo: null, accent: null, preset: "acme", layout: null });
  await dialog.getByLabel("Layout").selectOption("checklist");
  await expect.poll(() => backend.previews.at(-1)?.branding.layout).toBe("checklist");

  backend.dialog.file = "/Users/me/Desktop/Rotate an API key";
  await dialog.getByRole("button", { name: "Choose…" }).click();
  await dialog.getByRole("button", { name: "Export", exact: true }).click();
  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  expect(backend.exports.at(-1)).toEqual({
    guideId: API_KEY,
    format: "pdf",
    out: "/Users/me/Desktop/Rotate an API key.pdf",
    overwrite: false,
    branding: { logo: null, accent: null, preset: "acme", layout: "checklist" },
  });

  // None is remembered; the logo and accent are back.
  await dialog.getByRole("button", { name: "Export again" }).click();
  await preset.selectOption("");
  await expect(dialog.getByPlaceholder(/Accent colour/)).toBeVisible();
  await dialog.getByLabel("Replace an earlier export there").check();
  await dialog.getByRole("button", { name: "Export", exact: true }).click();
  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  expect(backend.exports.at(-1)?.branding).toEqual({ logo: null, accent: null, preset: null, layout: null });
  await dialog.getByRole("button", { name: "Done" }).click();
  await page.getByRole("button", { name: "Export…" }).click();
  await expect(page.getByLabel("Brand preset")).toHaveValue("");
});

test("edits, renames and deletes a preset", async ({ page, backend }) => {
  backend.presets.set("acme", {
    preset: {
      name: "Acme",
      colors: { accent: null, heading: null, text: null },
      fonts: { body: { kind: "sans", family: null }, heading: { kind: "sans", family: null } },
      cover: { company: "", author: "", version: "", date: true },
      header: "",
      footer: "",
      pageNumbers: true,
      layout: "compact",
      css: "",
    },
    logo: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
  });
  backend.defaultPreset = "acme";
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+,");
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Export" }).click();
  await expect(settings.getByText("Acme · default")).toBeVisible();

  await settings.getByRole("button", { name: "Edit" }).click();
  const editor = settings.getByRole("group", { name: "Edit Acme" });
  await editor.getByLabel("Preset name").fill("Acme Corp");
  await editor.getByLabel("Text font family").fill("Source Sans 3");
  await editor.getByRole("button", { name: "Save preset" }).click();
  await expect(settings.getByText("Acme Corp · default")).toBeVisible();
  // Same id, the logo kept.
  expect([...backend.presets.keys()]).toEqual(["acme"]);
  expect(backend.presets.get("acme")?.logo).toMatch(/^data:/);
  expect(backend.presets.get("acme")?.preset.fonts.body.family).toBe("Source Sans 3");

  // Delete asks for a second click.
  await settings.getByRole("button", { name: "Delete" }).click();
  await settings.getByRole("button", { name: "Delete?" }).click();
  await expect(settings.getByText("Acme Corp")).toHaveCount(0);
  expect(backend.presets.size).toBe(0);
  expect(backend.defaultPreset).toBeNull();
});
