import type { Page } from "@playwright/test";
import { API_KEY, CALENDAR, expect, openGuide, test } from "./fixtures";

/** Calendar steps: the first has an after frame, the second doesn't. */
const WITH_AFTER = "01M4WT7MM000000000000002G4";
const WITHOUT_AFTER = "01M4WT7MM000000000000002G7";

const before = (page: Page) => page.getByRole("img", { name: "Step screenshot with annotations" });
const after = (page: Page) => page.getByRole("img", { name: "Step screenshot after the action" });
const frameSwitch = (page: Page) => page.getByRole("group", { name: "Screenshot" });
const button = (page: Page, name: "Before" | "After") => frameSwitch(page).getByRole("button", { name, exact: true });
const note = (page: Page) => page.getByText("Annotations apply to the Before screenshot.");

test("shows a step's after frame without annotations", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  await expect(before(page)).toBeVisible();
  await expect(button(page, "Before")).toHaveAttribute("aria-pressed", "true");
  await expect(button(page, "After")).toHaveAttribute("aria-pressed", "false");
  const shapes = await before(page).locator("*").count();
  expect(shapes).toBeGreaterThan(1);

  await button(page, "After").click();
  await expect(after(page)).toBeVisible();
  await expect(before(page)).toHaveCount(0);
  await expect(after(page).locator("image")).toHaveAttribute("href", /^blob:/);
  // The whole 64×40 frame: this step has no recorded crop position.
  await expect(after(page)).toHaveAttribute("viewBox", "0 0 64 40");
  expect(backend.calls.some((c) => (c.args as { path?: string }).path === `steps/${WITH_AFTER}.after.webp`)).toBe(true);
  await expect(after(page).locator("image")).toHaveCount(1);
  await expect(after(page).locator("*")).toHaveCount(1);
  await expect(note(page)).toBeVisible();
  for (const tool of ["Select", "Arrow", "Box", "Blur", "Crop"]) {
    await expect(page.getByRole("button", { name: tool, exact: true })).toHaveCount(0);
  }
  await expect(button(page, "After")).toHaveAttribute("aria-pressed", "true");

  // Keyboard back to Before: the annotations and tools return.
  await button(page, "Before").focus();
  await page.keyboard.press("Enter");
  await expect(before(page)).toBeVisible();
  await expect(before(page).locator("*")).toHaveCount(shapes);
  await expect(page.getByRole("button", { name: "Arrow", exact: true })).toBeVisible();
  await expect(note(page)).toHaveCount(0);

  // View-only: nothing saved, nothing to undo.
  expect(backend.saves).toHaveLength(0);
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
});

test("hides the switch for a step without an after frame, and resets per step", async ({ page, backend: _ }) => {
  await openGuide(page, "Share a calendar event");
  await button(page, "After").click();
  await expect(after(page)).toBeVisible();

  await page.locator(`[data-outline-item="${WITHOUT_AFTER}"]`).click();
  await expect(page.getByRole("textbox", { name: "Step title" })).toHaveText("Double-click the Team sync event");
  await expect(frameSwitch(page)).toHaveCount(0);
  await expect(before(page)).toBeVisible();

  await page.locator(`[data-outline-item="${WITH_AFTER}"]`).click();
  await expect(button(page, "Before")).toHaveAttribute("aria-pressed", "true");
  await expect(before(page)).toBeVisible();
  await expect(after(page)).toHaveCount(0);
});

test.describe("framing", () => {
  test("shows the click crop's region of the after frame", async ({ page, backend }) => {
    const cropRect = { x: 16, y: 10, width: 32, height: 20 };
    backend.editStep(CALENDAR, WITH_AFTER, (s) => {
      s.image.cropRect = cropRect;
    });
    await openGuide(page, "Share a calendar event");
    await expect(before(page)).toHaveAttribute("viewBox", "16 10 32 20");
    await button(page, "After").click();
    await expect(after(page)).toHaveAttribute("viewBox", "16 10 32 20");

    // With the full screenshot shown, the whole frame.
    await button(page, "Before").click();
    await page.getByRole("button", { name: "Full", exact: true }).click();
    await button(page, "After").click();
    await expect(after(page)).toHaveAttribute("viewBox", "0 0 64 40");
  });

  test("shows the whole after frame when its size differs from the screenshot's", async ({ page, backend }) => {
    // The crop image (32×20) stands in for an after frame of another size.
    backend.editStep(CALENDAR, WITH_AFTER, (s) => {
      s.image.cropRect = { x: 16, y: 10, width: 32, height: 20 };
      s.image.after = `steps/${WITH_AFTER}.crop.webp`;
    });
    await openGuide(page, "Share a calendar event");
    await button(page, "After").click();
    await expect(after(page)).toHaveAttribute("viewBox", "0 0 32 20");
  });
});

test("an unreadable after frame shows the image error; Before is unaffected", async ({ page, backend }) => {
  backend.removeFile(CALENDAR, `steps/${WITH_AFTER}.after.webp`);
  await openGuide(page, "Share a calendar event");
  await button(page, "After").click();
  await expect(page.getByText(`no image steps/${WITH_AFTER}.after.webp`)).toBeVisible();
  await button(page, "Before").click();
  await expect(before(page).locator("image")).toHaveAttribute("href", /^blob:/);
});

test("an undecodable after frame shows an error, not endless loading", async ({ page, backend }) => {
  backend.setFile(CALENDAR, `steps/${WITH_AFTER}.after.webp`, Buffer.from("not a webp"));
  await openGuide(page, "Share a calendar event");
  await button(page, "After").click();
  await expect(page.getByText("The after screenshot can't be shown: the image is damaged.")).toBeVisible();
  await expect(page.getByText("Loading image…")).toHaveCount(0);
});

test("handles keep their size on screen after a round trip through After and a resize", async ({ page, backend: _ }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await openGuide(page, "Share a calendar event");
  await button(page, "After").click();
  await expect(after(page)).toBeVisible();
  await button(page, "Before").click();
  await expect(before(page).locator("image")).toHaveAttribute("href", /^blob:/);

  await page.setViewportSize({ width: 1400, height: 450 });
  // Select the box (12,6 16×10 in the 64×40 screenshot) by its middle.
  const box = (await before(page).boundingBox())!;
  const scale = Math.min(box.width / 64, box.height / 40);
  const left = box.x + (box.width - 64 * scale) / 2;
  const top = box.y + (box.height - 40 * scale) / 2;
  await page.mouse.click(left + 20 * scale, top + 11 * scale);
  const handle = before(page).locator('rect[fill="white"]').first();
  await expect(handle).toBeVisible();
  // 9 px, plus its 1.5 px stroke.
  await expect.poll(async () => (await handle.boundingBox())!.width).toBeCloseTo(10.5, 0);
});

test.describe("a read-only guide", () => {
  test.use({ backendOptions: { schemaVersions: { [API_KEY]: 2 } } });

  test("still has the switch", async ({ page, backend: _ }) => {
    await openGuide(page, "Rotate an API key");
    await page.locator('[data-outline-item="01M4WZCE2000000000000002R8"]').click();
    await button(page, "After").click();
    await expect(after(page)).toHaveAttribute("viewBox", "0 0 64 40");
    await expect(page.getByRole("toolbar", { name: "Annotation tools" })).toHaveCount(0);
  });
});
