import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, CircleAlert, LoaderCircle } from "lucide-react";
import type { Guide, Step } from "@guiderecorder/guide-schema";
import { Button } from "@guiderecorder/ui/button";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";
import { cn } from "@guiderecorder/ui/utils";
import {
  markdownPreviewDocument,
  markdownPreviewHtml,
  pageLabel,
  previewRequest,
  PreviewScheduler,
  type PreviewState,
} from "@/export/preview";
import { errorMessage, type ExportApi } from "@/lib/ipc";
import type { ExportFormat, ExportPreviewData, MotionOptions, PaperSize, ScormOptions, SkillOptions } from "@/lib/types";

const SHADOW = "shadow-[0_2px_4px_rgb(0_0_0/0.06),0_12px_40px_rgb(0_0_0/0.16)]";

/** Width the HTML and Markdown previews are laid out at, in CSS pixels, before scaling to fit. */
const PAGE_WIDTH = 800;

/**
 * An exported page in a sandboxed frame, laid out at `PAGE_WIDTH` and
 * scaled to fit. `scripts` lets the walkthrough's own script run (it can
 * still reach nothing: no same origin, and the page's CSP).
 */
function PageFrame({ html, title, scripts }: { html: string; title: string; scripts: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(0.44);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => el.clientWidth && setScale(el.clientWidth / PAGE_WIDTH));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  return (
    <div ref={box} className="relative min-h-0 flex-1 overflow-hidden bg-white">
      <iframe
        title={title}
        srcDoc={html}
        sandbox={scripts ? "allow-scripts" : ""}
        className="absolute top-0 left-0 origin-top-left border-0"
        style={{ width: PAGE_WIDTH, height: `${100 / scale}%`, transform: `scale(${scale})` }}
      />
    </div>
  );
}

/** A browser window around an HTML or Markdown preview. */
function BrowserChrome({ name, children }: { name: string; children: React.ReactNode }) {
  return (
    <div className={cn("flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg bg-white", SHADOW)}>
      <div className="flex h-6 shrink-0 items-center gap-1 border-b border-[#ececef] bg-[#f4f4f6] px-2.5">
        {["#ff5f57", "#febc2e", "#28c840"].map((c) => (
          <span key={c} className="size-2 rounded-full" style={{ background: c }} />
        ))}
        <span className="mx-auto truncate text-[9px] text-[#888]">{name}</span>
      </div>
      {children}
    </div>
  );
}

/** "Page 1 of 4" with arrows, and a thumbnail strip. */
function Pager({
  noun,
  index,
  count,
  thumbnails,
  onSelect,
}: {
  noun: string;
  index: number;
  count: number;
  thumbnails: string[];
  onSelect(index: number): void;
}) {
  return (
    <div className="flex shrink-0 flex-col gap-2 pt-3">
      <div className="flex items-center justify-center gap-2">
        <Button
          variant="ghost"
          size="icon"
          aria-label={`Previous ${noun.toLowerCase()}`}
          disabled={index === 0}
          onClick={() => onSelect(index - 1)}
        >
          <ChevronLeft />
        </Button>
        <span className="min-w-24 text-center text-xs text-muted-foreground tabular-nums" aria-live="polite">
          {pageLabel(noun, index, count)}
        </span>
        <Button
          variant="ghost"
          size="icon"
          aria-label={`Next ${noun.toLowerCase()}`}
          disabled={index >= thumbnails.length - 1}
          onClick={() => onSelect(index + 1)}
        >
          <ChevronRight />
        </Button>
      </div>
      {thumbnails.length > 1 && (
        <div className="flex gap-1.5 overflow-x-auto pb-1">
          {thumbnails.map((src, i) => (
            <button
              key={i}
              type="button"
              aria-label={pageLabel(noun, i, count)}
              aria-current={i === index ? "page" : undefined}
              onClick={() => onSelect(i)}
              className={cn(
                "h-12 shrink-0 overflow-hidden rounded-[3px] border outline-none focus-visible:ring-2 focus-visible:ring-ring",
                i === index ? "border-foreground ring-1 ring-foreground" : "border-border opacity-70 hover:opacity-100",
              )}
            >
              <img src={src} alt="" className="h-full w-auto" />
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** A slide as the deck lays it out: title, the step image (blurs burned in), description. */
function Slide({ step, number, image }: { step: Step | undefined; number: number; image: string | undefined }) {
  const title = step ? markdownToPlainText(step.title) || `Step ${number}` : `Step ${number}`;
  const description = step ? markdownToPlainText(step.description) : "";
  return (
    <div
      className={cn("flex w-full flex-col gap-2 rounded-[3px] bg-white p-4 text-[#1d1d1f]", SHADOW)}
      style={{ aspectRatio: "16 / 9" }}
    >
      <p className="truncate text-[13px] font-bold">
        {number}. {title}
      </p>
      <div className="flex min-h-0 flex-1 gap-3">
        <div className="flex min-w-0 flex-[2] items-center justify-center">
          {image && <img src={image} alt="" className="max-h-full max-w-full border border-[#d1d1d6]" />}
        </div>
        {description && <p className="line-clamp-6 flex-1 text-[8px] leading-normal">{description}</p>}
      </div>
    </div>
  );
}

function MarkdownPreview({ markdown, images }: { markdown: string; images: { link: string; src: string }[] }) {
  const [source, setSource] = useState(false);
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div role="radiogroup" aria-label="Markdown view" className="flex shrink-0 justify-center gap-1">
        {[
          { label: "Rendered", on: false },
          { label: "Source", on: true },
        ].map((v) => (
          <button
            key={v.label}
            type="button"
            role="radio"
            aria-checked={source === v.on}
            onClick={() => setSource(v.on)}
            className={cn(
              "rounded-md px-2.5 py-0.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring",
              source === v.on ? "bg-foreground text-background" : "text-muted-foreground hover:bg-foreground/[0.06]",
            )}
          >
            {v.label}
          </button>
        ))}
      </div>
      <BrowserChrome name="guide.md">
        {source ? (
          <pre className="min-h-0 flex-1 overflow-auto p-3 font-mono text-[9.5px] leading-normal break-words whitespace-pre-wrap text-[#1f2328]">
            {markdown}
          </pre>
        ) : (
          <PageFrame
            title="Export preview"
            html={markdownPreviewDocument(markdownPreviewHtml(markdown, images))}
            scripts={false}
          />
        )}
      </BrowserChrome>
    </div>
  );
}

/** The preview by kind; `index` is the page, slide or scene shown. */
function PreviewBody({
  preview,
  guide,
  format,
  index,
  onSelect,
}: {
  preview: ExportPreviewData;
  guide: Guide;
  format: ExportFormat;
  index: number;
  onSelect(index: number): void;
}) {
  switch (preview.kind) {
    case "html":
      return (
        <div className="flex min-h-0 flex-1 flex-col gap-2">
          <BrowserChrome name="guide.html">
            <PageFrame html={preview.html} title="Export preview" scripts={format === "walkthrough" || format === "scorm"} />
          </BrowserChrome>
          {(format === "walkthrough" || format === "scorm") && (
            <p className="shrink-0 text-center text-xs text-muted-foreground">Click through it as your readers will.</p>
          )}
        </div>
      );
    case "markdown":
      return <MarkdownPreview markdown={preview.markdown} images={preview.images} />;
    case "pages": {
      const page = preview.pages[index];
      return (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1 items-start justify-center">
            {page && (
              <img
                src={page}
                alt={pageLabel("Page", index, preview.pageCount)}
                className={cn("max-h-full max-w-full rounded-[3px] bg-white object-contain", SHADOW)}
              />
            )}
          </div>
          {preview.approximate && (
            <p className="shrink-0 pt-2 text-center text-xs text-muted-foreground">
              Word lays out its own pages; this is the PDF’s layout.
            </p>
          )}
          {preview.pageCount > preview.pages.length && (
            <p className="shrink-0 pt-1 text-center text-xs text-muted-foreground">
              Showing the first {preview.pages.length} pages.
            </p>
          )}
          <Pager
            noun="Page"
            index={index}
            count={preview.pageCount}
            thumbnails={preview.pages}
            onSelect={onSelect}
          />
        </div>
      );
    }
    case "slides": {
      const steps = guide.items.filter((i): i is Step => i.type === "step");
      return (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1 items-start">
            <Slide step={steps[index]} number={index + 1} image={preview.images[index]} />
          </div>
          {preview.images.length > 0 && (
            <Pager
              noun="Slide"
              index={index}
              count={preview.images.length}
              thumbnails={preview.images}
              onSelect={onSelect}
            />
          )}
        </div>
      );
    }
    case "frames": {
      const frame = preview.frames[index];
      return (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1 items-start justify-center">
            {frame && (
              <img
                src={frame}
                alt={pageLabel("Scene", index, preview.sceneCount)}
                className={cn("max-h-full max-w-full bg-[#18181b] object-contain", SHADOW)}
              />
            )}
          </div>
          <Pager
            noun="Scene"
            index={index}
            count={preview.sceneCount}
            thumbnails={preview.frames}
            onSelect={onSelect}
          />
        </div>
      );
    }
  }
}

/** How many pages, slides or scenes a preview can show. */
function shown(preview: ExportPreviewData | null): number {
  if (!preview) return 0;
  switch (preview.kind) {
    case "pages":
      return preview.pages.length;
    case "slides":
      return preview.images.length;
    case "frames":
      return preview.frames.length;
    default:
      return 1;
  }
}

/**
 * The export as the exporter makes it, from the saved guide: the real
 * logo, accent, paper, page breaks and layout, with blurs burned in. Made
 * again (after a short pause) whenever the options change.
 */
export function ExportPreview({
  guide,
  format,
  paper,
  logo,
  accent,
  reviewDetails,
  motion,
  skill,
  scorm,
  audience,
  api,
  flush,
}: {
  guide: Guide;
  format: ExportFormat;
  paper: PaperSize;
  logo: string | null;
  accent: string;
  reviewDetails: boolean;
  motion: MotionOptions;
  /** The agent skill options; the replay script (replay.js, or replay.ps1 for a Windows guide) adds a line to SKILL.md. */
  skill?: SkillOptions;
  /** The SCORM options; the page shows the quiz. */
  scorm?: ScormOptions;
  /** Only the sections for this audience; null or absent: everyone. */
  audience?: string | null;
  api: ExportApi;
  /** Saves the editor's changes, so the preview shows them. */
  flush(): Promise<boolean>;
}) {
  const [state, setState] = useState<PreviewState>({ preview: null, key: null, loading: false, error: null });
  const [index, setIndex] = useState(0);
  const scheduler = useRef<PreviewScheduler | null>(null);
  const deps = useRef({ api, flush });
  deps.current = { api, flush };

  useEffect(() => {
    const s = new PreviewScheduler(
      async (r) => {
        await deps.current.flush();
        return deps.current.api.previewExport(r.guideId, r.format, r.branding, r.paper, r.reviewDetails, r.motion, r.skill, r.scorm, r.audience);
      },
      setState,
      errorMessage,
    );
    scheduler.current = s;
    return () => s.dispose();
  }, []);

  const request = previewRequest({ guideId: guide.id, format, logo, accent, paper, reviewDetails, motion, skill, scorm, audience });
  const key = request && JSON.stringify(request);
  useEffect(() => {
    scheduler.current?.request(request);
    // `key` stands for the request.
  }, [key]);

  // A new preview starts on its first page, and a shorter one keeps the index in range.
  const count = shown(state.preview);
  useEffect(() => setIndex(0), [state.key]);
  const page = Math.min(index, Math.max(0, count - 1));

  return (
    <div aria-label="Export preview" aria-busy={state.loading} className="relative flex h-full w-[22rem] shrink-0 flex-col pb-4">
      {state.preview && (
        <div className={cn("flex min-h-0 flex-1 flex-col transition-opacity", state.loading && "opacity-60")}>
          <PreviewBody
            preview={state.preview}
            guide={guide}
            format={format}
            index={page}
            onSelect={setIndex}
          />
        </div>
      )}
      {state.loading && (
        <div className="pointer-events-none absolute inset-x-0 top-1/3 flex justify-center">
          <span className="flex items-center gap-2 rounded-full bg-card px-3 py-1.5 text-xs text-muted-foreground shadow">
            <LoaderCircle className="size-3.5 animate-spin" /> Updating preview…
          </span>
        </div>
      )}
      {state.error && !state.loading && (
        <p role="alert" className="flex shrink-0 items-start gap-2 pt-2 text-xs text-[var(--status-danger)]">
          <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
          <span>No preview: {state.error}</span>
        </p>
      )}
    </div>
  );
}
