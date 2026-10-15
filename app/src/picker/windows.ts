import type { CaptureWindow } from "@/lib/types";

/** The window's title, or a stand-in for one without. */
export function windowTitle(window: CaptureWindow): string {
  return window.title?.trim() || "Untitled window";
}

/** Width × height in points. */
export function windowSize(window: CaptureWindow): string {
  return `${Math.round(window.width)} × ${Math.round(window.height)}`;
}

/**
 * The windows whose app name, title or bundle id contain every word of
 * `query` (ignoring case), in their order.
 */
export function filterWindows(windows: CaptureWindow[], query: string): CaptureWindow[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return windows;
  return windows.filter((w) => {
    const text = `${w.appName} ${w.title ?? ""} ${w.bundleId ?? ""}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}
