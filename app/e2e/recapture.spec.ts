import type { Page } from "@playwright/test";
import type { Annotation, Guide, Step } from "@guiderecorder/guide-schema";
import { CALENDAR, expect, openGuide, outlineItems, saved, test } from "./fixtures";

// "Share a calendar event": step 1 clicks at (30, 36) and has two annotations.
const STEP_1 = "01M4WT7MM000000000000002G4";
const STEP_3 = "01M4WT7MM000000000000002GA";

const stepIn = (guide: Guide, id: string) => guide.items.find((item) => item.id === id) as Step;
const outlineItem = (page: Page, id: string) => page.locator(`[data-outline-item="${id}"]`);
const undo = (page: Page) => page.getByRole("button", { name: "Undo" }).click();
const controls = (page: Page) => page.getByRole("group", { name: "Recording" });

/** Opens the selected step's ⋯ menu and picks `item`. */
async function stepAction(page: Page, item: string) {
  await page.getByRole("button", { name: "More step actions" }).click();
  await page.getByRole("menuitem", { name: item }).click();
}

/** Where an annotation sits, whatever its shape. */
function origin(annotation: Annotation): { x: number; y: number } {
  const g = annotation.geometry as { x?: number; y?: number; from?: { x: number; y: number } };
  return g.from ?? { x: g.x!, y: g.y! };
}

test.beforeEach(async ({ page, backend }) => {
  for (const p of Object.keys(backend.permissions)) backend.permissions[p] = "granted";
  await openGuide(page, "Share a calendar event");
});

test("Re-capture replaces a step's screenshot and click, moves its annotations, and undoes", async ({ page, backend }) => {
  const before = structuredClone(stepIn(backend.guide(CALENDAR), STEP_1));
  await outlineItem(page, STEP_1).click();
  await stepAction(page, "Re-capture step");

  await expect(page.getByText(/Re-capturing a step: the next click/)).toBeVisible();
  // Into this guide, on the area the settings give (the fixture doesn't remember one).
  expect(backend.calls.find((c) => c.cmd === "start_recording")?.args).toEqual({
    title: null,
    into: { mode: "replace", guideId: CALENDAR, stepId: STEP_1 },
  });
  await controls(page).getByRole("button", { name: "Stop recording" }).click();

  // The click moved from (30, 36) to (60, 50): the step has annotations, so the user chooses.
  const dialog = page.getByRole("alertdialog", { name: "Step re-captured" });
  await expect(dialog).toContainText("The click moved 30, 14 pixels");
  await dialog.getByRole("button", { name: "Move annotations" }).click();
  await expect(dialog).toBeHidden();

  await saved(page);
  const after = stepIn(backend.lastSave(CALENDAR), STEP_1);
  const recorded = backend.recordings.at(-1)!.recorded!.steps[0];
  expect(after).toMatchObject({
    id: STEP_1,
    title: before.title,
    description: before.description,
    point: { x: 60, y: 50 },
    element: recorded.element,
    image: { full: recorded.image.full, show: "full" },
  });
  expect(after.annotations.map(origin)).toEqual(
    before.annotations.map((a) => ({ x: origin(a).x + 30, y: origin(a).y + 14 })),
  );
  // Replaced in place: no step added.
  await expect(outlineItems(page)).toHaveCount(backend.lastSave(CALENDAR).items.length);
  expect(backend.lastSave(CALENDAR).items).toHaveLength(9);

  await undo(page);
  await saved(page);
  expect(stepIn(backend.lastSave(CALENDAR), STEP_1)).toEqual(before);
});

test("Re-capture can clear the annotations", async ({ page, backend }) => {
  await outlineItem(page, STEP_1).click();
  await stepAction(page, "Re-capture step");
  await controls(page).getByRole("button", { name: "Stop recording" }).click();
  await page.getByRole("alertdialog", { name: "Step re-captured" }).getByRole("button", { name: "Clear annotations" }).click();
  await saved(page);
  expect(stepIn(backend.lastSave(CALENDAR), STEP_1).annotations).toEqual([]);
});

test("Insert recorded steps here adds the recording after the step, as one undo step", async ({ page, backend }) => {
  const items = backend.guide(CALENDAR).items.map((item) => item.id);
  await outlineItem(page, STEP_3).click();
  await stepAction(page, "Insert recorded steps here");
  await expect(page.getByText(/Recording steps into this guide/)).toBeVisible();
  expect(backend.calls.find((c) => c.cmd === "start_recording")?.args).toMatchObject({
    into: { mode: "insert", guideId: CALENDAR, afterId: STEP_3 },
  });
  await controls(page).getByRole("button", { name: "Stop recording" }).click();

  const added = backend.recordings.at(-1)!.recorded!.steps.map((s) => s.id);
  await expect(outlineItems(page)).toHaveCount(items.length + 2);
  await saved(page);
  const at = items.indexOf(STEP_3) + 1;
  expect(backend.lastSave(CALENDAR).items.map((item) => item.id)).toEqual([
    ...items.slice(0, at),
    ...added,
    ...items.slice(at),
  ]);
  // The new steps' screenshots load from the guide.
  await expect(outlineItem(page, added[0]).locator("img")).toBeVisible();

  await undo(page);
  await expect(outlineItems(page)).toHaveCount(items.length);
});
