// The SCORM runtime of an exported SCORM package, in a real browser: the
// SCO page (core's golden for "Rotate an API key", with a two-question quiz
// and the quiz-passed rule) in a frame of a mocked LMS that records every
// SCORM 1.2 API call. Not the app: nothing here uses the mocked backend.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, test, type Page } from "@playwright/test";

const GOLDEN = resolve(
  import.meta.dirname,
  "../../crates/guiderecorder-core/tests/golden/scorm/rotate-an-api-key/guide.html.golden",
);
const ORIGIN = "http://lms.test";

/** An LMS page with the SCORM 1.2 `API`, its data seeded from `?seed=`, framing the SCO. */
const LMS = `<!doctype html><title>LMS</title>
<script>
  window.calls = [];
  window.data = JSON.parse(new URLSearchParams(location.search).get("seed") || "{}");
  window.API = {
    LMSInitialize: () => (calls.push("LMSInitialize"), "true"),
    LMSFinish: () => (calls.push("LMSFinish"), "true"),
    LMSGetValue: (name) => (name in data ? data[name] : ""),
    LMSSetValue: (name, value) => (calls.push(name + "=" + value), (data[name] = value), "true"),
    LMSCommit: () => (calls.push("LMSCommit"), "true"),
    LMSGetLastError: () => "0",
    LMSGetErrorString: () => "",
    LMSGetDiagnostic: () => "",
  };
</script>
<iframe id="sco" src="course/guide.html" style="width:1100px;height:900px"></iframe>`;

/** Serves the LMS and the SCO page (with `completion` as its rule). */
async function serve(page: Page, completion = "quiz-passed") {
  const sco = readFileSync(GOLDEN, "utf8").replace('data-completion="quiz-passed"', `data-completion="${completion}"`);
  await page.route(`${ORIGIN}/**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/course/guide.html") return route.fulfill({ contentType: "text/html; charset=utf-8", body: sco });
    if (path === "/lms.html") return route.fulfill({ contentType: "text/html; charset=utf-8", body: LMS });
    return route.fulfill({ status: 404 });
  });
}

const calls = (page: Page) => page.evaluate(() => (window as unknown as { calls: string[] }).calls);
const data = (page: Page) => page.evaluate(() => (window as unknown as { data: Record<string, string> }).data);

test("reports progress, the quiz score and passing to the LMS", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await serve(page);
  await page.goto(`${ORIGIN}/lms.html`);
  const sco = page.frameLocator("#sco");
  await expect(sco.getByRole("heading", { name: "Before you start" })).toBeVisible();
  await expect.poll(() => calls(page)).toEqual(
    expect.arrayContaining(["LMSInitialize", "cmi.core.lesson_status=incomplete", "cmi.core.lesson_location=wt-slide-0"]),
  );

  // The last step alone doesn't complete a course passed by its quiz.
  await sco.locator('.wt-list a[data-slide="5"]').click();
  await expect.poll(async () => (await data(page))["cmi.core.lesson_location"]).toBe("wt-slide-5");
  expect((await data(page))["cmi.core.lesson_status"]).toBe("incomplete");

  await sco.locator(".wt-controls [data-next]").click();
  const quiz = sco.locator("#scorm-quiz");
  await expect(quiz.getByRole("heading", { name: "Quiz" })).toBeVisible();
  const check = quiz.getByRole("button", { name: "Check answers" });
  await check.click();
  await expect(quiz.getByRole("status")).toHaveText("Answer every question, then check again.");

  await quiz.getByLabel("The last step").check();
  await quiz.getByLabel("Yes, with Back or ←").check();
  // Arrow keys move between choices, not slides.
  await quiz.getByLabel("Yes, with Back or ←").press("ArrowLeft");
  await expect(sco.getByRole("heading", { name: "Done" })).toBeVisible();
  await quiz.getByLabel("Yes, with Back or ←").check();
  await check.click();
  await expect(quiz.getByRole("status")).toHaveText(
    "Not passed: 50% correct, 80% needed. Change your answers and check again.",
  );
  let stored = await data(page);
  expect(stored["cmi.core.score.raw"]).toBe("50");
  expect(stored["cmi.core.score.min"]).toBe("0");
  expect(stored["cmi.core.score.max"]).toBe("100");
  expect(stored["cmi.core.lesson_status"]).toBe("failed");

  await quiz.getByLabel("The first step").check();
  await check.click();
  await expect(quiz.getByRole("status")).toHaveText("Passed: 100% correct.");
  stored = await data(page);
  expect(stored["cmi.core.score.raw"]).toBe("100");
  expect(stored["cmi.core.lesson_status"]).toBe("passed");

  // Leaving ends the session once: its time, no suspend, a commit, LMSFinish.
  await page.evaluate(() => document.getElementById("sco")!.remove());
  const log = await calls(page);
  expect(log.filter((c) => c === "LMSFinish")).toHaveLength(1);
  expect(log.at(-1)).toBe("LMSFinish");
  stored = await data(page);
  expect(stored["cmi.core.exit"]).toBe("");
  expect(stored["cmi.core.session_time"]).toMatch(/^\d{2,4}:\d{2}:\d{2}\.\d{2}$/);
  expect(errors).toEqual([]);
});

test("completes on the last step and resumes at the bookmark", async ({ page }) => {
  await serve(page, "last-step");
  const seed = JSON.stringify({ "cmi.core.lesson_status": "incomplete", "cmi.core.lesson_location": "wt-slide-4" });
  await page.goto(`${ORIGIN}/lms.html?seed=${encodeURIComponent(seed)}`);
  const sco = page.frameLocator("#sco");
  // Back where the learner left off.
  await expect(sco.locator("#wt-slide-4")).toBeVisible();
  await expect(sco.locator("#wt-slide-0")).toBeHidden();
  expect((await data(page))["cmi.core.lesson_status"]).toBe("incomplete");

  await sco.locator(".wt-controls [data-next]").click();
  await expect.poll(async () => (await data(page))["cmi.core.lesson_status"]).toBe("completed");
  // Going back keeps it completed.
  await sco.locator(".wt-controls [data-back]").click();
  await expect.poll(async () => (await data(page))["cmi.core.lesson_location"]).toBe("wt-slide-4");
  expect((await data(page))["cmi.core.lesson_status"]).toBe("completed");
});

test("works on its own, without an LMS", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await serve(page);
  await page.goto(`${ORIGIN}/course/guide.html#wt-slide-6`);
  const quiz = page.locator("#scorm-quiz");
  await quiz.getByLabel("The first step").check();
  await quiz.getByLabel("Yes, with Back or ←").check();
  await quiz.getByRole("button", { name: "Check answers" }).click();
  await expect(quiz.getByRole("status")).toHaveText("Passed: 100% correct.");
  expect(errors).toEqual([]);
});
