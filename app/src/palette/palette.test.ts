import { describe, expect, it } from "vitest";
import type { GuideListItem } from "@/lib/types";
import {
  entryScore,
  fuzzyScore,
  parseQuery,
  rankEntries,
  recentFirst,
  recentGuideIds,
  rememberOpened,
  STEP_MATCH,
  type PaletteEntry,
} from "@/palette/palette";

function item(id: string, fields: Partial<GuideListItem> = {}): GuideListItem {
  return {
    id,
    path: id,
    title: id,
    summary: "",
    tags: [],
    stepCount: 1,
    kind: "walkthrough",
    hotspotCount: 0,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    folder: "",
    readOnly: false,
    duplicateOf: null,
    ...fields,
  };
}

function memory(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
  };
}

const guide = (id: string, title: string, recency: number, fields: Partial<PaletteEntry> = {}): PaletteEntry => ({
  id,
  kind: "guide",
  title,
  recency,
  ...fields,
});
const command = (id: string, title: string, fields: Partial<PaletteEntry> = {}): PaletteEntry => ({
  id,
  kind: "command",
  title,
  ...fields,
});
const tag = (name: string): PaletteEntry => ({ id: `tag:${name}`, kind: "tag", title: name });
const ids = (entries: PaletteEntry[]) => entries.map((e) => e.id);

describe("parseQuery", () => {
  it("reads the mode prefix", () => {
    expect(parseQuery("  vpn ")).toEqual({ mode: "all", text: "vpn" });
    expect(parseQuery("> export")).toEqual({ mode: "commands", text: "export" });
    expect(parseQuery("#it")).toEqual({ mode: "tags", text: "it" });
    expect(parseQuery(">")).toEqual({ mode: "commands", text: "" });
  });
});

describe("fuzzyScore", () => {
  it("ranks exact, prefix, word, substring, then letters in order", () => {
    const exact = fuzzyScore("export", "Export")!;
    const prefix = fuzzyScore("exp", "Export as PDF")!;
    const word = fuzzyScore("pdf", "Export as PDF")!;
    const inside = fuzzyScore("port", "Export as PDF")!;
    const letters = fuzzyScore("eap", "Export as PDF")!;
    expect(exact).toBeGreaterThan(prefix);
    expect(prefix).toBeGreaterThan(word);
    expect(word).toBeGreaterThan(inside);
    expect(inside).toBeGreaterThan(letters);
    expect(letters).toBeGreaterThan(0);
  });

  it("prefers letters at word starts", () => {
    expect(fuzzyScore("sce", "Share a calendar event")!).toBeGreaterThan(fuzzyScore("hrl", "Share a calendar event")!);
  });

  it("fails when a letter is missing or out of order", () => {
    expect(fuzzyScore("xyz", "Export")).toBeNull();
    expect(fuzzyScore("tx", "Export")).toBeNull();
  });

  it("matches anything with an empty query", () => {
    expect(fuzzyScore("  ", "Export")).toBe(0);
  });
});

describe("entryScore", () => {
  it("weighs keywords below the title", () => {
    const entry = guide("a", "Rotate an API key", 0, { keywords: ["security"] });
    expect(entryScore(entry, "rotate")!).toBeGreaterThan(entryScore(entry, "security")!);
    expect(entryScore(entry, "nothing")).toBeNull();
  });

  it("counts a guide found by its step text", () => {
    const entry = guide("a", "Rotate an API key", 0);
    expect(entryScore(entry, "dashboard", new Set(["a"]))).toBe(STEP_MATCH);
    expect(entryScore(command("a", "Duplicate"), "dashboard", new Set(["a"]))).toBeNull();
  });
});

describe("rankEntries", () => {
  const entries = [
    command("new", "New annotation guide"),
    guide("old", "Old guide", 3),
    guide("vpn", "Connect to the VPN", 0, { keywords: ["network"] }),
    command("theme-nebula", "Theme: Nebula", { searchOnly: true }),
    guide("printer", "Add a printer", 1),
    tag("network"),
    command("export", "Export as…"),
  ];

  it("lists recent guides, then commands, before anything is typed", () => {
    expect(ids(rankEntries(entries, ""))).toEqual(["vpn", "printer", "old", "new", "export"]);
    expect(ids(rankEntries(entries, "", { recentLimit: 1 }))).toEqual(["vpn", "new", "export"]);
  });

  it("keeps commands only after >", () => {
    expect(ids(rankEntries(entries, ">"))).toEqual(["new", "export"]);
    expect(ids(rankEntries(entries, "> neb"))).toEqual(["theme-nebula"]);
  });

  it("keeps tags only after #, all of them when empty", () => {
    expect(ids(rankEntries(entries, "#"))).toEqual(["tag:network"]);
    expect(ids(rankEntries(entries, "#net"))).toEqual(["tag:network"]);
    expect(ids(rankEntries(entries, "#vpn"))).toEqual([]);
  });

  it("orders matches by score, then recency, then the given order", () => {
    expect(ids(rankEntries(entries, "network"))).toEqual(["tag:network", "vpn"]);
    const ties = [guide("b", "Guide b", 2), guide("a", "Guide a", 1), command("c", "Guide c")];
    expect(ids(rankEntries(ties, "guide"))).toEqual(["a", "b", "c"]);
  });

  it("finds search-only entries once typed", () => {
    expect(ids(rankEntries(entries, "nebula"))).toEqual(["theme-nebula"]);
  });

  it("adds guides found by their step text", () => {
    expect(ids(rankEntries(entries, "dashboard", { stepMatches: new Set(["old"]) }))).toEqual(["old"]);
  });
});

describe("recent guides", () => {
  it("remembers the opened guides, most recent first, once each", () => {
    const storage = memory();
    expect(recentGuideIds(storage)).toEqual([]);
    rememberOpened("a", storage);
    rememberOpened("b", storage);
    rememberOpened("a", storage);
    expect(recentGuideIds(storage)).toEqual(["a", "b"]);
  });

  it("ignores what is not a list of ids", () => {
    expect(recentGuideIds(memory({ "guiderecorder.recentGuides": "{" }))).toEqual([]);
    expect(recentGuideIds(memory({ "guiderecorder.recentGuides": '["a", 1]' }))).toEqual(["a"]);
  });

  it("puts opened guides first, then the rest by last update", () => {
    const guides = [
      item("old", { updatedAt: "2026-01-01T00:00:00Z" }),
      item("new", { updatedAt: "2026-03-01T00:00:00Z" }),
      item("opened", { updatedAt: "2025-01-01T00:00:00Z" }),
      item("before", { updatedAt: "2025-01-01T00:00:00Z" }),
    ];
    expect(recentFirst(guides, ["gone", "opened", "before"]).map((g) => g.id)).toEqual(["opened", "before", "new", "old"]);
  });
});
