// Draws annotations in SVG, in full-screenshot pixels, following the
// exporter's rules (core `export/annotate.rs`) so the editor previews the
// export. Blur and pixelate are previews only: the export burns the real
// redaction into the pixels.

import { useEffect, useState } from "react";
import { colorOr } from "@/annotations/color";
import {
  arrowShape,
  maskOutside,
  blurAmount,
  calloutEdge,
  CALLOUT_TEXT,
  DEFAULT_CALLOUT_FILL,
  DEFAULT_CALLOUT_STROKE,
  DEFAULT_COLOR,
  DEFAULT_FONT_SIZE,
  DEFAULT_HIGHLIGHT,
  DEFAULT_HIGHLIGHT_OPACITY,
  DEFAULT_STROKE,
  LINE_HEIGHT,
  strokeWidth,
  type View,
} from "@/annotations/geometry";
import type { Annotation, Rect } from "@/lib/types";

/** The image under the layer: its URL and where it sits, in full pixels. */
export interface Backdrop {
  url: string;
  rect: Rect;
}

function opacity(annotation: Annotation, fallback?: number): number | undefined {
  const value = annotation.style?.opacity;
  const chosen = value !== undefined && Number.isFinite(value) ? value : fallback;
  return chosen === undefined ? undefined : Math.min(Math.max(chosen, 0), 1);
}

const valid = (r: Rect) =>
  [r.x, r.y, r.width, r.height].every(Number.isFinite) && r.width > 0 && r.height > 0;

/** A region of the backdrop drawn in blocks, like the exporter's pixelate. */
function Pixelated({ rect, block, backdrop }: { rect: Rect; block: number; backdrop: Backdrop }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    const image = new Image();
    image.onload = () => {
      if (cancelled) return;
      // The region in the backdrop image's own pixels.
      const sx = ((rect.x - backdrop.rect.x) * image.naturalWidth) / backdrop.rect.width;
      const sy = ((rect.y - backdrop.rect.y) * image.naturalHeight) / backdrop.rect.height;
      const sw = (rect.width * image.naturalWidth) / backdrop.rect.width;
      const sh = (rect.height * image.naturalHeight) / backdrop.rect.height;
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.ceil(rect.width / block));
      canvas.height = Math.max(1, Math.ceil(rect.height / block));
      const context = canvas.getContext("2d");
      if (!context) return;
      context.imageSmoothingEnabled = true;
      context.drawImage(image, sx, sy, sw, sh, 0, 0, canvas.width, canvas.height);
      setUrl(canvas.toDataURL());
    };
    image.src = backdrop.url;
    return () => {
      cancelled = true;
    };
  }, [rect.x, rect.y, rect.width, rect.height, block, backdrop.url, backdrop.rect]);
  if (!url) return null;
  return (
    <image
      href={url}
      x={rect.x}
      y={rect.y}
      width={rect.width}
      height={rect.height}
      preserveAspectRatio="none"
      style={{ imageRendering: "pixelated" }}
    />
  );
}

function Redaction({ annotation, backdrop }: { annotation: Annotation & { geometry: Rect }; backdrop: Backdrop | null }) {
  const rect = annotation.geometry;
  const amount = blurAmount(annotation);
  const id = `redact-${annotation.id}`;
  return (
    <g>
      {backdrop &&
        (annotation.style?.blurMode === "pixelate" ? (
          <Pixelated rect={rect} block={amount} backdrop={backdrop} />
        ) : (
          <>
            <clipPath id={`${id}-clip`}>
              <rect x={rect.x} y={rect.y} width={rect.width} height={rect.height} />
            </clipPath>
            <filter id={`${id}-blur`} filterUnits="userSpaceOnUse" x={rect.x} y={rect.y} width={rect.width} height={rect.height}>
              {/* Three box passes of radius r spread about as far as a Gaussian of σ = r. */}
              <feGaussianBlur stdDeviation={amount} edgeMode="duplicate" />
            </filter>
            <g clipPath={`url(#${id}-clip)`}>
              <image
                href={backdrop.url}
                x={backdrop.rect.x}
                y={backdrop.rect.y}
                width={backdrop.rect.width}
                height={backdrop.rect.height}
                preserveAspectRatio="none"
                filter={`url(#${id}-blur)`}
              />
            </g>
          </>
        ))}
      {/* Marks the region as a redaction in the editor only. */}
      <rect
        x={rect.x}
        y={rect.y}
        width={rect.width}
        height={rect.height}
        fill="none"
        stroke="#8e8e93"
        strokeWidth={1}
        strokeDasharray="6 4"
        vectorEffect="non-scaling-stroke"
      />
    </g>
  );
}

/** One annotation as the export draws it. Crops are drawn by the layer. */
export function Shape({ annotation, backdrop }: { annotation: Annotation; backdrop: Backdrop | null }) {
  const style = annotation.style ?? {};
  switch (annotation.type) {
    case "arrow": {
      const sw = strokeWidth(style.strokeWidth, DEFAULT_STROKE);
      const shape = arrowShape(annotation.geometry, sw);
      if (!shape) return null;
      const color = colorOr(style.color, DEFAULT_COLOR);
      const { from } = annotation.geometry;
      return (
        <g opacity={opacity(annotation)}>
          <line
            x1={from.x}
            y1={from.y}
            x2={shape.shaftEnd.x}
            y2={shape.shaftEnd.y}
            stroke={color}
            strokeWidth={sw}
            strokeLinecap="round"
          />
          <polygon points={shape.head.map((p) => `${p.x},${p.y}`).join(" ")} fill={color} />
        </g>
      );
    }
    case "box": {
      const r = annotation.geometry;
      if (!valid(r)) return null;
      const sw = strokeWidth(style.strokeWidth, DEFAULT_STROKE);
      return (
        <rect
          opacity={opacity(annotation)}
          x={r.x}
          y={r.y}
          width={r.width}
          height={r.height}
          rx={sw}
          fill={style.fill === undefined ? "none" : colorOr(style.fill, "none")}
          stroke={colorOr(style.color, DEFAULT_COLOR)}
          strokeWidth={sw}
        />
      );
    }
    case "highlight": {
      const r = annotation.geometry;
      if (!valid(r)) return null;
      return (
        <rect
          opacity={opacity(annotation, DEFAULT_HIGHLIGHT_OPACITY)}
          x={r.x}
          y={r.y}
          width={r.width}
          height={r.height}
          fill={colorOr(style.fill ?? style.color, DEFAULT_HIGHLIGHT)}
        />
      );
    }
    case "callout": {
      const g = annotation.geometry;
      if (!valid(g)) return null;
      const sw = strokeWidth(style.strokeWidth, DEFAULT_CALLOUT_STROKE);
      const stroke = colorOr(style.color, DEFAULT_COLOR);
      const fontSize =
        style.fontSize !== undefined && Number.isFinite(style.fontSize) && style.fontSize > 0
          ? Math.min(style.fontSize, 512)
          : DEFAULT_FONT_SIZE;
      const pad = Math.max(fontSize * 0.5, sw + 2);
      const edge = g.anchor && calloutEdge(g, g.anchor);
      return (
        <g opacity={opacity(annotation)}>
          {g.anchor && edge && (
            <>
              <line
                x1={edge.x}
                y1={edge.y}
                x2={g.anchor.x}
                y2={g.anchor.y}
                stroke={stroke}
                strokeWidth={sw}
                strokeLinecap="round"
              />
              <circle cx={g.anchor.x} cy={g.anchor.y} r={sw * 1.5 + 1} fill={stroke} />
            </>
          )}
          <rect
            x={g.x}
            y={g.y}
            width={g.width}
            height={g.height}
            rx={sw * 2}
            fill={colorOr(style.fill, DEFAULT_CALLOUT_FILL)}
            stroke={stroke}
            strokeWidth={sw}
          />
          <foreignObject x={g.x} y={g.y} width={g.width} height={g.height}>
            <div
              style={{
                padding: pad,
                fontSize,
                lineHeight: LINE_HEIGHT,
                color: CALLOUT_TEXT,
                fontFamily: "Lato, ui-sans-serif, system-ui, sans-serif",
                overflow: "hidden",
                height: "100%",
                boxSizing: "border-box",
                whiteSpace: "pre-wrap",
                overflowWrap: "break-word",
              }}
            >
              {g.text}
            </div>
          </foreignObject>
        </g>
      );
    }
    case "blur":
      return valid(annotation.geometry) ? <Redaction annotation={annotation} backdrop={backdrop} /> : null;
    case "crop":
      return null;
  }
}

/** The view outside the active crop, dimmed, with the crop outlined. */
export function CropMask({ view, rect }: { view: View; rect: Rect }) {
  return (
    <g pointerEvents="none">
      <path
        d={maskOutside(view, rect)}
        fill="black"
        fillOpacity={0.45}
        fillRule="evenodd"
      />
      <rect
        x={rect.x}
        y={rect.y}
        width={rect.width}
        height={rect.height}
        fill="none"
        stroke="white"
        strokeWidth={1.5}
        strokeDasharray="8 5"
        vectorEffect="non-scaling-stroke"
      />
    </g>
  );
}
