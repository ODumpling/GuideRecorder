import { describe, expect, it } from "vitest";
import type { CaptureWindow } from "@/lib/types";
import { filterWindows, windowSize, windowTitle } from "@/picker/windows";

const safari: CaptureWindow = { windowId: 1, appName: "Safari", bundleId: "com.apple.Safari", title: "Apple Support", width: 1200.4, height: 800 };
const notes: CaptureWindow = { windowId: 2, appName: "Notes", bundleId: "com.apple.Notes", title: null, width: 600, height: 500 };

describe("window picker helpers", () => {
  it("labels a window without a title", () => {
    expect(windowTitle(safari)).toBe("Apple Support");
    expect(windowTitle(notes)).toBe("Untitled window");
    expect(windowTitle({ ...notes, title: "  " })).toBe("Untitled window");
  });

  it("shows the size in whole points", () => {
    expect(windowSize(safari)).toBe("1200 × 800");
  });

  it("filters on app, title and bundle id, every word, ignoring case", () => {
    expect(filterWindows([safari, notes], "")).toEqual([safari, notes]);
    expect(filterWindows([safari, notes], "NOTES")).toEqual([notes]);
    expect(filterWindows([safari, notes], "safari support")).toEqual([safari]);
    expect(filterWindows([safari, notes], "com.apple")).toEqual([safari, notes]);
    expect(filterWindows([safari, notes], "safari notes")).toEqual([]);
  });
});
