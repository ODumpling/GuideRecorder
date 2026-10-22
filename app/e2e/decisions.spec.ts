import { API_KEY, CALENDAR, expect, openGuide, outlineItems, saved, test } from "./fixtures";

// Decisions and section audiences (#22).

test("inserts a decision, points an option at a step and tags a section", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  const items = backend.guide(CALENDAR).items;
  const invitees = items.find((item) => item.type === "step" && item.title.includes("Add Invitees"))!;

  await outlineItems(page).filter({ hasText: "Click Calendar in the Dock" }).click();
  await page.getByRole("button", { name: "Insert" }).click();
  await page.getByRole("menuitem", { name: "Decision below" }).click();
  await expect(outlineItems(page)).toHaveCount(items.length + 1);

  const question = page.getByRole("textbox", { name: "Decision question" });
  await question.click();
  await page.keyboard.type("Is the event already open?");
  await page.getByLabel("Option 1 label").fill("Already open");
  await page.getByLabel("Option 2 goes to").selectOption(`step:${invitees.id}`);
  await saved(page);

  const decision = backend.lastSave(CALENDAR).items[2];
  expect(decision).toMatchObject({
    type: "decision",
    question: "Is the event already open?",
    options: [
      { label: "Already open", target: { kind: "next" } },
      { label: "No", target: { kind: "step", id: invitees.id } },
    ],
  });
  await expect(outlineItems(page).nth(2)).toContainText("Is the event already open?");

  // Another option, then removed again.
  await page.getByRole("button", { name: "Add option" }).click();
  await expect(page.getByLabel("Option 3 label")).toBeVisible();
  await page.getByRole("button", { name: "Remove option 3" }).click();
  await expect(page.getByLabel("Option 3 label")).toHaveCount(0);

  await outlineItems(page).filter({ hasText: "Invite people" }).click();
  const audiences = page.getByLabel("Section audiences");
  await audiences.fill("Organizer, guest, ORGANIZER");
  await audiences.press("Enter");
  await saved(page);
  const section = backend.lastSave(CALENDAR).items.find((item) => item.type === "section" && item.title === "Invite people");
  expect(section).toMatchObject({ audiences: ["Organizer", "guest"] });
  await expect(outlineItems(page).filter({ hasText: "Invite people" }).getByTestId("section-audiences")).toHaveText(
    "Organizer, guest",
  );

  // Undo takes the tags off again.
  await page.keyboard.press("ControlOrMeta+z");
  await expect(audiences).toHaveValue("");
});

test("exports for one audience", async ({ page, backend }) => {
  await openGuide(page, "Share a calendar event");
  await outlineItems(page).filter({ hasText: "Invite people" }).click();
  await page.getByLabel("Section audiences").fill("Organizer");
  await page.getByLabel("Section audiences").press("Enter");
  await saved(page);

  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export guide" });
  await dialog.getByRole("radio", { name: /Markdown/ }).click();
  const audience = dialog.getByLabel("Audience");
  await expect(audience).toHaveValue("");
  await audience.selectOption("Organizer");
  await expect.poll(() => backend.previews.at(-1)?.audience).toBe("Organizer");
  backend.dialog.folder = "/Users/me/Exports";
  await dialog.getByRole("button", { name: "Choose…" }).click();
  await dialog.getByRole("button", { name: "Export", exact: true }).click();
  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  expect(backend.exports.at(-1)).toMatchObject({ guideId: CALENDAR, format: "md", audience: "Organizer" });

  // Everyone again: no audience is sent.
  await dialog.getByRole("button", { name: "Export again" }).click();
  await audience.selectOption("");
  await dialog.getByLabel("Replace an earlier export there").check();
  await dialog.getByRole("button", { name: "Export", exact: true }).click();
  await expect(dialog.getByText(/Exported to/)).toBeVisible();
  expect(backend.exports.at(-1)?.audience).toBeUndefined();

  // A SCORM course can be made for one audience too.
  await dialog.getByRole("button", { name: "Export again" }).click();
  await dialog.getByRole("radio", { name: /SCORM course/ }).click();
  await audience.selectOption("Organizer");
  await expect.poll(() => backend.previews.at(-1)).toMatchObject({ format: "scorm", audience: "Organizer" });
});

test("guides without audiences offer no audience choice", async ({ page, backend }) => {
  await openGuide(page, "Rotate an API key");
  // Its decision shows in the outline.
  const decision = backend.guide(API_KEY).items.findIndex((item) => item.type === "decision");
  await expect(outlineItems(page).nth(decision)).toContainText("Is the key used in production?");
  await page.getByRole("button", { name: "Export…" }).click();
  const dialog = page.getByRole("dialog", { name: "Export guide" });
  await expect(dialog.getByLabel("Audience")).toHaveCount(0);
});
