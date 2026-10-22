import type { Page } from "@playwright/test";
import type { Guide, Step } from "@guiderecorder/guide-schema";
import { API_KEY, emitEvents, expect, openGuide, saved, test } from "./fixtures";
import { AI_ENABLED } from "../src/lib/features";

test.skip(!AI_ENABLED, "AI writing is off for now");

// "Rotate an API key": step 3 is edited and shows its full screenshot.
const STEP_1 = "01M4WZCE2000000000000002R2";
const STEP_3 = "01M4WZCE2000000000000002R8";
const STEP_3_TITLE = "Click Create key and copy the new key";

const stepIn = (guide: Guide, id: string) => guide.items.find((item) => item.id === id) as Step;
const outlineItem = (page: Page, id: string) => page.locator(`[data-outline-item="${id}"]`);
const status = (page: Page, id: string) => outlineItem(page, id).getByTestId("step-status");
const stepTitle = (page: Page) => page.getByRole("textbox", { name: "Step title" });
const undo = (page: Page) => page.getByRole("button", { name: "Undo" }).click();

const aiActions = (page: Page) => page.getByRole("toolbar", { name: "AI actions" });

async function aiAction(page: Page, name: string) {
  await aiActions(page).getByRole("button", { name, exact: true }).click();
}

test.beforeEach(async ({ page, backend }) => {
  backend.apiKeyStored = true;
  await openGuide(page, "Rotate an API key");
});

test("Regenerate replaces an edited step, and Undo brings the edit back", async ({ page, backend }) => {
  await outlineItem(page, STEP_3).click();
  await expect(status(page, STEP_3)).toHaveText("Text: Edited");
  await aiAction(page, "Regenerate");

  await expect(stepTitle(page)).toHaveText("AI step 3");
  await expect(status(page, STEP_3)).toHaveText("Text: AI");
  expect(backend.aiGenerations.at(-1)).toEqual({
    guideId: API_KEY,
    runId: 1,
    options: { stepIds: [STEP_3], guidePass: false, force: true },
  });
  // Only that step; the guide details stay.
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue("Rotate an API key");
  await expect(page.getByRole("alert")).toHaveCount(0);

  await undo(page);
  await expect(stepTitle(page)).toHaveText(STEP_3_TITLE);
  await expect(status(page, STEP_3)).toHaveText("Text: Edited");
  await saved(page);
  expect(stepIn(backend.lastSave(API_KEY), STEP_3)).toMatchObject({
    title: "Click **Create key** and copy the new key",
    aiState: { status: "edited" },
  });
});

test("Rewrite → Shorter rewrites the step as one undo step", async ({ page, backend }) => {
  await outlineItem(page, STEP_1).click();
  await aiAction(page, "Shorter");
  await expect(stepTitle(page)).toHaveText("Rewritten shorter 1");
  expect(backend.aiRewrites).toEqual([{ guideId: API_KEY, stepId: STEP_1, style: { kind: "shorter" } }]);
  await saved(page);
  expect(stepIn(backend.lastSave(API_KEY), STEP_1)).toMatchObject({
    title: "Rewritten shorter 1",
    description: "Rewritten shorter for step 1.",
    aiState: { status: "ai", promptVersion: "rewrite.v1" },
  });
  await undo(page);
  await expect(stepTitle(page)).toHaveText("Open Settings → API keys");
});

test("Rewrite → Custom… takes an instruction", async ({ page, backend }) => {
  await outlineItem(page, STEP_3).click();
  await aiAction(page, "Custom…");
  const instruction = page.getByRole("textbox", { name: "Rewrite instruction" });
  await expect(instruction).toBeFocused();
  const submit = page.getByRole("form", { name: "Custom rewrite" }).getByRole("button", { name: "Rewrite" });
  await expect(submit).toBeDisabled();
  await instruction.fill("   ");
  await expect(submit).toBeDisabled();
  await instruction.fill("  Use plain words ");
  await instruction.press("Enter");

  await expect(stepTitle(page)).toHaveText("Rewritten custom (Use plain words) 3");
  await expect(page.getByRole("form", { name: "Custom rewrite" })).toHaveCount(0);
  expect(backend.aiRewrites.at(-1)!.style).toEqual({ kind: "custom", instruction: "Use plain words" });
  await undo(page);
  await expect(stepTitle(page)).toHaveText(STEP_3_TITLE);
  await expect(status(page, STEP_3)).toHaveText("Text: Edited");
});

test("a rewrite without a key says why", async ({ page, backend }) => {
  backend.apiKeyStored = false;
  await outlineItem(page, STEP_1).click();
  await aiAction(page, "For experts");
  await expect(page.getByRole("alert")).toHaveText("Add an API key in Settings to write with AI.");
  await expect(stepTitle(page)).toHaveText("Open Settings → API keys");
});

test("blur suggestions are blurred or kept in the Privacy tab, undoably", async ({ page, backend }) => {
  backend.aiSuggestions[STEP_3] = [
    { rect: { x: 40, y: 30, width: 200, height: 40 }, reason: "an API key" },
    { rect: { x: 300, y: 200, width: 120, height: 30 }, reason: "an email address" },
  ];
  const blurs = () => stepIn(backend.lastSave(API_KEY), STEP_3).annotations.filter((a) => a.type === "blur");
  await outlineItem(page, STEP_3).click();
  await aiAction(page, "Regenerate");
  const tab = page.getByRole("tab", { name: /Privacy/ });
  await expect(tab).toHaveText("Privacy2");
  await tab.click();
  const list = page.getByRole("list", { name: "Blur suggestions" });
  await expect(list.getByRole("listitem")).toHaveCount(2);
  await expect(page.locator("[data-suggestion]")).toHaveCount(2);
  await saved(page);
  const before = blurs().length;

  // Pointing at a finding highlights its area.
  await list.getByRole("listitem").nth(1).hover();
  await expect(page.locator('[data-suggestion="1"] rect').first()).toHaveAttribute("fill-opacity", "0.25");

  await page.getByRole("button", { name: "Blur it, suggestion 1" }).click();
  await expect(list.getByRole("listitem")).toHaveCount(1);
  await expect(list).toContainText("An email address");
  await expect(tab).toHaveText("Privacy1");
  await saved(page);
  expect(blurs()).toHaveLength(before + 1);
  expect(blurs().at(-1)!.geometry).toEqual({ x: 40, y: 30, width: 200, height: 40 });

  await page.getByRole("button", { name: "Keep, suggestion 1" }).click();
  await expect(list).toHaveCount(0);
  await expect(page.locator("[data-suggestion]")).toHaveCount(0);
  await saved(page);
  expect(blurs()).toHaveLength(before + 1);
  expect(stepIn(backend.lastSave(API_KEY), STEP_3).aiState.redactionSuggestions!.map((s) => s.review)).toEqual([
    "blurred",
    "kept",
  ]);
  // The whole guide shows what was decided.
  await expect(page.getByRole("list", { name: "All findings" })).toContainText("Step 3 · blurred");
  await expect(page.getByRole("list", { name: "All findings" })).toContainText("Step 3 · kept");

  await undo(page);
  await expect(list.getByRole("listitem")).toHaveCount(1);
  await undo(page);
  await expect(list.getByRole("listitem")).toHaveCount(2);
  await saved(page);
  expect(blurs()).toHaveLength(before);
});

test("Regenerate guide details replaces edited fields as one undo step", async ({ page, backend }) => {
  const title = page.getByRole("textbox", { name: "Guide title" });
  await title.fill("Rotate a key");
  await page.getByRole("button", { name: "Regenerate guide details" }).click();
  await expect(title).toHaveValue("AI: Rotate a key");
  expect(backend.aiGenerations.at(-1)!.options).toEqual({ stepIds: [], guidePass: true, force: true });
  // No step was written.
  await expect(outlineItem(page, STEP_1)).toContainText("Open Settings → API keys");
  await saved(page);
  expect(backend.lastSave(API_KEY)).toMatchObject({ summary: "An AI summary.", aiState: { status: "ai" } });

  await undo(page);
  await expect(title).toHaveValue("Rotate a key");
  await saved(page);
  expect(backend.lastSave(API_KEY)).toMatchObject({ title: "Rotate a key", aiState: { status: "edited" } });
});

test("explicit actions wait for a background run", async ({ page, backend }) => {
  backend.aiHold = true;
  await page.getByRole("button", { name: "Write with AI" }).click();
  await expect(page.getByRole("button", { name: "Stop" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Regenerate guide details" })).toBeDisabled();
  await outlineItem(page, STEP_1).click();
  await expect(aiActions(page)).toContainText("AI is writing this guide. Wait for it to finish, or stop it.");
  await expect(aiActions(page).getByRole("button", { name: "Regenerate", exact: true })).toBeDisabled();
  await expect(aiActions(page).getByRole("button", { name: "Shorter", exact: true })).toBeDisabled();

  await emitEvents(page, backend.aiRelease());
  await expect(page.getByRole("button", { name: "Write with AI" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Regenerate guide details" })).toBeEnabled();
  await aiAction(page, "Shorter");
  await expect(stepTitle(page)).toHaveText("Rewritten shorter 1");
});
