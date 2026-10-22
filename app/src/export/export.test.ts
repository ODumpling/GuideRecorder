import { describe, expect, it, vi } from "vitest";
import {
  blankPreset,
  exportBranding,
  initialPreset,
  isFontFamily,
  LAYOUTS,
  presetProblem,
  rememberPreset,
  baseName,
  defaultCourseId,
  DEFAULT_MOTION,
  DEFAULT_SCORM,
  DEFAULT_SKILL,
  exportName,
  initialMotion,
  isDocumentFormat,
  isMotionFormat,
  isPaperFormat,
  isSecondsPerStep,
  rememberMotion,
  initialPaper,
  initialReviewDetails,
  initialSkill,
  initialScorm,
  isMasteryScore,
  isAccent,
  isPlainName,
  localePaper,
  isFolderFormat,
  outPath,
  rememberPaper,
  rememberReviewDetails,
  rememberSkill,
  rememberScorm,
  runExport,
  scormProblem,
  unreviewedBlurs,
  unreviewedBlursText,
  warningLines,
} from "@/export/export";
import type { ExportApi } from "@/lib/ipc";
import type { Item } from "@guiderecorder/guide-schema";
import type { ExportResult, PresetList } from "@/lib/types";

function api(overrides: Partial<ExportApi> = {}): ExportApi {
  return {
    chooseFolder: vi.fn(async () => null),
    chooseFile: vi.fn(async () => null),
    chooseLogo: vi.fn(async () => null),
    exportGuide: vi.fn(async () => ({ files: ["/out/Guide/guide.md"], warnings: [], notes: [] })),
    previewExport: vi.fn(async () => ({ kind: "html" as const, html: "<p>Guide</p>" })),
    logoPreview: vi.fn(async () => "data:image/png;base64,"),
    reveal: vi.fn(async () => {}),
    ...overrides,
  };
}

const request = {
  guideId: "G",
  format: "md" as const,
  out: "/out/Guide",
  overwrite: false,
  branding: { logo: null, accent: "  #abc " },
  paper: "letter" as const,
  reviewDetails: true,
  motion: DEFAULT_MOTION,
};

describe("export names and paths", () => {
  it("makes a safe name from the title", () => {
    expect(exportName("Rotate an API key")).toBe("Rotate an API key");
    expect(exportName("a/b\\c: d")).toBe("a-b-c- d");
    expect(exportName("..hidden")).toBe("hidden");
    expect(exportName("tab\there\nnew")).toBe("tab-here-new");
    expect(exportName("   ")).toBe("Guide");
    expect(exportName("x".repeat(300))).toHaveLength(100);
  });

  it("accepts plain folder names only", () => {
    expect(isPlainName("Guide export")).toBe(true);
    for (const bad of ["", "  ", ".", "..", "a/b", "a\u0000"]) expect(isPlainName(bad)).toBe(false);
  });

  it("checks the accent like the exporters", () => {
    for (const ok of ["", "#abc", "#A1b2C3", " #fff "]) expect(isAccent(ok)).toBe(true);
    for (const bad of ["red", "#abcd", "abc", "#12345g"]) expect(isAccent(bad)).toBe(false);
  });

  it("builds the out path per format", () => {
    const destination = { folder: "/Users/me/Desktop/", name: " Guide ", file: "/Users/me/Guide.pdf" };
    expect(outPath("md", destination)).toBe("/Users/me/Desktop/Guide");
    expect(outPath("html", { ...destination, folder: null })).toBeNull();
    expect(isFolderFormat("walkthrough")).toBe(true);
    expect(outPath("walkthrough", destination)).toBe("/Users/me/Desktop/Guide");
    expect(isFolderFormat("skill")).toBe(true);
    expect(outPath("skill", destination)).toBe("/Users/me/Desktop/Guide");
    expect(outPath("md", { ...destination, name: "a/b" })).toBeNull();
    expect(outPath("pdf", destination)).toBe("/Users/me/Guide.pdf");
    // The file chosen for another format doesn't count.
    expect(outPath("docx", destination)).toBeNull();
    expect(baseName("/Users/me/Guide.pdf")).toBe("Guide.pdf");
    expect(baseName("/Users/me/Out/")).toBe("Out");
  });
});

describe("paper size", () => {
  it("defaults from the system locale", () => {
    for (const tag of ["en-US", "en-CA", "fr-CA", "es-MX", "fil-PH", "en-PH", "es-CL", "en"]) {
      expect(localePaper([tag])).toBe("letter");
    }
    for (const tag of ["en-GB", "en-AU", "de-DE", "de", "fr-FR", "ja-JP", "es-ES", "pt-BR"]) {
      expect(localePaper([tag])).toBe("a4");
    }
    // The first usable tag decides.
    expect(localePaper(["not a tag!", "en-GB", "en-US"])).toBe("a4");
    expect(localePaper([])).toBe("a4");
  });

  it("remembers the last choice over the locale", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, value),
    };
    expect(initialPaper(storage, ["en-US"])).toBe("letter");
    rememberPaper("a4", storage);
    expect(initialPaper(storage, ["en-US"])).toBe("a4");
    rememberPaper("letter", storage);
    expect(initialPaper(storage, ["de-DE"])).toBe("letter");
    // Junk or unreadable storage falls back to the locale.
    store.set("guiderecorder.exportPaper", "legal");
    expect(initialPaper(storage, ["de-DE"])).toBe("a4");
    const broken = {
      getItem: () => {
        throw new Error("denied");
      },
    };
    expect(initialPaper(broken, ["en-CA"])).toBe("letter");
  });
});

describe("review details", () => {
  it("are off at first and remember the last choice", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, value),
    };
    expect(initialReviewDetails(storage)).toBe(false);
    rememberReviewDetails(true, storage);
    expect(initialReviewDetails(storage)).toBe(true);
    rememberReviewDetails(false, storage);
    expect(initialReviewDetails(storage)).toBe(false);
    const broken = {
      getItem: () => {
        throw new Error("denied");
      },
    };
    expect(initialReviewDetails(broken)).toBe(false);
  });
});

describe("video, GIF and slides", () => {
  it("groups the formats", () => {
    expect(["mp4", "gif"].every((f) => isMotionFormat(f as "mp4"))).toBe(true);
    expect(isMotionFormat("pptx")).toBe(false);
    expect(isPaperFormat("pdf") && isPaperFormat("docx")).toBe(true);
    expect(isPaperFormat("pptx") || isPaperFormat("gif")).toBe(false);
    expect(isDocumentFormat("md") && isDocumentFormat("pdf")).toBe(true);
    expect(isDocumentFormat("pptx") || isDocumentFormat("mp4") || isDocumentFormat("skill")).toBe(false);
    expect(isFolderFormat("gif")).toBe(false);
    expect(outPath("mp4", { folder: null, name: "x", file: "/out/Guide.mp4" })).toBe("/out/Guide.mp4");
    expect(outPath("gif", { folder: null, name: "x", file: "/out/Guide.mp4" })).toBeNull();
  });

  it("accepts 1 to 30 seconds per step", () => {
    expect([1, 2.5, 30].every(isSecondsPerStep)).toBe(true);
    expect([0.5, 31, Number.NaN, "3", null].some(isSecondsPerStep)).toBe(false);
  });

  it("remembers the options and ignores bad stored values", () => {
    const store = new Map<string, string>();
    const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) };
    expect(initialMotion(storage)).toEqual(DEFAULT_MOTION);
    const chosen = { secondsPerStep: 5, aspect: "vertical" as const, zoom: false, captions: false };
    rememberMotion(chosen, storage);
    expect(initialMotion(storage)).toEqual(chosen);
    store.set("guiderecorder.exportMotion", JSON.stringify({ secondsPerStep: 99, aspect: "tall", zoom: "yes" }));
    expect(initialMotion(storage)).toEqual(DEFAULT_MOTION);
    store.set("guiderecorder.exportMotion", "{not json");
    expect(initialMotion(storage)).toEqual(DEFAULT_MOTION);
  });
});

describe("skill options", () => {
  it("remember whether to add the replay script, off at first", () => {
    const store = new Map<string, string>();
    const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) };
    expect(initialSkill(storage)).toEqual({ replay: false });
    rememberSkill({ replay: true }, storage);
    expect(initialSkill(storage)).toEqual({ replay: true });
    const broken = { getItem: () => { throw new Error("no storage"); } };
    expect(initialSkill(broken)).toEqual(DEFAULT_SKILL);
  });

  it("are sent with a skill export", async () => {
    const exportGuide = vi.fn(async () => ({ files: ["/out/Guide/SKILL.md"], warnings: [], notes: [] }));
    await runExport({ flush: async () => true, api: api({ exportGuide }) }, { ...request, format: "skill", skill: { replay: true } });
    expect(exportGuide).toHaveBeenCalledWith("G", "skill", "/out/Guide", false, { logo: null, accent: "#abc", preset: null, layout: null }, "letter", true, DEFAULT_MOTION, { replay: true }, DEFAULT_SCORM, null);
  });
});

describe("SCORM options", () => {
  it("export a .zip file", () => {
    expect(isFolderFormat("scorm")).toBe(false);
    // Walkthrough-based: branding and the review details apply.
    expect(isDocumentFormat("scorm")).toBe(true);
    expect(outPath("scorm", { folder: null, name: "x", file: "/out/Course.zip" })).toBe("/out/Course.zip");
    expect(outPath("scorm", { folder: null, name: "x", file: "/out/Course.scorm" })).toBeNull();
    expect(defaultCourseId("01ABC")).toBe("guiderecorder-01ABC");
  });

  it("remember the rule and mastery score, and each guide's course and quiz", () => {
    const store = new Map<string, string>();
    const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) };
    expect(initialScorm("A", storage)).toEqual(DEFAULT_SCORM);
    const chosen = { title: "Course", identifier: "c-1", completion: "quizPassed" as const, masteryScore: 70, quiz: "Q?\n- [x] A\n- [ ] B" };
    rememberScorm("A", chosen, storage);
    expect(initialScorm("A", storage)).toEqual(chosen);
    // Another guide keeps the rule and score, not the course or quiz.
    expect(initialScorm("B", storage)).toEqual({ ...DEFAULT_SCORM, completion: "quizPassed", masteryScore: 70 });
    store.set("guiderecorder.exportScorm", JSON.stringify({ completion: "never", masteryScore: 101 }));
    store.set("guiderecorder.exportScorm.A", "{not json");
    expect(initialScorm("A", storage)).toEqual(DEFAULT_SCORM);
    const broken = { getItem: () => { throw new Error("no storage"); } };
    expect(initialScorm(null, broken)).toEqual(DEFAULT_SCORM);
  });

  it("need a whole mastery score, and a quiz for the quiz rule", () => {
    expect([0, 80, 100].every(isMasteryScore)).toBe(true);
    expect([-1, 101, 50.5, Number.NaN, "80"].some(isMasteryScore)).toBe(false);
    expect(scormProblem(DEFAULT_SCORM)).toBeNull();
    expect(scormProblem({ ...DEFAULT_SCORM, masteryScore: Number.NaN })).toMatch(/mastery score/);
    expect(scormProblem({ ...DEFAULT_SCORM, completion: "quizPassed", quiz: "  " })).toMatch(/Add a quiz/);
    expect(scormProblem({ ...DEFAULT_SCORM, completion: "quizPassed", quiz: "Q?" })).toBeNull();
  });

  it("are sent with a SCORM export", async () => {
    const exportGuide = vi.fn(async () => ({ files: ["/out/Course.zip"], warnings: [], notes: [] }));
    const scorm = { ...DEFAULT_SCORM, title: "Course" };
    await runExport({ flush: async () => true, api: api({ exportGuide }) }, { ...request, format: "scorm", out: "/out/Course.zip", scorm });
    expect(exportGuide).toHaveBeenCalledWith("G", "scorm", "/out/Course.zip", false, { logo: null, accent: "#abc", preset: null, layout: null }, "letter", true, DEFAULT_MOTION, DEFAULT_SKILL, scorm, null);
  });
});

describe("audience", () => {
  it("is sent when chosen, and null (everyone) otherwise", async () => {
    const exportGuide = vi.fn(async () => ({ files: ["/out/Guide/guide.md"], warnings: [], notes: [] }));
    await runExport({ flush: async () => true, api: api({ exportGuide }) }, { ...request, audience: "Admin" });
    expect(exportGuide).toHaveBeenLastCalledWith("G", "md", "/out/Guide", false, { logo: null, accent: "#abc", preset: null, layout: null }, "letter", true, DEFAULT_MOTION, DEFAULT_SKILL, DEFAULT_SCORM, "Admin");
    await runExport({ flush: async () => true, api: api({ exportGuide }) }, { ...request, audience: "" });
    expect(exportGuide).toHaveBeenLastCalledWith("G", "md", "/out/Guide", false, { logo: null, accent: "#abc", preset: null, layout: null }, "letter", true, DEFAULT_MOTION, DEFAULT_SKILL, DEFAULT_SCORM, null);
  });
});

describe("runExport", () => {
  it("saves first, then exports with a trimmed accent", async () => {
    const order: string[] = [];
    const exportGuide = vi.fn(async () => {
      order.push("export");
      return { files: ["/out/Guide/guide.md"], warnings: [], notes: [] };
    });
    const outcome = await runExport(
      {
        flush: async () => {
          order.push("flush");
          return true;
        },
        api: api({ exportGuide }),
      },
      request,
    );
    expect(order).toEqual(["flush", "export"]);
    expect(exportGuide).toHaveBeenCalledWith("G", "md", "/out/Guide", false, { logo: null, accent: "#abc", preset: null, layout: null }, "letter", true, DEFAULT_MOTION, DEFAULT_SKILL, DEFAULT_SCORM, null);
    expect(outcome).toEqual({ ok: true, result: { files: ["/out/Guide/guide.md"], warnings: [], notes: [] } });
  });

  it("does not export when the save fails", async () => {
    const exportApi = api();
    const outcome = await runExport({ flush: async () => false, api: exportApi }, request);
    expect(outcome.ok).toBe(false);
    expect(exportApi.exportGuide).not.toHaveBeenCalled();
  });

  it("reports an existing target so overwrite can be offered", async () => {
    const exportGuide = vi.fn(async () => {
      throw { kind: "exists", message: "/out/Guide already has files in it" };
    });
    const outcome = await runExport({ flush: async () => true, api: api({ exportGuide }) }, request);
    expect(outcome).toEqual({ ok: false, exists: true, error: "/out/Guide already has files in it" });
    const other = await runExport(
      { flush: async () => true, api: api({ exportGuide: async () => Promise.reject(new Error("boom")) }) },
      { ...request, branding: { logo: null, accent: "" } },
    );
    expect(other).toEqual({ ok: false, exists: false, error: "boom" });
  });
});

it("names warnings by step number", () => {
  const step = (id: string) => ({ type: "step", id }) as Item;
  const items: Item[] = [step("A"), { type: "section", id: "S", title: "x" }, step("B")];
  const result: ExportResult = {
    files: [],
    warnings: [
      { stepId: "B", message: "crop position unknown" },
      { stepId: "Z", message: "gone" },
    ],
    notes: [],
  };
  expect(warningLines(result, items)).toEqual(["Step 2: crop position unknown", "A removed step: gone"]);
});

describe("unreviewed blur suggestions", () => {
  const rect = { x: 1, y: 2, width: 3, height: 4 };
  function steps(counts: number[]): Item[] {
    return counts.map((n, i) => ({
      type: "step",
      id: `S${i + 1}`,
      title: "",
      description: "",
      kind: "click",
      image: { full: "steps/x.webp", show: "full" },
      annotations: [],
      aiState: { status: "ai", redactionSuggestions: Array.from({ length: n }, () => ({ rect, reason: "email address" })) },
    }));
  }

  it("counts suggestions by step number, skipping other items", () => {
    const items: Item[] = [{ type: "section", id: "X", title: "Start" }, ...steps([0, 2, 0, 1])];
    expect(unreviewedBlurs(items)).toEqual({ stepIds: ["S2", "S4"], steps: [2, 4], count: 3 });
    expect(unreviewedBlurs(steps([0, 0]))).toEqual({ stepIds: [], steps: [], count: 0 });
  });

  it("skips reviewed suggestions", () => {
    const items = steps([2, 1]);
    const [first, second] = items as Extract<Item, { type: "step" }>[];
    first.aiState.redactionSuggestions![0].review = "kept";
    second.aiState.redactionSuggestions![0].review = "blurred";
    expect(unreviewedBlurs(items)).toEqual({ stepIds: ["S1"], steps: [1], count: 1 });
  });

  it("words the warning", () => {
    expect(unreviewedBlursText({ steps: [], count: 0 })).toBeNull();
    expect(unreviewedBlursText({ steps: [3], count: 1 })).toBe(
      "1 suggested blur in step 3 has not been reviewed. Unless accepted, it is not blurred in the export.",
    );
    expect(unreviewedBlursText({ steps: [2, 4], count: 3 })).toBe(
      "3 suggested blurs in steps 2 and 4 have not been reviewed. Unless accepted, they are not blurred in the export.",
    );
    expect(unreviewedBlursText({ steps: [1, 2, 3, 4, 5, 6, 7], count: 7 })).toContain("in steps 1, 2, 3, 4 and 3 more have");
    expect(unreviewedBlursText({ steps: [1, 2, 3, 4, 5], count: 5 })).toContain("in steps 1, 2, 3, 4 and 5 have");
  });
});

describe("brand presets", () => {
  const list = (ids: string[], defaultId: string | null = null): PresetList => ({
    presets: ids.map((id) => ({
      id,
      preset: id === "broken" ? null : blankPreset(id),
      logo: null,
      error: id === "broken" ? "not a valid preset" : null,
    })),
    defaultId,
  });
  const storage = (value: string | null) => ({ getItem: () => value });

  it("start from the last choice, else the library's default", () => {
    expect(initialPreset(list(["acme", "zeta"], "zeta"), storage(null))).toBe("zeta");
    expect(initialPreset(list(["acme", "zeta"], "zeta"), storage("acme"))).toBe("acme");
    // None chosen last time stays none.
    expect(initialPreset(list(["acme"], "acme"), storage(""))).toBe("");
    // Gone or unreadable: the default, else none.
    expect(initialPreset(list(["acme"], "acme"), storage("old"))).toBe("acme");
    expect(initialPreset(list(["broken"], "broken"), storage(null))).toBe("");
    expect(initialPreset(list([]), storage(null))).toBe("");
  });

  it("are remembered", () => {
    const setItem = vi.fn();
    rememberPreset("acme", { setItem });
    expect(setItem).toHaveBeenCalledWith("guiderecorder.exportPreset", "acme");
  });

  it("send the preset instead of the logo and accent, and the layout where it applies", () => {
    const choices = { logo: "/logo.png", accent: "#abc", preset: "acme", layout: "compact" as const };
    expect(exportBranding("pdf", choices)).toEqual({ logo: null, accent: null, preset: "acme", layout: "compact" });
    // The walkthrough has no layout; slides no branding.
    expect(exportBranding("walkthrough", choices)).toEqual({ logo: null, accent: null, preset: "acme", layout: null });
    expect(exportBranding("pptx", choices)).toEqual({ logo: null, accent: null, preset: null, layout: null });
    expect(exportBranding("md", { ...choices, preset: "" })).toEqual({ logo: "/logo.png", accent: null, preset: null, layout: "compact" });
    expect(exportBranding("html", { ...choices, preset: "", layout: null })).toEqual({ logo: "/logo.png", accent: "#abc", preset: null, layout: null });
    expect(LAYOUTS.map((l) => l.layout)).toEqual(["image-then-text", "side-by-side", "compact", "checklist"]);
  });

  it("are checked before saving", () => {
    expect(presetProblem(blankPreset(""))).toMatch(/name/);
    expect(presetProblem(blankPreset("Acme"))).toBeNull();
    const preset = blankPreset("Acme");
    expect(presetProblem({ ...preset, colors: { ...preset.colors, heading: "navy" } })).toMatch(/Colours/);
    expect(presetProblem({ ...preset, css: "</style>" })).toMatch(/</);
    const font = { kind: "serif" as const, family: "Inter; x" };
    expect(presetProblem({ ...preset, fonts: { ...preset.fonts, body: font } })).toMatch(/Font/);
    expect(isFontFamily("Source Sans 3")).toBe(true);
    expect(isFontFamily("Noto Sans 日本語")).toBe(true);
    expect(isFontFamily("")).toBe(true);
    expect(isFontFamily("a\"b")).toBe(false);
  });
});
