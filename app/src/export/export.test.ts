import { describe, expect, it, vi } from "vitest";
import {
  baseName,
  exportName,
  isAccent,
  isPlainName,
  outPath,
  runExport,
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
    expect(outPath("md", { ...destination, name: "a/b" })).toBeNull();
    expect(outPath("pdf", destination)).toBe("/Users/me/Guide.pdf");
    // The file chosen for another format doesn't count.
    expect(outPath("docx", destination)).toBeNull();
    expect(baseName("/Users/me/Guide.pdf")).toBe("Guide.pdf");
    expect(baseName("/Users/me/Out/")).toBe("Out");
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
    expect(exportGuide).toHaveBeenCalledWith("G", "md", "/out/Guide", false, { logo: null, accent: "#abc" });
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
