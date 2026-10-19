import { useStepThumbnail } from "@/components/useStepThumbnail";
import { isAccent, isFolderFormat } from "@/export/export";
import type { ExportFormat, Guide, PaperSize, Step } from "@/lib/types";
import { cn } from "@/lib/utils";
import { markdownToPlainText } from "@/text/markdown";

/** Steps shown on the preview page. */
const PREVIEW_STEPS = 3;
/** The exports' accent when none is set. */
const DEFAULT_ACCENT = "#1d1c1a";

function PreviewStep({
  guideId,
  step,
  number,
  accent,
}: {
  guideId: string;
  step: Step;
  number: number;
  accent: string;
}) {
  const path = step.image.show === "crop" && step.image.crop ? step.image.crop : step.image.full;
  const image = useStepThumbnail(guideId, path || null);
  return (
    <div className="flex gap-2.5">
      <span
        className="flex size-[18px] shrink-0 items-center justify-center rounded-full text-[9px] font-bold text-white"
        style={{ background: accent }}
      >
        {number}
      </span>
      <div className="min-w-0 flex-1">
        <p className="text-[10.5px] leading-snug font-semibold">{markdownToPlainText(step.title) || "Untitled step"}</p>
        {step.description && (
          <p className="mt-0.5 line-clamp-2 text-[9.5px] leading-normal text-[#555]">
            {markdownToPlainText(step.description)}
          </p>
        )}
        {image && <img src={image} alt="" className="mt-1.5 max-h-28 rounded-[3px] border border-[#e6e3dc]" />}
      </div>
    </div>
  );
}

/**
 * A sketch of the export's first page as the options change: the accent,
 * logo, paper and review details. Not the exporter's own rendering; the
 * file is made when you export.
 */
export function ExportPreview({
  guide,
  format,
  paper,
  logo,
  accent,
  reviewDetails,
}: {
  guide: Guide;
  format: ExportFormat;
  paper: PaperSize;
  logo: string | null;
  accent: string;
  reviewDetails: boolean;
}) {
  const color = accent.trim() && isAccent(accent) ? accent.trim() : DEFAULT_ACCENT;
  const web = isFolderFormat(format);
  const steps = guide.items.filter((i): i is Step => i.type === "step");
  const review = [
    guide.version && `Version ${guide.version}`,
    guide.owner && `Owner: ${guide.owner}`,
    guide.lastReviewed && `Reviewed ${guide.lastReviewed}`,
  ].filter(Boolean);
  return (
    <div
      aria-label="Export preview"
      className={cn(
        "flex w-[22rem] shrink-0 flex-col overflow-hidden bg-white text-[#1d1c1a] shadow-[0_2px_4px_rgb(0_0_0/0.06),0_12px_40px_rgb(0_0_0/0.16)]",
        web ? "h-full rounded-lg" : "rounded-[3px]",
      )}
      style={web ? undefined : { aspectRatio: paper === "letter" ? "8.5 / 11" : "210 / 297" }}
    >
      {web && (
        <div className="flex h-6 shrink-0 items-center gap-1 border-b border-[#ececef] bg-[#f4f4f6] px-2.5">
          {["#ff5f57", "#febc2e", "#28c840"].map((c) => (
            <span key={c} className="size-2 rounded-full" style={{ background: c }} />
          ))}
          <span className="mx-auto truncate text-[9px] text-[#888]">guide.html</span>
        </div>
      )}
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden px-7 py-6">
        {logo && (
          // Where the logo goes; the file itself is read when exporting.
          <div className="flex items-center gap-1.5 text-[9px] font-semibold tracking-wider text-[#888] uppercase">
            <span className="size-3.5 rounded-[3px]" style={{ background: color }} />
            Logo
          </div>
        )}
        <div>
          <h1 className="font-serif text-[26px] leading-[1.05]">{guide.title || "Untitled"}</h1>
          <div className="mt-2 h-0.5 w-10 rounded-full" style={{ background: color }} />
        </div>
        {reviewDetails && review.length > 0 && <p className="text-[9px] text-[#777]">{review.join(" · ")}</p>}
        {guide.intro && (
          <p className="line-clamp-3 text-[10px] leading-normal text-[#444]">{markdownToPlainText(guide.intro)}</p>
        )}
        {guide.prerequisites && (
          <div className="rounded-[4px] bg-[#f6f4ef] px-2.5 py-2 text-[9.5px]">
            <p className="mb-0.5 text-[8px] font-semibold tracking-wider text-[#8a857b] uppercase">Before you start</p>
            <p className="line-clamp-2">{markdownToPlainText(guide.prerequisites)}</p>
          </div>
        )}
        {steps.slice(0, PREVIEW_STEPS).map((step, i) => (
          <PreviewStep key={step.id} guideId={guide.id} step={step} number={i + 1} accent={color} />
        ))}
      </div>
    </div>
  );
}
