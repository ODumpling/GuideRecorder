import type { Page } from "@playwright/test";
import type { Guide, Step } from "../src/lib/types";
import type { Backend } from "./backend";
import { API_KEY, emitEvents, expect, openGuide, outlineItems, record, saved, test } from "./fixtures";

// "Rotate an API key": steps 1, 2, 4 and 5 can be written; step 3 is edited.
const STEP_1 = "01M4WZCE2000000000000002R2";
const STEP_2 = "01M4WZCE2000000000000002R4";
const STEP_3 = "01M4WZCE2000000000000002R8";

const stepIn = (guide: Guide, id: string) => guide.items.find((item) => item.id === id) as Step;
const outlineItem = (page: Page, id: string) => page.locator(`[data-outline-item="${id}"]`);

test("Write with AI writes the steps and the guide details, but not edited steps", async ({ page, backend }) => {
  backend.apiKeyStored = true;
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Write with AI" }).click();

  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue("AI: Rotate an API key");
  await expect(outlineItem(page, STEP_1)).toContainText("AI step 1");
  await expect(outlineItem(page, STEP_2)).toContainText("AI step 2");
  await expect(outlineItem(page, STEP_3)).toContainText("Click Create key");
  await expect(page.getByRole("alert")).toHaveText("AI wrote 4 steps and the guide details. Kept your edits to 1 step.");
  await expect(page.getByRole("button", { name: "Write with AI" })).toBeVisible();
  // Saved like any change, but not an undo step.
  await saved(page);
  const last = backend.lastSave(API_KEY);
  expect(stepIn(last, STEP_1)).toMatchObject({ title: "AI step 1", aiState: { status: "ai", promptVersion: "step.v1" } });
  expect(last).toMatchObject({ summary: "An AI summary.", tags: ["ai"], aiState: { status: "ai" } });
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();
  expect(backend.aiGenerations).toEqual([{ guideId: API_KEY, runId: 1 }]);
});

test("a step edited during the run keeps the user's text", async ({ page, backend }) => {
  backend.apiKeyStored = true;
  backend.aiHold = true;
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Write with AI" }).click();
  await expect(page.getByRole("button", { name: "Stop" })).toBeVisible();
  await expect(outlineItem(page, STEP_2)).toContainText("Writing…");
  await expect(outlineItem(page, STEP_3)).not.toContainText("Writing…");

  await outlineItem(page, STEP_2).click();
  const title = page.getByRole("textbox", { name: "Step title" });
  await title.click();
  await page.keyboard.press("End");
  await page.keyboard.type(" now");
  await expect(title).toHaveText("Enter your password to confirm now");

  await emitEvents(page, backend.aiRelease());
  await expect(outlineItem(page, STEP_1)).toContainText("AI step 1");
  await expect(title).toHaveText("Enter your password to confirm now");
  await expect(outlineItem(page, STEP_2)).not.toContainText("Writing…");
  await expect(page.getByRole("alert")).toHaveText("AI wrote 3 steps and the guide details. Kept your edits to 2 steps.");
  await saved(page);
  expect(stepIn(backend.lastSave(API_KEY), STEP_2)).toMatchObject({
    title: "Enter your password to confirm now",
    aiState: { status: "edited" },
  });
});

test("Stop cancels the run and leaves the text", async ({ page, backend }) => {
  backend.apiKeyStored = true;
  backend.aiHold = true;
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Write with AI" }).click();
  await page.getByRole("button", { name: "Stop" }).click();

  await expect(page.getByRole("alert")).toHaveText("Stopped writing. Kept your edits to 1 step.");
  await expect(page.getByRole("button", { name: "Write with AI" })).toBeVisible();
  expect(backend.aiCancels).toEqual([1]);
  await expect(outlineItem(page, STEP_1)).toContainText("Open Settings → API keys");
  await expect(outlineItem(page, STEP_1)).not.toContainText("Writing…");
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue("Rotate an API key");
});

test("without a key, Write with AI says why", async ({ page, backend }) => {
  await openGuide(page, "Rotate an API key");
  await page.getByRole("button", { name: "Write with AI" }).click();
  await expect(page.getByRole("alert")).toHaveText("Add an API key in Settings to write with AI.");
  backend.apiKeyStored = true;
  backend.settings.ai.enabled = false;
  await page.getByRole("button", { name: "Write with AI" }).click();
  await expect(page.getByRole("alert")).toHaveText("AI is turned off in Settings.");
  expect(backend.aiGenerations).toEqual([]);
});

test.describe("after a recording", () => {
  const recordGuide = async (page: Page, backend: Backend) => {
    for (const p of Object.keys(backend.permissions)) backend.permissions[p] = "granted";
    await expect(page.getByRole("button", { name: /Rotate an API key/ })).toBeVisible();
    await record(page);
    await page.getByRole("group", { name: "Recording" }).getByRole("button", { name: "Stop recording" }).click();
    await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue(/Recording 2026-10-15/);
    return backend.recordings[0].guideId;
  };

  test("auto-run writes the new guide", async ({ page, backend }) => {
    backend.apiKeyStored = true;
    const id = await recordGuide(page, backend);
    await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue(/^AI: Recording/);
    await expect(outlineItems(page).filter({ hasText: "AI step 1" })).toHaveCount(1);
    expect(backend.aiGenerations).toEqual([{ guideId: id, runId: 1 }]);
  });

  test("auto-run does nothing, silently, without a key", async ({ page, backend }) => {
    await recordGuide(page, backend);
    await expect(page.getByRole("button", { name: "Write with AI" })).toBeVisible();
    await expect(page.getByRole("alert")).toHaveCount(0);
    expect(backend.calls.some((c) => c.cmd === "api_key_status")).toBe(true);
    expect(backend.calls.some((c) => c.cmd === "ai_generate")).toBe(false);
  });
});
