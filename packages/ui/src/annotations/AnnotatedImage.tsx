// A step's image with its annotations, read-only, drawn as the export
// draws them. The web shows steps with it; the app edits them with its
// annotation editor instead.

import { useEffect, useMemo, useRef, useState } from "react";
import type { Annotation, StepImage } from "@guiderecorder/guide-schema";
import { activeCrop, cropFits, drawOrder, hotspotNumbers, imageView } from "./geometry.ts";
import { CropMask, Shape } from "./Shapes.tsx";

type Size = { width: number; height: number };

/** An image's natural size once it loads; `failed` if it can't be decoded. */
export function useImageSize(url: string | null): { size: Size | null; failed: boolean } {
  const [result, setResult] = useState<{ url: string; size: Size | null } | null>(null);
  useEffect(() => {
    if (!url) return;
    let cancelled = false;
    const image = new Image();
    image.onload = () => {
      if (!cancelled) setResult({ url, size: { width: image.naturalWidth, height: image.naturalHeight } });
    };
    image.onerror = () => {
      if (!cancelled) setResult({ url, size: null });
    };
    image.src = url;
    return () => {
      cancelled = true;
    };
  }, [url]);
  const current = result && result.url === url ? result : null;
  return { size: current?.size ?? null, failed: !!current && !current.size };
}

/**
 * The step's image as the guide shows it (its crop or the full screenshot)
 * with the annotations over it. `src` gives the URL of a bundle-relative
 * image path, or null if there is none.
 */
export function AnnotatedImage({
  image,
  annotations,
  src,
  label = "Step screenshot",
}: {
  image: StepImage;
  annotations: Annotation[];
  src(path: string): string | null;
  label?: string;
}) {
  // A crop whose size doesn't fit its `cropRect` shows the full screenshot.
  const [cropRejected, setCropRejected] = useState(false);
  const shown = imageView(image, cropRejected);
  const url = src(shown.path);
  const { size, failed } = useImageSize(url);
  const svg = useRef<SVGSVGElement>(null);
  const [screenWidth, setScreenWidth] = useState(0);

  const misfit = !!shown.rect && !!size && !cropFits(shown.rect, size);
  useEffect(() => {
    if (misfit) setCropRejected(true);
  }, [misfit]);

  const view = useMemo(() => shown.rect ?? (size && { x: 0, y: 0, ...size }), [shown.rect, size]);
  const backdrop = useMemo(() => (url && view ? { url, rect: view } : null), [url, view]);
  const ready = !!backdrop && !misfit;
  useEffect(() => {
    const element = svg.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setScreenWidth(entry!.contentRect.width));
    observer.observe(element);
    return () => observer.disconnect();
  }, [ready]);

  if (!ready || !view || !url) {
    return (
      <div className="flex min-h-48 items-center justify-center rounded-xl border border-border bg-muted p-6 text-sm text-muted-foreground">
        {!url ? "No image" : failed ? "The screenshot can't be shown." : "Loading image…"}
      </div>
    );
  }
  const crop = activeCrop(annotations);
  const numbers = hotspotNumbers(annotations);
  // Image pixels per screen pixel, for sizes the export keeps on screen.
  const unit = screenWidth ? view.width / screenWidth : 1;
  return (
    <div className="flex items-center justify-center overflow-hidden rounded-xl border border-border bg-muted shadow-lg">
      <svg
        ref={svg}
        role="img"
        aria-label={label}
        viewBox={`${view.x} ${view.y} ${view.width} ${view.height}`}
        className="block max-h-[60vh] w-full select-none"
        style={{ aspectRatio: `${view.width} / ${view.height}` }}
      >
        <image href={url} x={view.x} y={view.y} width={view.width} height={view.height} preserveAspectRatio="none" />
        {/* The exporter's order: redactions last, over everything but hotspots. */}
        {drawOrder(annotations).map((annotation) => (
          <Shape
            key={annotation.id}
            annotation={annotation}
            backdrop={backdrop}
            number={numbers.get(annotation.id)}
            unit={unit}
          />
        ))}
        {crop?.type === "crop" && <CropMask view={view} rect={crop.geometry} />}
      </svg>
    </div>
  );
}
