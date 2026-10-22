import type { Page } from "@playwright/test";
import type { Editor } from "@tiptap/core";
import type { Annotation, Step } from "@guiderecorder/guide-schema";
import type { Backend } from "./backend";
import { API_KEY, CALENDAR, expect, openGuide, outlineItems, saved, SCHEMA_VERSION, test } from "./fixtures";

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

test("drags a step to the end of the outline, and Esc cancels a drag", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  const before = backend.guide(CALENDAR).items.map((item) => item.id);
  const row = page.locator("li", { has: page.locator(`[data-outline-item="${before[2]}"]`) });
  const box = (await row.boundingBox())!;
  const last = (await page.locator(`[data-outline-item="${before.at(-1)}"]`).boundingBox())!;

  // Esc while dragging leaves the order as it was.
  await page.mouse.move(box.x + 20, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 20, last.y + last.height / 2, { steps: 5 });
  await page.keyboard.press("Escape");
  await page.mouse.up();
  await expect(outlineItems(page)).toHaveCount(before.length);
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();

  // Below the last item: the end.
  await page.mouse.move(box.x + 20, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 20, last.y + last.height / 2, { steps: 5 });
  await page.mouse.move(box.x + 20, last.y + last.height + 16, { steps: 3 });
  await page.mouse.up();
  await expect.poll(() => lastItems(backend).map((item) => item.id)).toEqual([...before.slice(0, 2), ...before.slice(3), before[2]]);
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

test.describe("the outline from the keyboard", () => {
  // Section, step, step, note, section, step, step, step, note.
  const [SECTION, STEP_1, STEP_2, , , , , , LAST] = [
    "01M4WT7MM000000000000002G1",
    "01M4WT7MM000000000000002G4",
    "01M4WT7MM000000000000002G7",
    "01M4WT7MM000000000000002G8",
    "01M4WT7MM000000000000002G9",
    "01M4WT7MM000000000000002GA",
    "01M4WT7MM000000000000002GC",
    "01M4WT7MM000000000000002GD",
    "01M4WT7MM000000000000002GE",
  ];
  const item = (page: Page, id: string) => page.locator(`[data-outline-item="${id}"]`);
  const announcement = (page: Page) => page.getByTestId("outline-announcement");
  const title = (page: Page) => page.getByRole("textbox", { name: "Step title" });

  test("selects with the arrows, reorders with Option and announces both", async ({ page, backend }) => {
    await openGuide(page, "Share a calendar event");
    await item(page, STEP_1).focus();
    await page.keyboard.press("ArrowDown");
    await expect(item(page, STEP_2)).toBeFocused();
    await expect(title(page)).toHaveText("Double-click the Team sync event");
    await expect(announcement(page)).toHaveText("Step 2 of 5 selected");

    await page.keyboard.press("Alt+ArrowUp");
    const before = backend.guide(CALENDAR).items.map((i) => i.id);
    await expect.poll(() => lastItems(backend).map((i) => i.id)).toEqual([before[0], STEP_2, STEP_1, ...before.slice(3)]);
    await expect(announcement(page)).toHaveText("Moved to position 2 of 9");
    await expect(item(page, STEP_2)).toBeFocused();
    await expect(item(page, STEP_2)).toHaveText(/^1/);

    await page.keyboard.press("End");
    await expect(item(page, LAST)).toBeFocused();
    await expect(item(page, LAST)).toHaveAttribute("aria-current", "true");
    await page.keyboard.press("Home");
    await expect(item(page, before[0])).toBeFocused();
  });

  test("Enter edits the step title and Esc goes back to the outline", async ({ page, backend: _ }) => {
    await openGuide(page, "Share a calendar event");
    await item(page, STEP_2).focus();
    await page.keyboard.press("Enter");
    await expect(title(page)).toBeFocused();
    await expect(title(page)).toHaveText("Double-click the Team sync event");
    await page.keyboard.press("Escape");
    await expect(item(page, STEP_2)).toBeFocused();
  });

  test("J, K and Cmd+D move around, but not while typing", async ({ page, backend: _ }) => {
    await openGuide(page, "Share a calendar event");
    await item(page, STEP_1).focus();
    await page.keyboard.press("j");
    await expect(title(page)).toHaveText("Double-click the Team sync event");
    await expect(item(page, STEP_2)).toBeFocused();
    await page.keyboard.press("ControlOrMeta+]");
    await expect(item(page, STEP_2)).not.toHaveAttribute("aria-current");
    await page.keyboard.press("k");
    await expect(item(page, STEP_2)).toHaveAttribute("aria-current", "true");
    await page.keyboard.press("ControlOrMeta+d");
    await expect(page.getByRole("button", { name: "Guide details", exact: true })).toHaveAttribute("aria-current", "true");
    // From the guide details, J goes to the first item.
    await page.keyboard.press("j");
    await expect(item(page, SECTION)).toHaveAttribute("aria-current", "true");

    // In a text field, J is a letter.
    await item(page, STEP_1).click();
    await title(page).click();
    await page.keyboard.press("End");
    await page.keyboard.press("j");
    await expect(title(page)).toHaveText("Click Calendar in the Dockj");
  });

  test("collapses a section to its step count", async ({ page, backend: _ }) => {
    await openGuide(page, "Share a calendar event");
    await expect(outlineItems(page)).toHaveCount(9);
    await page.getByRole("button", { name: "Collapse section" }).first().click();
    await expect(outlineItems(page)).toHaveCount(6);
    await expect(outlineItems(page).first()).toHaveText(/Open the event\s*2 steps/);
    await expect(outlineItems(page).first()).toHaveAttribute("aria-expanded", "false");
    // Folding moved the selection from its first step to the section.
    await expect(outlineItems(page).first()).toHaveAttribute("aria-current", "true");
    // Selecting a hidden step unfolds the section.
    await page.getByRole("button", { name: "Next item" }).click();
    await expect(outlineItems(page)).toHaveCount(9);
    await expect(title(page)).toHaveText("Click Calendar in the Dock");
    await page.getByRole("button", { name: "Collapse section" }).first().click();
    await page.getByRole("button", { name: "Expand section" }).click();
    await expect(outlineItems(page)).toHaveCount(9);
  });

  test("selects a range with Shift-click, then merges or deletes it as one step", async ({ page, backend }) => {
    await openGuide(page, "Share a calendar event");
    const count = backend.guide(CALENDAR).items.length;
    await item(page, STEP_1).click();
    await item(page, STEP_2).click({ modifiers: ["Shift"] });
    const bar = page.getByRole("toolbar", { name: "Selected items" });
    await expect(bar).toContainText("2 items selected");
    await bar.getByRole("button", { name: "Merge 2 steps" }).click();
    await expect.poll(() => lastItems(backend).length).toBe(count - 1);
    await expect(bar).toHaveCount(0);

    await item(page, STEP_1).click();
    await page.keyboard.press("Shift+ArrowDown");
    await expect(bar).toContainText("2 items selected");
    // The note after the merged step can't merge.
    await expect(bar.getByRole("button", { name: /Merge/ })).toHaveCount(0);
    await page.keyboard.press("Delete");
    await expect.poll(() => lastItems(backend).length).toBe(count - 3);
    await page.keyboard.press("ControlOrMeta+z");
    await expect(outlineItems(page)).toHaveCount(count - 1);
  });
});

test.describe("a guide from a newer GuideRecorder", () => {
  test.use({ backendOptions: { schemaVersions: { [API_KEY]: SCHEMA_VERSION + 1 } } });

  test("opens read-only with a banner", async ({ page, backend }) => {
    await openGuide(page, "Rotate an API key");
    await expect(page.getByText("This guide was made with a newer version of GuideRecorder. It is read-only.")).toBeVisible();
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

test("shows a step's web page as a link that opens in the browser", async ({ page, backend }) => {
  const url = "https://calendar.example.com/events/42?view=week";
  backend.editStep(CALENDAR, "01M4WT7MM000000000000002G4", (s) => {
    s.url = url;
  });
  await openGuide(page, "Share a calendar event");
  const link = page.getByRole("link", { name: "calendar.example.com/events/42?view=week" });
  await expect(link).toHaveAttribute("href", url);
  await link.click();
  await expect.poll(() => backend.openedUrls).toEqual([url]);
  // In the browser, not in the app's own window.
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue("Share a calendar event");
});

test("never links to a step's page that is not a web address", async ({ page, backend }) => {
  backend.editStep(CALENDAR, "01M4WT7MM000000000000002G4", (s) => {
    s.url = "javascript:alert(1)";
  });
  await openGuide(page, "Share a calendar event");
  await expect(page.getByRole("textbox", { name: "Step title" })).toHaveText("Click Calendar in the Dock");
  await expect(page.getByText("Web page")).toHaveCount(0);
});
