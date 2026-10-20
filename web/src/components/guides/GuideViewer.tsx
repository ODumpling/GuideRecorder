import { useEffect, useMemo, useRef, useState, type ReactNode, type Ref } from "react";
import type { Guide } from "@guiderecorder/guide-schema";
import { Button, Card } from "../ui";
import { Markdown } from "./Markdown";
import { StepFigure, type ImageUrl } from "./StepFigure";
import { plainText, slides, viewerItems, type ViewerItem, type ViewerNote, type ViewerStep } from "./viewer";

// A guide for reading on the web: the React counterpart of the HTML export
// (core `export/html.rs`) and its interactive walkthrough
// (`export/walkthrough.rs`). It knows nothing about workspaces or sign-in:
// the page passes the guide and a function from image paths to URLs, so the
// library and public share links show guides the same way.

export type { ImageUrl } from "./StepFigure";
export type ViewerMode = "document" | "walkthrough";

export interface GuideViewerProps {
  /** The guide.json to show. */
  guide: Guide;
  /**
   * A URL for a bundle-relative image path (`steps/<id>.webp`), or null if
   * there is none. Called during render; keep it cheap.
   */
  imageUrl: ImageUrl;
  /** How it opens; the reader can switch. Default `document`. */
  mode?: ViewerMode;
  /** Above the title, e.g. a back link and actions. */
  header?: ReactNode;
  /** Under the summary, e.g. who changed it last. */
  meta?: ReactNode;
}

type Words = ReturnType<typeof words>;

/** The viewer's own words, by guide language, as the exporters choose theirs. */
function words(language: string, annotated: boolean) {
  const de = language.split(/[-_]/)[0]?.toLowerCase() === "de";
  return de
    ? {
        step: annotated ? "Seite" : "Schritt",
        prerequisites: "Voraussetzungen",
        notes: { tip: "Tipp", warning: "Warnung", note: "Hinweis" },
        document: "Dokument",
        walkthrough: "Schritt für Schritt",
        start: "Bevor Sie beginnen",
        begin: "Los geht’s",
        back: "Zurück",
        next: "Weiter",
        restart: "Neu starten",
        steps: "Schritte",
        of: "von",
        done: "Fertig",
        doneText: "Sie haben das Ende dieser Anleitung erreicht.",
        target: "Hier klicken, um fortzufahren",
        hint: "Klicken Sie auf die markierte Stelle oder drücken Sie → oder die Eingabetaste, um fortzufahren; ← geht zurück.",
      }
    : {
        step: annotated ? "Page" : "Step",
        prerequisites: "Prerequisites",
        notes: { tip: "Tip", warning: "Warning", note: "Note" },
        document: "Document",
        walkthrough: "Walkthrough",
        start: "Before you start",
        begin: "Start",
        back: "Back",
        next: "Next",
        restart: "Restart",
        steps: "Steps",
        of: "of",
        done: "Done",
        doneText: "You have reached the end of this guide.",
        target: "Click here to continue",
        hint: "Click the highlighted spot, or press → or Enter, to continue; ← goes back.",
      };
}

export function GuideViewer({ guide, imageUrl, mode: initialMode = "document", header, meta }: GuideViewerProps) {
  const [mode, setMode] = useState<ViewerMode>(initialMode);
  const items = useMemo(() => viewerItems(guide), [guide]);
  const w = words(guide.language ?? "en", guide.kind === "annotated");
  const steps = items.filter((i): i is ViewerStep => i.type === "step");

  return (
    <div className="mx-auto grid w-full max-w-5xl gap-6 p-6" lang={guide.language || undefined}>
      {header}
      <Card className="grid gap-3">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <h1 className="font-serif text-3xl">{guide.title}</h1>
          {steps.length ? (
            <div role="group" aria-label="View" className="inline-flex rounded-lg border border-border p-0.5 text-sm">
              {(["document", "walkthrough"] as const).map((m) => (
                <button
                  key={m}
                  type="button"
                  aria-pressed={mode === m}
                  onClick={() => setMode(m)}
                  className={`rounded-md px-3 py-1 ${mode === m ? "bg-muted font-medium" : "text-muted-foreground hover:text-foreground"}`}
                >
                  {w[m]}
                </button>
              ))}
            </div>
          ) : null}
        </div>
        {guide.summary ? <p className="text-muted-foreground">{guide.summary}</p> : null}
        {meta}
      </Card>
      {mode === "walkthrough" && steps.length ? (
        <Walkthrough guide={guide} items={items} total={steps.length} imageUrl={imageUrl} w={w} />
      ) : (
        <DocumentView guide={guide} items={items} steps={steps} imageUrl={imageUrl} w={w} />
      )}
    </div>
  );
}

function Intro({ guide, w }: { guide: Guide; w: Words }) {
  return (
    <>
      {guide.intro?.trim() ? <Markdown text={guide.intro} className="text-sm" /> : null}
      {guide.prerequisites?.trim() ? (
        <div className="grid gap-1">
          <h3 className="text-sm font-semibold">{w.prerequisites}</h3>
          <Markdown text={guide.prerequisites} className="text-sm" />
        </div>
      ) : null}
    </>
  );
}

const NOTE_STYLE = {
  tip: "border-l-success",
  warning: "border-l-destructive",
  note: "border-l-brand",
} as const;

function NoteCard({ note, w }: { note: ViewerNote; w: Words }) {
  return (
    <aside role="note" className={`rounded-xl border border-l-4 border-border bg-card p-4 text-sm ${NOTE_STYLE[note.kind]}`}>
      <p className="mb-1 text-xs font-semibold tracking-wide text-muted-foreground uppercase">{w.notes[note.kind]}</p>
      <Markdown text={note.body} />
    </aside>
  );
}

function StepTitle({ step, w, as: As = "h3", ref }: { step: ViewerStep; w: Words; as?: "h2" | "h3"; ref?: Ref<HTMLHeadingElement> }) {
  return (
    <As ref={ref} tabIndex={ref ? -1 : undefined} className={`flex items-baseline gap-3 outline-none ${As === "h2" ? "text-xl font-semibold" : "text-lg font-medium"}`}>
      <span aria-label={`${w.step} ${step.number}`} className="grid size-7 shrink-0 place-content-center rounded-full bg-brand text-sm text-brand-foreground">
        {step.number}
      </span>
      <span>{step.title.trim() ? <Markdown text={step.title} inline /> : w.step}</span>
    </As>
  );
}

function DocumentView({ guide, items, steps, imageUrl, w }: { guide: Guide; items: ViewerItem[]; steps: ViewerStep[]; imageUrl: ImageUrl; w: Words }) {
  const hasIntro = Boolean(guide.intro?.trim() || guide.prerequisites?.trim());
  return (
    <div className={steps.length > 1 ? "grid gap-6 lg:grid-cols-[minmax(0,1fr)_14rem]" : "grid gap-6"}>
      <div className="grid min-w-0 gap-6">
        {hasIntro ? (
          <Card className="grid gap-3">
            <Intro guide={guide} w={w} />
          </Card>
        ) : null}
        {items.map((item) => {
          if (item.type === "section") {
            return (
              <h2 key={item.id} className="mt-2 text-xl font-semibold">
                {item.title}
              </h2>
            );
          }
          if (item.type === "note") return <NoteCard key={item.id} note={item} w={w} />;
          return (
            <Card key={item.id} id={`step-${item.number}`} className="grid scroll-mt-6 gap-4">
              <StepTitle step={item} w={w} />
              <StepFigure step={item} imageUrl={imageUrl} alt={plainText(item.title)} />
              {item.description.trim() ? <Markdown text={item.description} className="text-sm" /> : null}
            </Card>
          );
        })}
      </div>
      {steps.length > 1 ? (
        <nav aria-label={w.steps} className="hidden lg:block">
          <ol className="sticky top-6 grid gap-1 text-sm">
            {steps.map((s) => (
              <li key={s.id}>
                <a href={`#step-${s.number}`} className="flex gap-2 rounded-md px-2 py-1 text-muted-foreground hover:bg-muted hover:text-foreground">
                  <span className="tabular-nums">{s.number}</span>
                  <span className="truncate">{plainText(s.title) || w.step}</span>
                </a>
              </li>
            ))}
          </ol>
        </nav>
      ) : null}
    </div>
  );
}

function Walkthrough({ guide, items, total, imageUrl, w }: { guide: Guide; items: ViewerItem[]; total: number; imageUrl: ImageUrl; w: Words }) {
  const all = useMemo(() => slides(guide, items), [guide, items]);
  const [index, setIndex] = useState(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const moved = useRef(false);
  const slide = all[Math.min(index, all.length - 1)]!;
  const last = all.length - 1;

  const go = (next: number, focusHeading = true) => {
    const clamped = Math.min(Math.max(next, 0), last);
    if (clamped === index) return;
    moved.current = focusHeading;
    setIndex(clamped);
  };

  useEffect(() => {
    if (moved.current) heading.current?.focus();
    moved.current = false;
  }, [index]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (e.metaKey || e.ctrlKey || e.altKey || target?.closest("input, textarea, select, [contenteditable]")) return;
      // Enter on a button already clicks it.
      if (e.key === "ArrowRight" || (e.key === "Enter" && !target?.closest("button, a"))) {
        e.preventDefault();
        go(index + 1, false);
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(index - 1, false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const status =
    slide.kind === "intro"
      ? w.start
      : slide.kind === "done"
        ? w.done
        : `${w.step} ${slide.step.number} ${w.of} ${total}${plainText(slide.step.title) ? `: ${plainText(slide.step.title)}` : ""}`;

  return (
    <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_14rem]">
      <div className="grid min-w-0 content-start gap-4">
        <div className="h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden="true">
          <div className="h-full bg-brand transition-[width]" style={{ width: `${(index / Math.max(last, 1)) * 100}%` }} />
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button variant="secondary" aria-disabled={index === 0} onClick={() => go(index - 1, false)}>
            ← {w.back}
          </Button>
          <p className="min-w-0 flex-1 truncate text-center text-sm text-muted-foreground" role="status" aria-live="polite" aria-atomic="true">
            {status}
          </p>
          <Button aria-disabled={index === last} onClick={() => go(index + 1, false)}>
            {w.next} →
          </Button>
          <Button variant="secondary" onClick={() => go(0)}>
            {w.restart}
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">{w.hint}</p>
        <Card className="grid gap-4">
          {slide.kind === "intro" ? (
            <>
              <h2 ref={heading} tabIndex={-1} className="text-xl font-semibold outline-none">
                {w.start}
              </h2>
              <Intro guide={guide} w={w} />
              <p>
                <Button onClick={() => go(index + 1)}>{w.begin} →</Button>
              </p>
            </>
          ) : slide.kind === "done" ? (
            <>
              <h2 ref={heading} tabIndex={-1} className="text-xl font-semibold outline-none">
                {w.done}
              </h2>
              <p className="text-sm">{w.doneText}</p>
              {slide.notes.map((n) => (
                <NoteCard key={n.id} note={n} w={w} />
              ))}
              <p>
                <Button onClick={() => go(0)}>{w.restart}</Button>
              </p>
            </>
          ) : (
            <>
              {slide.step.section ? <p className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">{slide.step.section}</p> : null}
              <StepTitle step={slide.step} w={w} as="h2" ref={heading} />
              {slide.notes.map((n) => (
                <NoteCard key={n.id} note={n} w={w} />
              ))}
              <StepFigure
                key={slide.step.id}
                step={slide.step}
                imageUrl={imageUrl}
                alt={plainText(slide.step.title)}
                onTarget={() => go(index + 1)}
                targetLabel={w.target}
              />
              {slide.step.description.trim() ? <Markdown text={slide.step.description} className="text-sm" /> : null}
            </>
          )}
        </Card>
      </div>
      <nav aria-label={w.steps}>
        <ol className="grid gap-1 text-sm lg:sticky lg:top-6">
          {all.map((s, i) => {
            const label = s.kind === "intro" ? w.start : s.kind === "done" ? w.done : plainText(s.step.title) || w.step;
            const section = s.kind === "step" && s.step.section !== null && (all[i - 1]?.kind !== "step" || (all[i - 1] as { step: ViewerStep }).step.section !== s.step.section);
            return (
              <li key={s.kind === "step" ? s.step.id : s.kind}>
                {section ? <p className="mt-2 px-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">{(s as { step: ViewerStep }).step.section}</p> : null}
                <button
                  type="button"
                  aria-current={i === index ? "step" : undefined}
                  onClick={() => go(i)}
                  className={`flex w-full gap-2 rounded-md px-2 py-1 text-left ${i === index ? "bg-muted font-medium" : "text-muted-foreground hover:bg-muted hover:text-foreground"}`}
                >
                  {s.kind === "step" ? <span className="tabular-nums">{s.step.number}</span> : null}
                  <span className="truncate">{label}</span>
                </button>
              </li>
            );
          })}
        </ol>
      </nav>
    </div>
  );
}
