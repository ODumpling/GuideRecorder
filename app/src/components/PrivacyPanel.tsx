import { Check, EyeOff, Info } from "lucide-react";
import {
  acceptBlurSuggestion,
  acceptBlurSuggestions,
  keepBlurSuggestion,
  keepBlurSuggestions,
  type SuggestionRef,
} from "@/annotations/commands";
import { Button } from "@/components/ui/button";
import type { Command } from "@/editor/commands";
import { editorStore, useEditor } from "@/editor/editor";
import { inspectorStore } from "@/editor/inspector";
import {
  findings,
  kindKey,
  kindLabel,
  pendingOf,
  REVIEW_LABELS,
  scanStatus,
  sourceLabel,
  type Finding,
} from "@/editor/privacy";
import type { Guide, Step } from "@/lib/types";
import { ulid } from "@/lib/ulid";
import { cn } from "@/lib/utils";

const HEADING = "text-[11px] font-semibold tracking-wide text-muted-foreground uppercase";

function run(build: (guide: Guide) => Command): void {
  const doc = editorStore.getState().doc;
  if (!doc) return;
  inspectorStore.getState().focus(null);
  try {
    editorStore.getState().dispatch(build(doc.guide));
  } catch (error) {
    console.error("privacy review failed:", error);
  }
}

const refOf = ({ stepId, index }: Finding): SuggestionRef => ({ stepId, index });

function FindingIcon({ reviewed }: { reviewed: boolean }) {
  return (
    <span
      aria-hidden
      className={cn(
        "grid size-7 shrink-0 place-items-center rounded-lg",
        reviewed
          ? "bg-[color-mix(in_oklab,var(--status-success)_16%,transparent)] text-[var(--status-success)]"
          : "bg-[color-mix(in_oklab,var(--status-warning)_16%,transparent)] text-[var(--status-warning)]",
      )}
    >
      {reviewed ? <Check className="size-4" /> : <EyeOff className="size-4" />}
    </span>
  );
}

/** The step's pending findings, numbered as on the screenshot: Blur it or Keep. */
function ThisStep({ step, all, readOnly }: { step: Step; all: Finding[]; readOnly: boolean }) {
  const pending = pendingOf(step);
  const focus = inspectorStore.getState().focus;
  if (!pending.length) {
    const any = (step.aiState.redactionSuggestions ?? []).length > 0;
    return <p className="text-sm text-muted-foreground">{any ? "Everything on this step is reviewed." : "Nothing found on this step."}</p>;
  }
  return (
    <ul className="flex flex-col gap-2" aria-label="Blur suggestions">
      {pending.map((finding, i) => {
        const { suggestion } = finding;
        const sameKind = all.filter((f) => !f.suggestion.review && kindKey(f.suggestion) === kindKey(suggestion));
        return (
          <li
            key={finding.index}
            className="flex gap-2.5 rounded-lg border border-border bg-card p-2.5 text-card-foreground"
            onPointerEnter={() => focus(refOf(finding))}
            onPointerLeave={() => focus(null)}
            onFocus={() => focus(refOf(finding))}
            onBlur={() => focus(null)}
          >
            <FindingIcon reviewed={false} />
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="flex items-baseline gap-1.5 text-[13px] font-semibold">
                <span className="text-xs text-[var(--status-warning)] tabular-nums">{i + 1}</span>
                <span className="truncate">{kindLabel(suggestion)}</span>
              </span>
              <span className="truncate text-xs text-muted-foreground">
                {[suggestion.preview, sourceLabel(suggestion)].filter(Boolean).join(" · ")}
              </span>
              {!readOnly && (
                <div className="mt-1.5 flex flex-wrap gap-1">
                  <Button
                    size="sm"
                    aria-label={`Blur it, suggestion ${i + 1}`}
                    title="Blur this area"
                    onClick={() => run((guide) => acceptBlurSuggestion(guide, step.id, finding.index, ulid()))}
                  >
                    Blur it
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    aria-label={`Keep, suggestion ${i + 1}`}
                    title="Leave this area as it is"
                    onClick={() => run((guide) => keepBlurSuggestion(guide, step.id, finding.index))}
                  >
                    Keep
                  </Button>
                  {sameKind.length > 1 && (
                    <Button
                      size="sm"
                      variant="ghost"
                      title={`Leave every ${kindLabel(suggestion).toLowerCase()} in the guide as it is`}
                      onClick={() => run((guide) => keepBlurSuggestions(guide, sameKind.map(refOf)))}
                    >
                      Keep all {sameKind.length} of this kind
                    </Button>
                  )}
                </div>
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}

/** Every finding of the guide in step order, with what was decided; a click goes to its step. */
function WholeGuide({ all, selectedId }: { all: Finding[]; selectedId: string }) {
  if (!all.length) return <p className="text-sm text-muted-foreground">Nothing found in this guide.</p>;
  return (
    <ul className="-mx-2 flex flex-col gap-0.5" aria-label="All findings">
      {all.map((finding) => {
          const review = finding.suggestion.review ?? "pending";
          return (
            <li key={`${finding.stepId}:${finding.index}`}>
              <button
                type="button"
                aria-current={finding.stepId === selectedId || undefined}
                onClick={() => editorStore.getState().select(finding.stepId)}
                className={cn(
                  "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left hover:bg-accent",
                  finding.stepId === selectedId && "bg-accent/60",
                )}
              >
                <FindingIcon reviewed={review !== "pending"} />
                <span className="flex min-w-0 flex-col">
                  <span className="truncate text-[13px] font-medium">
                    {kindLabel(finding.suggestion)}
                    {finding.suggestion.preview && ` ${finding.suggestion.preview}`}
                  </span>
                  <span className={cn("text-xs text-muted-foreground", review === "pending" && "text-[var(--status-warning)]")}>
                    Step {finding.step} · {REVIEW_LABELS[review]}
                  </span>
                </span>
              </button>
            </li>
          );
      })}
    </ul>
  );
}

/**
 * The Privacy tab: this step's blur suggestions to blur or keep, every
 * finding of the guide, which steps were scanned, and bulk actions.
 */
export function PrivacyPanel({
  step,
  readOnly,
  onSettings,
}: {
  step: Step;
  readOnly: boolean;
  onSettings?: () => void;
}) {
  const items = useEditor((s) => s.doc?.guide.items ?? []);
  const all = findings(items);
  const pending = all.filter((f) => !f.suggestion.review);
  const status = scanStatus(items);
  return (
    <div className="flex flex-col gap-5">
      <section className="flex flex-col gap-2">
        <h2 className={HEADING}>This step</h2>
        <ThisStep step={step} all={all} readOnly={readOnly} />
      </section>
      {!readOnly && pending.length > 1 && (
        <Button
          size="sm"
          variant="outline"
          className="self-start"
          title="Blur every area still to review, on every step"
          onClick={() => run((guide) => acceptBlurSuggestions(guide, pending.map(refOf), pending.map(() => ulid())))}
        >
          Blur all {pending.length} pending
        </Button>
      )}
      <section className="flex flex-col gap-2">
        <h2 className={HEADING}>Whole guide</h2>
        <WholeGuide all={all} selectedId={step.id} />
      </section>
      {status && (
        <p className="flex gap-2 rounded-lg bg-muted/60 p-2.5 text-xs text-muted-foreground" data-testid="scan-status">
          <Info aria-hidden className="mt-px size-3.5 shrink-0" />
          {status}
        </p>
      )}
      {onSettings && (
        <Button size="sm" variant="ghost" className="-mx-2 self-start text-muted-foreground" onClick={onSettings}>
          Add words or patterns to find…
        </Button>
      )}
    </div>
  );
}
