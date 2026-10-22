import { lazy, Suspense, useId, useState, type ReactNode } from "react";
import { X } from "lucide-react";
import {
  editedGuideFields,
  setGuideField,
  setReviewField,
  type GuideTextField,
  type ReviewField,
} from "@/editor/commands";
import { editorStore, useEditor } from "@/editor/editor";
import { GUIDE_STATUSES, isReviewOverdue, localToday, reviewDue } from "@/editor/library";
import type { Guide, GuideStatus } from "@guiderecorder/guide-schema";
import type { ReviewFields } from "@/lib/types";
import { cn } from "@guiderecorder/ui/utils";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";
import { addTags, removeTag } from "@/text/tags";
import { AI_ENABLED } from "@/lib/features";

// TipTap is most of the bundle; load it with the first opened guide.
const MarkdownEditor = lazy(() => import("@guiderecorder/ui/markdown-editor"));

/** Sets a guide field through the store (undoable, autosaved). */
function set<K extends GuideTextField>(key: K, value: Guide[K]): boolean {
  const { doc, dispatch } = editorStore.getState();
  return !!doc && dispatch(setGuideField(doc.guide, key, value));
}

const LABEL = "text-[11px] font-semibold tracking-wide text-muted-foreground uppercase";

/** A field's name; `id` ties it to a plain input (Markdown editors carry their own label). */
function FieldLabel({ id, label, edited, hint }: { id?: string; label: string; edited: boolean; hint: string }) {
  return (
    <div className="flex items-baseline gap-2">
      {id ? (
        <label htmlFor={id} id={`${id}-label`} className={LABEL}>
          {label}
        </label>
      ) : (
        <h2 className={LABEL}>{label}</h2>
      )}
      {edited && (
        <span className="text-[10px] text-muted-foreground" title={AI_ENABLED ? "Written by you: Write with AI leaves it as it is" : "Written by you"}>
          Edited
        </span>
      )}
      <span className="ml-auto text-xs text-muted-foreground">{hint}</span>
    </div>
  );
}

const CARD = "flex flex-col gap-5 rounded-xl border border-border bg-card p-5 text-card-foreground shadow-sm";

function Field({ children }: { children: ReactNode }) {
  return <section className="flex flex-col gap-1.5">{children}</section>;
}

/** A Markdown guide field (intro, prerequisites). */
function MarkdownField({
  field,
  value,
  readOnly,
  label,
  placeholder,
}: {
  field: "intro" | "prerequisites";
  value: string;
  readOnly: boolean;
  label: string;
  placeholder: string;
}) {
  const fallback = (
    <div
      aria-label={label}
      className={cn(
        "min-h-16 rounded-md border border-input px-2.5 py-1.5 text-sm whitespace-pre-wrap",
        !value && "text-muted-foreground",
      )}
    >
      {markdownToPlainText(value) || placeholder}
    </div>
  );
  return (
    <Suspense fallback={fallback}>
      <MarkdownEditor
        value={value}
        readOnly={readOnly}
        label={label}
        placeholder={placeholder}
        className="text-sm"
        onChange={(markdown) => set(field, markdown)}
        onUndo={() => editorStore.getState().undo()}
        onRedo={() => editorStore.getState().redo()}
      />
    </Suspense>
  );
}

/** The guide's tags as chips, with a field that adds more (Enter or comma). */
function TagsField({ id, tags, readOnly }: { id: string; tags: string[]; readOnly: boolean }) {
  const [draft, setDraft] = useState("");
  const commit = (text: string) => {
    const next = addTags(tags, text);
    if (next !== tags) set("tags", next);
    setDraft("");
  };
  return (
    <div
      className={cn(
        "flex min-h-8 flex-wrap items-center gap-1 rounded-md border border-input px-1.5 py-1 focus-within:ring-2 focus-within:ring-ring",
        readOnly && "opacity-60",
      )}
    >
      <ul className="contents" aria-label="Tags">
        {tags.map((tag, i) => (
          <li key={`${i}.${tag}`} className="flex items-center gap-0.5 rounded bg-muted py-0.5 pr-0.5 pl-2 text-xs">
            {tag}
            {!readOnly && (
              <button
                type="button"
                aria-label={`Remove tag ${tag}`}
                title={`Remove tag ${tag}`}
                className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-accent-foreground"
                onClick={() => set("tags", removeTag(tags, i))}
              >
                <X className="size-3" />
              </button>
            )}
          </li>
        ))}
      </ul>
      {!readOnly && (
        <input
          id={id}
          aria-labelledby={`${id}-label`}
          className="h-6 min-w-24 flex-1 bg-transparent px-1 text-sm outline-none placeholder:text-muted-foreground"
          placeholder={tags.length ? "Add a tag" : "Add tags, separated by commas"}
          value={draft}
          onChange={(e) => {
            const text = e.target.value;
            // A typed or pasted comma ends the tag.
            if (/[,\n]/.test(text)) commit(text);
            else setDraft(text);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit(draft);
            } else if (e.key === "Backspace" && !draft && tags.length) {
              e.preventDefault();
              set("tags", removeTag(tags, tags.length - 1));
            }
          }}
          onBlur={() => draft.trim() && commit(draft)}
        />
      )}
    </div>
  );
}

/**
 * The centre pane when no item is selected: the guide's intro,
 * prerequisites, summary and tags, written by hand. Each edit is undoable
 * and autosaved, and marks the field edited, so Write with AI keeps it.
 * Below them: owner, status, version and review schedule.
 */
export function GuideDetails() {
  const guide = useEditor((s) => s.doc?.guide);
  const readOnly = useEditor((s) => s.doc?.readOnly ?? true);
  const id = useId();
  if (!guide) return null;
  const edited = new Set(editedGuideFields(guide));
  return (
    <div
      key={guide.id}
      className="mx-auto flex w-full max-w-[52rem] flex-col gap-6 px-10 pt-7 pb-10"
      aria-label="Guide details"
    >
      <div>
        <p className="text-[11px] font-semibold tracking-[0.08em] text-brand uppercase">Guide details</p>
        <h1 className="mt-2 font-serif text-[40px] leading-none tracking-tight">{guide.title || "Untitled"}</h1>
      </div>
      <div className={CARD}>
        <Field>
          <FieldLabel label="Introduction" edited={edited.has("intro")} hint="Shown before the first step" />
          <MarkdownField
            field="intro"
            value={guide.intro}
            readOnly={readOnly}
            label="Introduction"
            placeholder="What this guide helps the reader do"
          />
        </Field>
        <Field>
          <FieldLabel label="Prerequisites" edited={edited.has("prerequisites")} hint="What the reader needs first" />
          <MarkdownField
            field="prerequisites"
            value={guide.prerequisites}
            readOnly={readOnly}
            label="Prerequisites"
            placeholder="A list, e.g. an account with admin access"
          />
        </Field>
        <Field>
          <FieldLabel
            id={`${id}-summary`}
            label="Summary"
            edited={edited.has("summary")}
            hint="Shown in the library and search"
          />
          <textarea
            id={`${id}-summary`}
            rows={2}
            className="w-full min-w-0 resize-y rounded-md border border-input bg-transparent px-2.5 py-1.5 text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
            placeholder="One or two sentences about the guide"
            value={guide.summary}
            disabled={readOnly}
            onChange={(e) => set("summary", e.target.value)}
          />
        </Field>
        <Field>
          <FieldLabel id={`${id}-tags`} label="Tags" edited={edited.has("tags")} hint="Press Enter or comma to add" />
          <TagsField id={`${id}-tags`} tags={guide.tags} readOnly={readOnly} />
        </Field>
      </div>
      <div className={CARD}>
        <ReviewSection id={`${id}-review`} guide={guide} readOnly={readOnly} />
      </div>
    </div>
  );
}

/** Sets an owner, status, version or review field through the store (undoable, autosaved). */
function setReview<K extends ReviewField>(key: K, value: ReviewFields[K]): boolean {
  const { doc, dispatch } = editorStore.getState();
  return !!doc && dispatch(setReviewField(doc.guide, key, value));
}

const INPUT =
  "h-8 w-full min-w-0 rounded-md border border-input bg-transparent px-2.5 text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60";

/** Who owns the guide, its status and version, and when it is reviewed. */
function ReviewSection({ id, guide, readOnly }: { id: string; guide: Guide; readOnly: boolean }) {
  const today = localToday();
  const due = reviewDue(guide);
  const overdue = isReviewOverdue(guide, today);
  const small = (label: string, field: string) => (
    <label htmlFor={`${id}-${field}`} className="text-xs text-muted-foreground">
      {label}
    </label>
  );
  return (
    <Field>
      <FieldLabel
        label="Ownership and review"
        edited={false}
        hint={
          due
            ? overdue
              ? `Review overdue since ${due}`
              : `Next review due ${due}`
            : "Shown in the library and exports"
        }
      />
      <div className="grid grid-cols-2 gap-x-3 gap-y-2.5 sm:grid-cols-3">
        <div className="flex flex-col gap-1">
          {small("Owner", "owner")}
          <input
            id={`${id}-owner`}
            className={INPUT}
            placeholder="Who keeps it current"
            value={guide.owner ?? ""}
            disabled={readOnly}
            onChange={(e) => setReview("owner", e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          {small("Status", "status")}
          <select
            id={`${id}-status`}
            className={INPUT}
            value={guide.status ?? ""}
            disabled={readOnly}
            onChange={(e) => setReview("status", (e.target.value || null) as GuideStatus | null)}
          >
            <option value="">Not set</option>
            {GUIDE_STATUSES.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
              </option>
            ))}
          </select>
        </div>
        <div className="flex flex-col gap-1">
          {small("Version", "version")}
          <input
            id={`${id}-version`}
            className={INPUT}
            placeholder="e.g. 1.2"
            value={guide.version ?? ""}
            disabled={readOnly}
            onChange={(e) => setReview("version", e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          {small("Last reviewed", "reviewed")}
          <div className="flex gap-1.5">
            <input
              id={`${id}-reviewed`}
              type="date"
              className={INPUT}
              value={guide.lastReviewed ?? ""}
              disabled={readOnly}
              onChange={(e) => setReview("lastReviewed", e.target.value)}
            />
            <button
              type="button"
              disabled={readOnly || guide.lastReviewed === today}
              title="Mark the guide reviewed today"
              className="h-8 shrink-0 rounded-md border border-input px-2 text-xs hover:bg-accent disabled:opacity-50"
              onClick={() => setReview("lastReviewed", today)}
            >
              Today
            </button>
          </div>
        </div>
        <div className="flex flex-col gap-1">
          {small("Review every (months)", "interval")}
          <input
            id={`${id}-interval`}
            type="number"
            min={0}
            max={120}
            step={1}
            className={INPUT}
            placeholder="No schedule"
            value={guide.reviewIntervalMonths ?? ""}
            disabled={readOnly}
            onChange={(e) => {
              const months = Math.trunc(Number(e.target.value));
              setReview("reviewIntervalMonths", e.target.value && months > 0 ? Math.min(months, 120) : null);
            }}
          />
        </div>
      </div>
    </Field>
  );
}
