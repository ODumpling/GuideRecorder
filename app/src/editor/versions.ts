import type { Guide } from "@guiderecorder/guide-schema";
import type { VersionInfo, VersionReason } from "@/lib/types";

// The History dialog's text: what each version is, and a line diff of a
// guide's text (guide fields, then one block per section, step and note).

export const REASONS: Record<VersionReason, string> = {
  closed: "Closed",
  beforeAi: "Before Write with AI",
  manual: "Saved by you",
  beforeRestore: "Before a restore",
  beforeMcpEdit: "Before an MCP edit",
  other: "Saved",
};

/** The version's label, else why it was saved. */
export function versionName(version: VersionInfo): string {
  return version.label ?? REASONS[version.reason];
}

/** The guide's text as lines, for a diff. Empty fields are left out. */
export function guideLines(guide: Guide): string[] {
  const lines: string[] = [`Title: ${guide.title}`];
  const field = (name: string, value: string | undefined) => {
    if (value?.trim()) lines.push(...`${name}: ${value.trim()}`.split("\n"));
  };
  field("Summary", guide.summary);
  field("Intro", guide.intro);
  field("Prerequisites", guide.prerequisites);
  if (guide.tags.length > 0) lines.push(`Tags: ${guide.tags.join(", ")}`);
  let number = 0;
  for (const item of guide.items) {
    lines.push("");
    switch (item.type) {
      case "section":
        lines.push(`## ${item.title}`);
        break;
      case "step": {
        lines.push(`${++number}. ${item.title}`);
        for (const line of item.description.trim().split("\n")) if (line.trim()) lines.push(`   ${line}`);
        const hotspots = item.annotations.filter((a) => a.type === "hotspot").length;
        const marks = item.annotations.length - hotspots;
        if (marks > 0) lines.push(`   [${marks} ${marks === 1 ? "annotation" : "annotations"}]`);
        if (hotspots > 0) lines.push(`   [${hotspots} ${hotspots === 1 ? "hotspot" : "hotspots"}]`);
        break;
      }
      case "note":
        lines.push(...`${item.kind === "tip" ? "Tip" : item.kind === "warning" ? "Warning" : "Note"}: ${item.body.trim()}`.split("\n"));
        break;
    }
  }
  return lines;
}

export type DiffLine = { kind: "same" | "added" | "removed"; text: string };

/** Longest diff computed exactly; beyond it, lines are compared in place. */
const MAX_CELLS = 4_000_000;

/**
 * A line diff turning `before` into `after` (longest common subsequence):
 * removed lines come before the lines added in their place.
 */
export function diffLines(before: string[], after: string[]): DiffLine[] {
  // Common ends are cheap, and keep the table small.
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start++;
  let end = 0;
  while (
    end < before.length - start &&
    end < after.length - start &&
    before[before.length - 1 - end] === after[after.length - 1 - end]
  ) {
    end++;
  }
  const a = before.slice(start, before.length - end);
  const b = after.slice(start, after.length - end);
  const same = (lines: string[]) => lines.map((text): DiffLine => ({ kind: "same", text }));
  const middle: DiffLine[] = [];
  if ((a.length + 1) * (b.length + 1) > MAX_CELLS) {
    middle.push(...a.map((text): DiffLine => ({ kind: "removed", text })));
    middle.push(...b.map((text): DiffLine => ({ kind: "added", text })));
  } else {
    // lcs[i][j]: the longest common subsequence of a[i..] and b[j..].
    const width = b.length + 1;
    const lcs = new Uint32Array((a.length + 1) * width);
    for (let i = a.length - 1; i >= 0; i--) {
      for (let j = b.length - 1; j >= 0; j--) {
        lcs[i * width + j] =
          a[i] === b[j] ? lcs[(i + 1) * width + j + 1]! + 1 : Math.max(lcs[(i + 1) * width + j]!, lcs[i * width + j + 1]!);
      }
    }
    let i = 0;
    let j = 0;
    while (i < a.length || j < b.length) {
      if (i < a.length && j < b.length && a[i] === b[j]) {
        middle.push({ kind: "same", text: a[i]! });
        i++;
        j++;
      } else if (j >= b.length || (i < a.length && lcs[(i + 1) * width + j]! >= lcs[i * width + j + 1]!)) {
        middle.push({ kind: "removed", text: a[i++]! });
      } else {
        middle.push({ kind: "added", text: b[j++]! });
      }
    }
  }
  return [...same(before.slice(0, start)), ...middle, ...same(before.slice(before.length - end))];
}

/** How many lines a diff adds and removes. */
export function diffStats(diff: DiffLine[]): { added: number; removed: number } {
  return {
    added: diff.filter((line) => line.kind === "added").length,
    removed: diff.filter((line) => line.kind === "removed").length,
  };
}
