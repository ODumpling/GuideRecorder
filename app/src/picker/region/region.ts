import type { Rect } from "@guiderecorder/guide-schema";

/** The smallest region side, in points; `start_recording` refuses smaller. */
export const MIN_REGION_SIDE = 40;

export interface Point {
  x: number;
  y: number;
}

/**
 * The rectangle between where a drag started and where the pointer is,
 * whichever way it went, kept inside the `bounds` (width × height) and
 * rounded to whole points.
 */
export function dragRect(from: Point, to: Point, bounds: { width: number; height: number }): Rect {
  const clampX = (x: number) => Math.min(Math.max(x, 0), bounds.width);
  const clampY = (y: number) => Math.min(Math.max(y, 0), bounds.height);
  const left = Math.round(Math.min(clampX(from.x), clampX(to.x)));
  const top = Math.round(Math.min(clampY(from.y), clampY(to.y)));
  const right = Math.round(Math.max(clampX(from.x), clampX(to.x)));
  const bottom = Math.round(Math.max(clampY(from.y), clampY(to.y)));
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/** Big enough to record: both sides at least `MIN_REGION_SIDE`. */
export function isPickable(rect: Rect | null): rect is Rect {
  return !!rect && rect.width >= MIN_REGION_SIDE && rect.height >= MIN_REGION_SIDE;
}
