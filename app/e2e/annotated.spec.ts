import type { Locator, Page } from "@playwright/test";
import type { Annotation, Step } from "@guiderecorder/guide-schema";
import type { Backend } from "./backend";
import { expect, outlineItems, saved, test } from "./fixtures";

const layer = (page: Page) => page.getByRole("img", { name: "Page with annotations" });

/** Drags across `target` from one fraction of it to another. */
async function drag(page: Page, target: Locator, from: [number, number], to: [number, number]) {
  const box = (await target.boundingBox())!;
  const at = ([x, y]: [number, number]) => [box.x + box.width * x, box.y + box.height * y] as const;
  await page.mouse.move(...at(from));
  await page.mouse.down();
  await page.mouse.move(...at([(from[0] + to[0]) / 2, (from[1] + to[1]) / 2]), { steps: 4 });
  await page.mouse.move(...at(to), { steps: 4 });
  await page.mouse.up();
}

/** Moves the pointer to a fraction of `target`. */
async function pointAt(page: Page, target: Locator, [x, y]: [number, number]) {
  const box = (await target.boundingBox())!;
  await page.mouse.move(box.x + box.width * x, box.y + box.height * y);
}

/** New Guide, then Annotation guide. */
async function chooseAnnotationGuide(page: Page) {
  await page.getByRole("button", { name: "New Guide" }).click();
  await page.getByRole("dialog", { name: "New guide" }).getByRole("button", { name: "Annotation guide" }).click();
}

/** Makes an annotation guide from the library (the mock's dialog picks two pages) and waits for it. */
async function newGuide(page: Page, backend: Backend): Promise<string> {
  await chooseAnnotationGuide(page);
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue(/^Floor plan/);
  return backend.annotatedGuides.at(-1)!;
}

/** The hotspots of a page, as last saved. */
function savedHotspots(backend: Backend, guideId: string, index = 0): Annotation[] {
  const items = backend.saves.findLast((g) => g.id === guideId)?.items ?? [];
  return (items[index] as Step | undefined)?.annotations.filter((a) => a.type === "hotspot") ?? [];
}

/** A hotspot's text. */
const geometry = (a: Annotation | undefined) =>
  a?.type === "hotspot" ? { title: a.geometry.title, description: a.geometry.description } : null;

test("makes an annotation guide and explains a page with a hotspot, undoably", async ({ page, backend }) => {
  const id = await newGuide(page, backend);
  expect(backend.calls.map((c) => c.cmd)).toContain("new_annotated_guide");
  // Listed, opened, and its first page selected.
  await expect(outlineItems(page)).toHaveCount(2);
  await expect(outlineItems(page).first()).toHaveText(/^1Page 1/);
  await expect(page.getByRole("textbox", { name: "Page title" })).toHaveText("Page 1");
  await expect(page.getByRole("textbox", { name: "Page description" })).toBeVisible();
  await expect(page.getByRole("toolbar", { name: "page actions" })).toBeVisible();
  // Walkthrough-only controls are left out.
  await expect(page.getByRole("button", { name: "Write with AI" })).toHaveCount(0);
  await expect(page.getByRole("toolbar", { name: "AI actions" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Regenerate guide details" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Split" })).toHaveCount(0);
  await expect(page.getByRole("group", { name: "Image" })).toHaveCount(0);
  await expect(page.getByTestId("step-status")).toHaveCount(0);
  await expect(page.getByText("Drag over the image to add a hotspot the reader can point at.")).toBeVisible();

  // The hotspot tool is ready: drag, then type.
  await expect(page.getByRole("button", { name: "Hotspot", exact: true })).toHaveAttribute("aria-pressed", "true");
  await drag(page, layer(page), [0.2, 0.2], [0.5, 0.5]);
  const title = page.getByRole("textbox", { name: "Hotspot title" });
  await expect(title).toBeFocused();
  // Untitled, it is dashed and listed as such.
  await expect(layer(page).locator("[data-hotspot] rect")).toHaveAttribute("stroke-dasharray", "6 4");
  await expect(page.getByRole("list", { name: "Hotspots" })).toHaveText("1Untitled hotspot");
  await page.keyboard.type("Kitchen");
  await page.getByRole("textbox", { name: "Hotspot description" }).click();
  await page.keyboard.type("Gas hob");
  await page.keyboard.press("Enter");
  await page.keyboard.type("North window");

  await expect.poll(() => savedHotspots(backend, id).map(geometry)).toEqual([
    { title: "Kitchen", description: "Gas hob\nNorth window" },
  ]);
  const [hotspot] = savedHotspots(backend, id);
  if (hotspot.type !== "hotspot") throw new Error("not a hotspot");
  expect(hotspot.geometry.width).toBeGreaterThan(0);
  expect(hotspot.geometry.height).toBeGreaterThan(0);
  await expect(layer(page).locator("[data-hotspot] rect")).not.toHaveAttribute("stroke-dasharray");
  await expect(layer(page).locator("[data-hotspot] text")).toHaveText("1");
  await expect(page.getByRole("list", { name: "Hotspots" })).toHaveText("1KitchenGas hob North window");

  // Each field's typing is one undo step, then the hotspot itself.
  await page.keyboard.press("ControlOrMeta+z");
  await expect(title).toHaveValue("Kitchen");
  await expect(page.getByRole("textbox", { name: "Hotspot description" })).toHaveValue("");
  await page.keyboard.press("ControlOrMeta+z");
  await expect(title).toHaveValue("");
  await page.keyboard.press("ControlOrMeta+z");
  await expect(layer(page).locator("[data-hotspot]")).toHaveCount(0);
  await expect.poll(() => savedHotspots(backend, id)).toEqual([]);
  for (let i = 0; i < 3; i++) await page.keyboard.press("ControlOrMeta+Shift+z");
  await expect(page.getByRole("textbox", { name: "Hotspot description" })).toHaveValue("Gas hob\nNorth window");
  await expect.poll(() => savedHotspots(backend, id).map(geometry)).toEqual([
    { title: "Kitchen", description: "Gas hob\nNorth window" },
  ]);
  await saved(page);
});

test("adds pages from the insert menu as one undo step", async ({ page, backend }) => {
  const id = await newGuide(page, backend);
  const before = backend.guide(id).items.map((item) => item.id);
  backend.pagesDialog = 3;
  await page.getByRole("button", { name: "Insert" }).click();
  await expect(page.getByRole("menuitem", { name: "Step from image…" })).toHaveCount(0);
  await page.getByRole("menuitem", { name: "Add pages…" }).click();

  // After the selected first page; the first new one is selected.
  await expect(outlineItems(page)).toHaveCount(5);
  await expect(page.getByRole("textbox", { name: "Page title" })).toHaveText("Page 3");
  await expect.poll(() => backend.saves.findLast((g) => g.id === id)?.items.length).toBe(5);
  const items = backend.lastSave(id).items as Step[];
  expect(items.map((item) => item.id)).toEqual([before[0], ...items.slice(1, 4).map((item) => item.id), before[1]]);
  expect(items.slice(1, 4).map((item) => [item.type, item.kind, item.image.show])).toEqual(
    Array(3).fill(["step", "manual", "full"]),
  );
  for (const item of items) expect(backend.hasFile(id, item.image.full)).toBe(true);

  await page.getByRole("button", { name: "Undo" }).click();
  await expect(outlineItems(page)).toHaveCount(2);
  await expect.poll(() => backend.lastSave(id).items.map((item) => item.id)).toEqual(before);

  // Cancelled: nothing changes.
  backend.pagesDialog = null;
  await page.getByRole("button", { name: "Insert" }).click();
  await page.getByRole("menuitem", { name: "Add pages…" }).click();
  await expect.poll(() => backend.calls.filter((c) => c.cmd === "add_annotated_pages").length).toBe(2);
  await expect(outlineItems(page)).toHaveCount(2);
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
});

test("shows hotspot text on pointing, picks hotspots from the list, and previews the page", async ({ page, backend }) => {
  await newGuide(page, backend);
  const title = page.getByRole("textbox", { name: "Hotspot title" });
  await drag(page, layer(page), [0.1, 0.1], [0.3, 0.3]);
  await page.keyboard.type("Hall");
  // The hotspot tool again, for a second one.
  await page.getByRole("button", { name: "Hotspot", exact: true }).click();
  await drag(page, layer(page), [0.6, 0.6], [0.9, 0.9]);
  await expect(title).toHaveValue("");
  await page.keyboard.type("Garden");
  await expect(layer(page).locator("[data-hotspot] text")).toHaveText(["1", "2"]);

  // Pointing at the unselected one shows its text.
  const tooltip = page.getByRole("tooltip");
  await pointAt(page, layer(page), [0.2, 0.2]);
  await expect(tooltip).toHaveText("1Hall");
  await pointAt(page, layer(page), [0.45, 0.45]);
  await expect(tooltip).toHaveCount(0);

  // The inspector's list selects.
  const list = page.getByRole("list", { name: "Hotspots" });
  await list.getByRole("button", { name: /Hall/ }).click();
  await expect(title).toHaveValue("Hall");
  await expect(list.getByRole("button", { name: /Hall/ })).toHaveAttribute("aria-current", "true");

  // Preview: no tools; pointing shows the text, a click keeps it.
  await page.getByRole("button", { name: "Preview" }).click();
  const preview = page.getByRole("img", { name: "Page preview" });
  await expect(page.getByRole("button", { name: "Hotspot", exact: true })).toHaveCount(0);
  await expect(title).toHaveCount(0);
  await pointAt(page, preview, [0.75, 0.75]);
  await expect(tooltip).toHaveText("2Garden");
  await page.mouse.down();
  await page.mouse.up();
  await pointAt(page, preview, [0.45, 0.45]);
  await expect(tooltip).toHaveText("2Garden");
  await page.keyboard.press("Escape");
  await expect(tooltip).toHaveCount(0);
  // A popover pinned when preview ends doesn't come back with it.
  await pointAt(page, preview, [0.75, 0.75]);
  await page.mouse.down();
  await page.mouse.up();
  await page.getByRole("button", { name: "Preview" }).click();
  await expect(page.getByRole("button", { name: "Hotspot", exact: true })).toBeVisible();
  await page.mouse.move(0, 0);
  await page.getByRole("button", { name: "Preview" }).click();
  await expect(tooltip).toHaveCount(0);
  await page.getByRole("button", { name: "Preview" }).click();
});

test("a cancelled or failed new annotation guide opens nothing", async ({ page, backend }) => {
  backend.pagesDialog = null;
  const button = page.getByRole("button", { name: "New Guide" });
  await chooseAnnotationGuide(page);
  await expect.poll(() => backend.calls.filter((c) => c.cmd === "new_annotated_guide").length).toBe(1);
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveCount(0);
  await expect(button).toBeEnabled();

  backend.pagesError = { kind: "invalid", message: "The PDF could not be read" };
  await chooseAnnotationGuide(page);
  await expect(page.getByText("Could not make the annotation guide: The PDF could not be read")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveCount(0);
});
