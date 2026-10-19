import { colorOr } from "@guiderecorder/ui/color";
import { DEFAULT_HOTSPOT_COLOR } from "@guiderecorder/ui/geometry";
import type { AnnotationStyle, HotspotGeometry } from "@guiderecorder/guide-schema";

/** The popover's width at most, in CSS pixels; it is kept that far from the right edge. */
export const POPOVER_WIDTH = 288;

/**
 * Where the popover sits in the layer's box, in CSS pixels: `top` below a
 * hotspot, or `bottom` above one.
 */
export type PopoverAnchor = { left: number } & ({ top: number } | { bottom: number });

/** A hotspot's number badge, in its colour. */
export function HotspotBadge({ number, style }: { number: number; style?: AnnotationStyle }) {
  return (
    <span
      className="flex size-5 shrink-0 items-center justify-center rounded-full text-xs font-semibold text-white tabular-nums"
      style={{ background: colorOr(style?.color, DEFAULT_HOTSPOT_COLOR) }}
    >
      {number}
    </span>
  );
}

/** A hotspot's title and description, as the reader sees them on pointing at it. */
export function HotspotPopover({
  hotspot,
  number,
  style,
  anchor,
}: {
  hotspot: HotspotGeometry;
  number: number;
  style?: AnnotationStyle;
  anchor: PopoverAnchor;
}) {
  const title = hotspot.title.trim();
  const description = hotspot.description.trim();
  return (
    <div
      role="tooltip"
      className="pointer-events-none absolute z-20 flex w-max flex-col gap-1 rounded-md border border-border bg-popover px-3 py-2 text-sm text-popover-foreground shadow-md"
      style={{ ...anchor, maxWidth: POPOVER_WIDTH }}
    >
      <span className="flex items-center gap-2 font-medium">
        <HotspotBadge number={number} style={style} />
        {title || <span className="text-muted-foreground italic">Untitled hotspot</span>}
      </span>
      {description && <p className="whitespace-pre-wrap text-muted-foreground">{description}</p>}
    </div>
  );
}
