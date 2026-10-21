// The SCORM 1.2 runtime of SCORM exports (core
// `export/templates/scorm.js`), run in Node against a mocked LMS API. The
// e2e test `scorm.spec.ts` runs it in an exported page.
import { describe, expect, it } from "vitest";
import source from "../../../crates/guiderecorder-core/src/export/templates/scorm.js?raw";

interface Api {
  LMSInitialize(arg: string): string;
  LMSFinish(arg: string): string;
  LMSGetValue(name: string): string;
  LMSSetValue(name: string, value: string): string;
  LMSCommit(arg: string): string;
}

interface Tracker {
  status: string;
  begin(): string;
  visit(bookmark: string, end: boolean, trail?: string): void;
  trail(): string;
  answer(right: number, total: number): { score: number; passed: boolean };
  finish(ms: number): void;
}

interface Scorm {
  findApi(win: unknown): Api | null;
  sessionTime(ms: number): string;
  Tracker: new (api: Api | null, completion: string | null, mastery: number) => Tracker;
}

/** The script as a module: it exports its parts instead of starting. */
function load(): Scorm {
  const module = { exports: {} as unknown };
  new Function("module", source)(module);
  return module.exports as Scorm;
}

const scorm = load();

/** A SCORM 1.2 LMS that records every call; `data` is what it has stored. */
function lms(data: Record<string, string> = {}, initialize = "true") {
  const calls: string[] = [];
  const api: Api = {
    LMSInitialize: () => (calls.push("initialize"), initialize),
    LMSFinish: () => (calls.push("finish"), "true"),
    LMSGetValue: (name) => data[name] ?? "",
    LMSSetValue: (name, value) => {
      calls.push(`${name}=${value}`);
      data[name] = value;
      return "true";
    },
    LMSCommit: () => (calls.push("commit"), "true"),
  };
  return { api, calls, data };
}

describe("findApi", () => {
  it("finds the API on the window, a parent or the opener", () => {
    const { api } = lms();
    const top: Record<string, unknown> = { API: api };
    top.parent = top;
    const middle = { parent: top };
    const frame = { parent: middle, opener: null };
    expect(scorm.findApi(frame)).toBe(api);
    expect(scorm.findApi(top)).toBe(api);

    const opener: Record<string, unknown> = { API: api };
    opener.parent = opener;
    const popup: Record<string, unknown> = { opener };
    popup.parent = popup;
    expect(scorm.findApi(popup)).toBe(api);
  });

  it("skips frames of another origin and finds nothing without an LMS", () => {
    const { api } = lms();
    const top: Record<string, unknown> = { API: api };
    top.parent = top;
    const foreign = {
      parent: top,
      get API(): unknown {
        throw new Error("SecurityError");
      },
    };
    expect(scorm.findApi({ parent: foreign })).toBe(api);

    const alone: Record<string, unknown> = { opener: null };
    alone.parent = alone;
    expect(scorm.findApi(alone)).toBeNull();
    // Something called API that isn't one.
    const fake: Record<string, unknown> = { API: {} };
    fake.parent = fake;
    expect(scorm.findApi(fake)).toBeNull();
  });
});

describe("Tracker", () => {
  it("completes on the last step, bookmarks and resumes", () => {
    const { api, calls, data } = lms({ "cmi.core.lesson_status": "not attempted" });
    const tracker = new scorm.Tracker(api, "last-step", 80);
    expect(tracker.begin()).toBe("");
    expect(calls).toEqual(["initialize", "cmi.core.lesson_status=incomplete", "commit"]);

    tracker.visit("wt-slide-1", false);
    expect(data["cmi.core.lesson_location"]).toBe("wt-slide-1");
    expect(data["cmi.core.lesson_status"]).toBe("incomplete");
    tracker.visit("wt-slide-4", true);
    expect(data["cmi.core.lesson_status"]).toBe("completed");
    // Going back doesn't undo it.
    tracker.visit("wt-slide-2", false);
    expect(data["cmi.core.lesson_status"]).toBe("completed");

    tracker.finish(3_723_450);
    expect(data["cmi.core.session_time"]).toBe("01:02:03.45");
    expect(data["cmi.core.exit"]).toBe("");
    expect(calls.slice(-2)).toEqual(["commit", "finish"]);
    // Once only.
    tracker.finish(1);
    expect(calls.filter((c) => c === "finish")).toHaveLength(1);

    // The next session starts at the bookmark, still completed.
    const next = new scorm.Tracker(lms(data).api, "last-step", 80);
    expect(next.begin()).toBe("wt-slide-2");
    expect(next.status).toBe("completed");
  });

  it("keeps the way taken through decisions for the next session", () => {
    const { api, data } = lms();
    const tracker = new scorm.Tracker(api, "last-step", 80);
    tracker.begin();
    // A decision on slide 4 led straight to the end slide.
    tracker.visit("wt-slide-7", true, "0 1 2 3 4");
    expect(data["cmi.suspend_data"]).toBe("0 1 2 3 4");
    expect(data["cmi.core.lesson_status"]).toBe("completed");
    const next = new scorm.Tracker(lms(data).api, "last-step", 80);
    expect(next.begin()).toBe("wt-slide-7");
    expect(next.trail()).toBe("0 1 2 3 4");
    // Anything else stored there is ignored.
    expect(new scorm.Tracker(lms({ "cmi.suspend_data": "x" }).api, "last-step", 80).trail()).toBe("");
    // A long way keeps its latest part, within SCORM 1.2's 4096 characters.
    const long = Array.from({ length: 2000 }, (_, i) => String(i % 10)).join(" ") + " 12345";
    tracker.visit("wt-slide-1", false, long);
    const kept = data["cmi.suspend_data"]!;
    expect(kept.length).toBeLessThanOrEqual(4096);
    expect(long.endsWith(kept) && kept.endsWith(" 12345")).toBe(true);
    expect(kept).toMatch(/^\d+( \d+)*$/);
  });

  it("suspends an unfinished lesson on leaving", () => {
    const { api, data } = lms();
    const tracker = new scorm.Tracker(api, "last-step", 80);
    tracker.begin();
    tracker.visit("wt-slide-1", false);
    tracker.finish(500);
    expect(data["cmi.core.exit"]).toBe("suspend");
    expect(data["cmi.core.lesson_status"]).toBe("incomplete");
  });

  it("passes or fails by the quiz with the quiz rule", () => {
    const { api, data } = lms();
    const tracker = new scorm.Tracker(api, "quiz-passed", 75);
    tracker.begin();
    // The last step alone doesn't complete it.
    tracker.visit("wt-slide-5", true);
    expect(data["cmi.core.lesson_status"]).toBe("incomplete");

    expect(tracker.answer(2, 4)).toEqual({ score: 50, passed: false });
    expect(data["cmi.core.lesson_status"]).toBe("failed");
    expect(data["cmi.core.score.raw"]).toBe("50");
    expect(data["cmi.core.score.min"]).toBe("0");
    expect(data["cmi.core.score.max"]).toBe("100");

    expect(tracker.answer(3, 4)).toEqual({ score: 75, passed: true });
    expect(data["cmi.core.lesson_status"]).toBe("passed");
    expect(data["cmi.core.score.raw"]).toBe("75");

    // A worse try keeps the pass and its score.
    expect(tracker.answer(0, 4)).toEqual({ score: 0, passed: false });
    expect(data["cmi.core.lesson_status"]).toBe("passed");
    expect(data["cmi.core.score.raw"]).toBe("75");
    tracker.finish(0);
    expect(data["cmi.core.exit"]).toBe("");
  });

  it("reports a score but completes on the last step with the last-step rule", () => {
    const { api, data } = lms();
    const tracker = new scorm.Tracker(api, "last-step", 80);
    tracker.begin();
    tracker.visit("wt-slide-3", true);
    expect(tracker.answer(1, 3)).toEqual({ score: 33, passed: false });
    expect(data["cmi.core.lesson_status"]).toBe("completed");
    expect(data["cmi.core.score.raw"]).toBe("33");
  });

  it("works without an LMS, or with one that fails to start", () => {
    for (const api of [null, lms({}, "false").api]) {
      const tracker = new scorm.Tracker(api, "quiz-passed", 80);
      expect(tracker.begin()).toBe("");
      tracker.visit("wt-slide-1", true);
      expect(tracker.answer(1, 1)).toEqual({ score: 100, passed: true });
      tracker.finish(10);
    }
    const failing = lms({}, "false");
    new scorm.Tracker(failing.api, "last-step", 80).finish(1);
    expect(failing.calls).toEqual(["initialize"]);
  });
});

describe("sessionTime", () => {
  it("is a SCORM 1.2 timespan", () => {
    expect(scorm.sessionTime(0)).toBe("00:00:00.00");
    expect(scorm.sessionTime(61_005)).toBe("00:01:01.01");
    expect(scorm.sessionTime(100 * 3_600_000)).toBe("100:00:00.00");
    expect(scorm.sessionTime(-5)).toBe("00:00:00.00");
  });
});
