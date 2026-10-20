import { useState } from "react";
import {
  arrowShape,
  blurAmount,
  CALLOUT_TEXT,
  calloutEdge,
  chooseImage,
  colorOr,
  cropFits,
  DEFAULT_CALLOUT_FILL,
  DEFAULT_CALLOUT_STROKE,
  DEFAULT_COLOR,
  DEFAULT_FONT_SIZE,
  DEFAULT_HIGHLIGHT,
  DEFAULT_HIGHLIGHT_OPACITY,
  DEFAULT_HOTSPOT_COLOR,
  DEFAULT_STROKE,
  drawOrder,
  hotspots,
  inView,
  percentBox,
  strokeWidth,
  validRect,
  viewRect,
  type Annotation,
  type Rect,
  type ViewerStep,
} from "./viewer";

// A step's screenshot with its annotations drawn over it in SVG, in
// full-screenshot pixels, as the exporter draws them; hotspots as
// interactive markers with a numbered legend, as in the HTML export.
// Blur and pixelate cover their regions on screen; the exports burn them
// into the pixels.

export type ImageUrl = (path: string) => string | null;

interface Props {
  step: ViewerStep;
  imageUrl: ImageUrl;
  /** Alt text: the step's title as plain text. */
  alt: string;
  /** Walkthrough mode: a pulsing target on the click point that calls this. */
  onTarget?: () => void;
  targetLabel?: string;
}

const clamp01 = (v: number | null | undefined, fallback?: number) => {
  const value = v != null && Number.isFinite(v) ? v : fallback;
  return value === undefined ? undefined : Math.min(Math.max(value, 0), 1);
};

export function StepFigure({ step, imageUrl, alt, onTarget, targetLabel = "Click here to continue" }: Props) {
  const [cropRejected, setCropRejected] = useState(false);
  const [size, setSize] = useState<{ path: string; width: number; height: number } | null>(null);
  if (!step.image) return null;
  const choice = chooseImage(step.image, cropRejected);
  const url = imageUrl(choice.path);
  if (!url) {
    return (
      <div className="grid aspect-video place-content-center rounded-lg border border-dashed border-border text-sm text-muted-foreground">
        Image unavailable
      </div>
    );
  }

  const measured = size?.path === choice.path ? size : null;
  // The image's place in the full screenshot, once its size is known.
  const imageRect: Rect | null = measured ? (choice.rect ?? { x: 0, y: 0, width: measured.width, height: measured.height }) : null;

  if (!imageRect) {
    const measure = (img: HTMLImageElement) => {
      const { naturalWidth: width, naturalHeight: height } = img;
      if (!width || !height) return;
      if (choice.rect && !cropFits(choice.rect, { width, height })) setCropRejected(true);
      else setSize({ path: choice.path, width, height });
    };
    // Shown while it loads, and measured: the overlay needs its size. An
    // image server-rendered and loaded before hydration fires no `load`,
    // so the ref measures it too.
    return (
      <img
        ref={(img) => {
          if (img?.complete) measure(img);
        }}
        src={url}
        alt={alt}
        className="w-full rounded-lg border border-border"
        loading="lazy"
        onLoad={(e) => measure(e.currentTarget)}
      />
    );
  }

  const view = viewRect(imageRect, step.annotations);
  const spots = hotspots(step.annotations);
  const target = onTarget && step.point && inView(step.point, view) ? percentBox({ ...step.point, width: 0, height: 0 }, view) : null;

  return (
    <figure className="grid gap-3">
      <div className="relative overflow-visible rounded-lg border border-border" style={{ aspectRatio: `${view.width} / ${view.height}` }}>
        <svg
          viewBox={`${view.x} ${view.y} ${view.width} ${view.height}`}
          className="absolute inset-0 h-full w-full rounded-lg"
          role="img"
          aria-label={alt}
        >
          <image href={url} x={imageRect.x} y={imageRect.y} width={imageRect.width} height={imageRect.height} preserveAspectRatio="none" />
          {drawOrder(step.annotations).map((a) => (
            <Shape key={a.id} annotation={a} url={url} imageRect={imageRect} />
          ))}
        </svg>
        {spots.map(({ number, hotspot }) => {
          const box = percentBox(hotspot.geometry, view);
          const color = colorOr(hotspot.style?.color, DEFAULT_HOTSPOT_COLOR);
          const title = hotspot.geometry.title?.trim() ?? "";
          const description = hotspot.geometry.description?.trim() ?? "";
          const below = (hotspot.geometry.y - view.y) / view.height < 0.6;
          return (
            <button
              key={hotspot.id}
              type="button"
              className="group absolute rounded-[3px] border-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
              style={{ ...box, borderColor: color, background: `color-mix(in srgb, ${color} 12%, transparent)`, borderStyle: title ? "solid" : "dashed" }}
              aria-label={title ? `${number}. ${title}` : String(number)}
              aria-describedby={description ? `hotspot-${hotspot.id}` : undefined}
            >
              <span
                aria-hidden="true"
                className="absolute -top-2.5 -left-2.5 grid size-5 place-content-center rounded-full border border-white text-[11px] font-semibold text-white"
                style={{ background: color }}
              >
                {number}
              </span>
              {title || description ? (
                <span
                  role="tooltip"
                  className={`pointer-events-none absolute left-0 z-10 hidden w-64 max-w-[70vw] rounded-lg border border-border bg-card p-3 text-left text-sm text-foreground shadow-lg group-hover:block group-focus:block ${below ? "top-full mt-2" : "bottom-full mb-2"}`}
                >
                  {title ? <strong className="block">{title}</strong> : null}
                  {description ? <span className="block whitespace-pre-line text-muted-foreground">{description}</span> : null}
                </span>
              ) : null}
            </button>
          );
        })}
        {target ? (
          <button
            type="button"
            onClick={onTarget}
            aria-label={targetLabel}
            className="absolute size-9 -translate-x-1/2 -translate-y-1/2 rounded-full border-[3px] border-brand bg-brand/15 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
            style={{ left: target.left, top: target.top }}
          >
            <span aria-hidden="true" className="absolute inset-0 animate-ping rounded-full border-2 border-brand motion-reduce:animate-none" />
          </button>
        ) : null}
      </div>
      {spots.length ? (
        <ol className="grid gap-2 text-sm">
          {spots.map(({ number, hotspot }) => (
            <li key={hotspot.id} id={`hotspot-${hotspot.id}`} className="flex gap-2">
              <span
                className="grid size-5 shrink-0 place-content-center rounded-full text-[11px] font-semibold text-white"
                style={{ background: colorOr(hotspot.style?.color, DEFAULT_HOTSPOT_COLOR) }}
              >
                {number}
              </span>
              <span>
                {hotspot.geometry.title ? <strong className="block">{hotspot.geometry.title}</strong> : null}
                {hotspot.geometry.description ? <span className="block whitespace-pre-line text-muted-foreground">{hotspot.geometry.description}</span> : null}
              </span>
            </li>
          ))}
        </ol>
      ) : null}
    </figure>
  );
}

/** One annotation as the exporter draws it (core `export/annotate.rs`). */
function Shape({ annotation, url, imageRect }: { annotation: Annotation; url: string; imageRect: Rect }) {
  const style = annotation.style ?? {};
  switch (annotation.type) {
    case "arrow": {
      const sw = strokeWidth(style.strokeWidth, DEFAULT_STROKE);
      const shape = arrowShape(annotation.geometry, sw);
      if (!shape) return null;
      const color = colorOr(style.color, DEFAULT_COLOR);
      const { from } = annotation.geometry;
      return (
        <g opacity={clamp01(style.opacity)}>
          <line x1={from.x} y1={from.y} x2={shape.shaftEnd.x} y2={shape.shaftEnd.y} stroke={color} strokeWidth={sw} strokeLinecap="round" />
          <polygon points={shape.head.map((p) => `${p.x},${p.y}`).join(" ")} fill={color} />
        </g>
      );
    }
    case "box": {
      const r = annotation.geometry;
      if (!validRect(r)) return null;
      const sw = strokeWidth(style.strokeWidth, DEFAULT_STROKE);
      return (
        <rect
          opacity={clamp01(style.opacity)}
          {...r}
          rx={sw}
          fill={style.fill == null ? "none" : colorOr(style.fill, "none")}
          stroke={colorOr(style.color, DEFAULT_COLOR)}
          strokeWidth={sw}
        />
      );
    }
    case "highlight": {
      const r = annotation.geometry;
      if (!validRect(r)) return null;
      return <rect opacity={clamp01(style.opacity, DEFAULT_HIGHLIGHT_OPACITY)} {...r} fill={colorOr(style.fill ?? style.color, DEFAULT_HIGHLIGHT)} />;
    }
    case "callout": {
      const g = annotation.geometry;
      if (!validRect(g)) return null;
      const sw = strokeWidth(style.strokeWidth, DEFAULT_CALLOUT_STROKE);
      const stroke = colorOr(style.color, DEFAULT_COLOR);
      const fontSize = style.fontSize != null && Number.isFinite(style.fontSize) && style.fontSize > 0 ? Math.min(style.fontSize, 512) : DEFAULT_FONT_SIZE;
      const edge = g.anchor && calloutEdge(g, g.anchor);
      return (
        <g opacity={clamp01(style.opacity)}>
          {g.anchor && edge ? (
            <>
              <line x1={edge.x} y1={edge.y} x2={g.anchor.x} y2={g.anchor.y} stroke={stroke} strokeWidth={sw} strokeLinecap="round" />
              <circle cx={g.anchor.x} cy={g.anchor.y} r={sw * 1.5 + 1} fill={stroke} />
            </>
          ) : null}
          <rect x={g.x} y={g.y} width={g.width} height={g.height} rx={sw * 2} fill={colorOr(style.fill, DEFAULT_CALLOUT_FILL)} stroke={stroke} strokeWidth={sw} />
          <foreignObject x={g.x} y={g.y} width={g.width} height={g.height}>
            <div
              style={{
                padding: Math.max(fontSize * 0.5, sw + 2),
                fontSize,
                lineHeight: 1.25,
                color: CALLOUT_TEXT,
                height: "100%",
                boxSizing: "border-box",
                overflow: "hidden",
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
    case "blur": {
      const r = annotation.geometry;
      if (!validRect(r)) return null;
      // Pixelate is shown as a blur of the block size: a cross-origin image
      // can't be read back to draw real blocks. Either way nothing shows through.
      const amount = blurAmount(style);
      const id = `blur-${annotation.id}`;
      return (
        <g>
          <clipPath id={`${id}-clip`}>
            <rect {...r} />
          </clipPath>
          <filter id={`${id}-filter`} filterUnits="userSpaceOnUse" {...r}>
            <feGaussianBlur stdDeviation={amount} edgeMode="duplicate" />
          </filter>
          <rect {...r} fill="#8e8e93" />
          <g clipPath={`url(#${id}-clip)`}>
            <image href={url} {...imageRect} preserveAspectRatio="none" filter={`url(#${id}-filter)`} />
          </g>
        </g>
      );
    }
    default:
      return null;
  }
}
