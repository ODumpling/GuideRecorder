import { API_KEY, CALENDAR, expect, openGuide, outlineItems, saved, test } from "./fixtures";

test("inserts chosen steps from another guide, undoably", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  const before = await outlineItems(page).count();
  await outlineItems(page).first().click();

  await page.getByRole("button", { name: "Insert" }).click();
  await page.getByRole("menuitem", { name: "Steps from another guide…" }).click();
  const dialog = page.getByRole("dialog", { name: "Insert steps from another guide" });
  const insert = dialog.getByRole("button", { name: /^Insert/ });
  await expect(insert).toBeDisabled();
  // The open guide is not offered.
  await expect(dialog.getByRole("combobox", { name: "Guide" }).getByRole("option", { name: "Share a calendar event" })).toHaveCount(0);
  await dialog.getByRole("combobox", { name: "Guide" }).selectOption({ label: "Rotate an API key" });

  const steps = dialog.getByRole("list", { name: "Steps" }).getByRole("checkbox");
  const count = await steps.count();
  expect(count).toBeGreaterThan(2);
  await expect(insert).toHaveText(`Insert ${count} steps`);
  await dialog.getByLabel(`All ${count} steps`).uncheck();
  await expect(insert).toBeDisabled();
  await steps.nth(0).check();
  await steps.nth(2).check();
  await insert.click();
  await expect(dialog).toBeHidden();

  await expect(outlineItems(page)).toHaveCount(before + 2);
  const source = backend.guide(API_KEY).items.filter((i) => i.type === "step");
  expect(backend.imports).toEqual([{ guideId: CALENDAR, sourceId: API_KEY, stepIds: [source[0]!.id, source[2]!.id] }]);
  await saved(page);
  const items = backend.saves.at(-1)!.items;
  const added = items.filter((i) => i.type === "step" && i.id.startsWith("01JS"));
  expect(added.map((s) => s.type === "step" && s.title)).toEqual([source[0]!.title, source[2]!.title]);

  // One undo takes both out.
  await page.keyboard.press("ControlOrMeta+z");
  await expect(outlineItems(page)).toHaveCount(before);
});
