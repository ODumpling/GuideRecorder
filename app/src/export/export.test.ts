import { describe, expect, it, vi } from "vitest";
import {
  baseName,
  DEFAULT_MOTION,
  exportName,
  initialMotion,
  isDocumentFormat,
  isMotionFormat,
  isPaperFormat,
  isSecondsPerStep,
  rememberMotion,
  initialPaper,
  initialReviewDetails,
  isAccent,
  isPlainName,
  localePaper,
  isFolderFormat,
  outPath,
  rememberPaper,
  rememberReviewDetails,
  runExport,
  unreviewedBlurs,
  unreviewedBlursText,
  warningLines,
} from "@/export/export";
import type { ExportApi } from "@/lib/ipc";
import type { ExportResult, Item } from "@/lib/types";

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
    expect(isDocumentFormat("pptx") || isDocumentFormat("mp4")).toBe(false);
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
    expect(exportGuide).toHaveBeenCalledWith("G", "md", "/out/Guide", false, { logo: null, accent: "#abc" }, "letter", true, DEFAULT_MOTION);
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
