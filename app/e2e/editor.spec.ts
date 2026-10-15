import type { Page } from "@playwright/test";
import type { Editor } from "@tiptap/core";
import type { Annotation, Step } from "../src/lib/types";
import type { Backend } from "./backend";
import { API_KEY, CALENDAR, expect, openGuide, outlineItems, saved, test } from "./fixtures";

const screenshot = (page: Page) => page.getByRole("img", { name: "Step screenshot with annotations" });

/** Draws with the current annotation tool from one fraction of the screenshot to another. */
async function drawOnScreenshot(page: Page, from: [number, number], to: [number, number]) {
  const box = (await screenshot(page).boundingBox())!;
  const at = ([x, y]: [number, number]) => [box.x + box.width * x, box.y + box.height * y] as const;
  await page.mouse.move(...at(from));
  await page.mouse.down();
  await page.mouse.move(...at([(from[0] + to[0]) / 2, (from[1] + to[1]) / 2]), { steps: 4 });
  await page.mouse.move(...at(to), { steps: 4 });
  await page.mouse.up();
}

/** The items of the last saved guide; none before the first save. */
const lastItems = (backend: Backend) => backend.saves.at(-1)?.items ?? [];
const step = (items: { id: string }[], id: string) => items.find((item) => item.id === id) as Step | undefined;
/** A TipTap editor's root element, which carries its editor. */
type EditorElement = HTMLElement & { editor: Editor };
const blurs = (s: Pick<Step, "annotations">) => s.annotations.filter((a: Annotation) => a.type === "blur");

test("opens a guide from the library", async ({ page, backend }) => {
  await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
  await openGuide(page, "Share a calendar event");

  const titles = backend.guide(CALENDAR).items.length;
  await expect(outlineItems(page)).toHaveCount(titles);
  await expect(outlineItems(page).first()).toHaveText("Open the event");
  // The first step is selected, with its screenshot and Markdown title.
  await expect(page.getByRole("textbox", { name: "Step title" })).toHaveText("Click Calendar in the Dock");
  await expect(page.getByRole("textbox", { name: "Step title" }).locator("strong")).toHaveText("Calendar");
  await expect(screenshot(page).locator("image")).toHaveAttribute("href", /^blob:/);
  expect(backend.calls.map((c) => c.cmd)).toContain("step_image");
});

test("edits a step title with the editor, then undoes it", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  const title = page.getByRole("textbox", { name: "Step title" });
  await title.click();
  await page.keyboard.press("End");
  await page.keyboard.type(" now");
  await expect(title).toHaveText("Click Calendar in the Dock now");
  await expect.poll(() => step(lastItems(backend), "01M4WT7MM000000000000002G4")?.title).toBe(
    "Click **Calendar** in the Dock now",
  );

  await page.keyboard.press("ControlOrMeta+z");
  await expect(title).toHaveText("Click Calendar in the Dock");
  await expect.poll(() => backend.saves.length).toBe(2);
  expect(step(backend.lastSave(CALENDAR).items, "01M4WT7MM000000000000002G4")!.title).toBe(
    "Click **Calendar** in the Dock",
  );
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Redo" })).toBeEnabled();
  await saved(page);
});

test("reorders steps by dragging in the outline", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  const before = backend.guide(CALENDAR).items.map((item) => item.id);
  const row = (id: string) => page.locator("li", { has: page.locator(`[data-outline-item="${id}"]`) });

  // The double-click step (third) goes above the first section.
  await row(before[2]).dragTo(row(before[0]));

  const after = [before[2], before[0], before[1], ...before.slice(3)];
  await expect(outlineItems(page).first()).toHaveText(/Double-click the Team sync event/);
  await expect.poll(() => lastItems(backend).map((item) => item.id)).toEqual(after);
  // Step numbers follow the new order.
  await expect(outlineItems(page).first()).toHaveText(/^1/);
});

test("adds a blur annotation that is saved with the step", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  await page.getByRole("button", { name: "Blur", exact: true }).click();
  await drawOnScreenshot(page, [0.2, 0.2], [0.6, 0.7]);

  const id = "01M4WT7MM000000000000002G4";
  await expect.poll(() => blurs(step(lastItems(backend), id) ?? { annotations: [] }).length).toBe(1);
  const [blur] = blurs(step(backend.lastSave(CALENDAR).items, id)!);
  if (blur.type !== "blur") throw new Error("not a blur");
  expect(blur.geometry.width).toBeGreaterThan(0);
  expect(blur.geometry.height).toBeGreaterThan(0);
  // Drawn on screen, over the screenshot.
  await expect(screenshot(page).locator(`filter[id="redact-${blur.id}-blur"]`)).toHaveCount(1);
});

test("splits a step, and both halves keep the blur", async ({ page, backend }) => {
  const id = "01M4WT7MM000000000000002G7";
  const count = backend.guide(CALENDAR).items.length;
  await openGuide(page, "Share a calendar event");
  await page.locator(`[data-outline-item="${id}"]`).click();
  await expect(page.getByRole("textbox", { name: "Step title" })).toHaveText("Double-click the Team sync event");
  await page.getByRole("button", { name: "Blur", exact: true }).click();
  await drawOnScreenshot(page, [0.1, 0.1], [0.4, 0.5]);
  await expect.poll(() => backend.saves.length).toBeGreaterThan(0);

  // Cut the description "The event opens in its own window." after "opens".
  const description = page.getByRole("textbox", { name: "Step description" });
  await description.click();
  await expect(description).toBeFocused();
  // Arrow keys move the DOM cursor, and ProseMirror reads it back later from
  // `selectionchange`. Until it has, a timer it set on focus can put its own
  // older cursor back into the DOM, dropping the moves made so far. Repeat the
  // moves until both the DOM and the editor have the cursor after "opens".
  const cut = "The event opens".length;
  await expect(async () => {
    await page.keyboard.press("End");
    for (let i = 0; i < " in its own window.".length; i++) await page.keyboard.press("ArrowLeft");
    await expect
      .poll(
        () =>
          description.evaluate((el) => [
            getSelection()?.anchorOffset,
            (el as unknown as EditorElement).editor.state.selection.$from.parentOffset,
          ]),
        // A dropped move never recovers: retry soon.
        { timeout: 1_000 },
      )
      .toEqual([cut, cut]);
  }).toPass();
  await page.getByRole("button", { name: "Split" }).click();

  await expect(outlineItems(page)).toHaveCount(count + 1);
  await expect.poll(() => lastItems(backend).length).toBe(count + 1);
  const items = backend.lastSave(CALENDAR).items;
  const index = items.findIndex((item) => item.id === id);
  const first = items[index] as Step;
  const second = items[index + 1] as Step;
  expect(second.type).toBe("step");
  expect(first.description.trim()).toBe("The event opens");
  expect(second.description.trim()).toBe("in its own window.");

  expect(blurs(first)).toHaveLength(1);
  expect(blurs(second)).toHaveLength(1);
  expect(blurs(second)[0].geometry).toEqual(blurs(first)[0].geometry);
  expect(blurs(second)[0].id).not.toBe(blurs(first)[0].id);
  // The new half has its own copies of the screenshots.
  expect(second.image.full).toBe(`steps/${second.id}.webp`);
  expect(backend.hasFile(CALENDAR, second.image.full)).toBe(true);
  // The new half is selected, and its blur is drawn.
  await expect(page.getByRole("textbox", { name: "Step description" })).toHaveText("in its own window.");
  await expect(screenshot(page).locator(`filter[id="redact-${blurs(second)[0].id}-blur"]`)).toHaveCount(1);
});

test.describe("a guide from a newer GuidesYa", () => {
  test.use({ backendOptions: { schemaVersions: { [API_KEY]: 2 } } });

  test("opens read-only with a banner", async ({ page, backend }) => {
    await openGuide(page, "Rotate an API key");
    await expect(page.getByText("This guide was made with a newer version of GuidesYa. It is read-only.")).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Guide title" })).toBeDisabled();
    await expect(page.getByRole("textbox", { name: "Step title" })).toHaveAttribute("contenteditable", "false");
    await expect(page.getByRole("toolbar", { name: "Annotation tools" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Split" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Insert" })).toBeDisabled();
    // Typing changes nothing and nothing is saved.
    await page.getByRole("textbox", { name: "Step title" }).click();
    await page.keyboard.type("x");
    await expect(page.getByRole("textbox", { name: "Step title" })).toHaveText("Open Settings → API keys");
    expect(backend.saves).toHaveLength(0);
  });
});
