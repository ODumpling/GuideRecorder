// Where the after frame (`StepImage.after`) is shown: in the Before view's
// framing, so the two line up when switching.

import type { Rect } from "@/lib/types";

type Size = { width: number; height: number };

/**
 * The after frame's view (its pixels; the SVG `viewBox`). `crop`: the part
 * of the full screenshot the Before view shows, or null when it shows the
 * whole screenshot. The crop applies only when the after frame is the full
 * screenshot's size; otherwise the whole after frame is shown. Null until
 * the sizes it needs are known (`full` is needed only with a crop).
 */
export function afterFrame(crop: Rect | null, after: Size | null, full: Size | null): Rect | null {
  if (!after) return null;
  const whole = { x: 0, y: 0, width: after.width, height: after.height };
  if (!crop) return whole;
  if (!full) return null;
  return after.width === full.width && after.height === full.height ? crop : whole;
}
