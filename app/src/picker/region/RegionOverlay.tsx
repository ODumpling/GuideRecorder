import { useEffect, useRef, useState, type PointerEvent } from "react";
import type { Rect } from "@guiderecorder/guide-schema";
import { dragRect, isPickable, MIN_REGION_SIDE, type Point } from "@/picker/region/region";

const DIM = "rgba(0, 0, 0, 0.45)";

/**
 * The region overlay (app/region.html): drag a rectangle over the display;
 * releasing, or Return, picks it once it is at least 40 × 40; Esc cancels.
 * `rect` is in CSS px, which are the display's points.
 */
export function RegionOverlay({ onFinish }: { onFinish(result: { rect: Rect } | null): Promise<void> }) {
  const root = useRef<HTMLDivElement>(null);
  const start = useRef<Point | null>(null);
  const finished = useRef(false);
  const [rect, setRect] = useState<Rect | null>(null);
  const [tooSmall, setTooSmall] = useState(false);

  const finish = (result: { rect: Rect } | null) => {
    if (finished.current) return;
    finished.current = true;
    onFinish(result).catch((error) => {
      console.error("finishing the region pick failed:", error);
      finished.current = false;
    });
  };

  useEffect(() => {
    root.current?.focus();
  }, []);

  // Below the menu bar; moved to the bottom while the selection reaches the hint's band.
  const hintAtBottom = !!rect && rect.y < window.innerHeight * 0.15 + 60 && rect.y + rect.height > window.innerHeight * 0.15;

  const point = (e: PointerEvent): Point => ({ x: e.clientX, y: e.clientY });
  const bounds = () => ({ width: window.innerWidth, height: window.innerHeight });

  return (
    <div
      ref={root}
      tabIndex={-1}
      aria-label="Select a region to record"
      className="fixed inset-0 cursor-crosshair overflow-hidden outline-none select-none"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        start.current = point(e);
        setRect(dragRect(start.current, start.current, bounds()));
        setTooSmall(false);
      }}
      onPointerMove={(e) => {
        if (start.current) setRect(dragRect(start.current, point(e), bounds()));
      }}
      onPointerUp={(e) => {
        if (!start.current) return;
        const drawn = dragRect(start.current, point(e), bounds());
        start.current = null;
        // A click without a drag isn't worth a hint.
        const dragged = drawn.width > 0 || drawn.height > 0;
        if (isPickable(drawn)) {
          setRect(drawn);
          finish({ rect: drawn });
        } else {
          setRect(null);
          setTooSmall(dragged);
        }
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          finish(null);
        } else if (e.key === "Enter" && isPickable(rect)) {
          e.preventDefault();
          start.current = null;
          finish({ rect });
        }
      }}
    >
      {rect ? (
        <>
          <div
            aria-label="Selected region"
            className="pointer-events-none absolute border-2 border-brand"
            style={{
              left: rect.x,
              top: rect.y,
              width: rect.width,
              height: rect.height,
              boxShadow: `0 0 0 100vmax ${DIM}`,
              outline: "1px solid rgba(0, 0, 0, 0.5)",
            }}
          />
          <span
            aria-label="Region size"
            className="pointer-events-none absolute rounded bg-brand px-1.5 py-0.5 font-mono text-xs text-brand-foreground tabular-nums"
            style={{
              left: rect.x,
              // Below the selection, or inside its bottom edge near the screen's.
              top: rect.y + rect.height + 24 <= window.innerHeight ? rect.y + rect.height + 6 : rect.y + rect.height - 24,
            }}
          >
            {rect.width} × {rect.height}
          </span>
        </>
      ) : (
        <div className="pointer-events-none absolute inset-0" style={{ background: DIM }} />
      )}
      <p
        role="status"
        className={`pointer-events-none absolute left-1/2 -translate-x-1/2 rounded-lg ${hintAtBottom ? "bottom-[15vh]" : "top-[15vh]"} border border-border bg-popover px-4 py-2 text-center text-sm text-popover-foreground shadow-lg`}
      >
        {tooSmall
          ? `Too small: drag at least ${MIN_REGION_SIDE} × ${MIN_REGION_SIDE}. Esc to cancel.`
          : "Drag to select the area to record. Release or press Return to start, Esc to cancel."}
      </p>
    </div>
  );
}
