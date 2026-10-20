import { Link, useRouter } from "@tanstack/react-router";
import { useEffect, useRef, useState, type MouseEvent } from "react";
import type { Guide } from "@guiderecorder/guide-schema";
import { equal, type Revision } from "@guiderecorder/sync";
import { AnnotatedImage } from "@guiderecorder/ui/annotated-image";
import MarkdownEditor from "@guiderecorder/ui/markdown-editor";
import { Input } from "@guiderecorder/ui/input";
import { ItemView } from "@guiderecorder/ui/step-view";
import { deleteItem, moveItem, setGuideText, setItemText, type GuideTextField } from "@/lib/draft";
import { saveGuideEdit } from "@/server/editor";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { ErrorText, Field } from "@/components/ui";

// The web editor: a synced guide's text, in the desktop app's editors. A
// save is a new revision on the revision the edit began on; the server
// merges it with changes made meanwhile, and the app pulls it like any
// other change.

interface Props {
  workspaceId: string;
  revision: Revision;
}

/** Undo history: guides before (`past`) and after (`future`) the draft. */
interface History {
  past: Guide[];
  future: Guide[];
  /** The field the last change typed into; more typing there is one undo step. */
  typing: string | null;
}

export function GuideEditor({ workspaceId, revision }: Props) {
  const router = useRouter();
  const [draft, setDraftState] = useState<Guide>(() => structuredClone(revision.guide));
  // The draft as of the last change, for handlers that run between renders.
  const latest = useRef(draft);
  const history = useRef<History>({ past: [], future: [], typing: null });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [conflicts, setConflicts] = useState<string[]>([]);
  const dirty = !equal(draft, revision.guide);

  const setDraft = (guide: Guide) => {
    latest.current = guide;
    setDraftState(guide);
  };

  /** Applies a change; `typing` names the field typed into, if any. */
  function change(next: (guide: Guide) => Guide, typing: string | null = null) {
    const guide = latest.current;
    const changed = next(guide);
    if (changed === guide) return;
    const h = history.current;
    if (!typing || h.typing !== typing) h.past.push(guide);
    h.future = [];
    h.typing = typing;
    setDraft(changed);
  }

  function travel(from: "past" | "future") {
    const h = history.current;
    const target = h[from].pop();
    if (!target) return;
    h[from === "past" ? "future" : "past"].push(latest.current);
    h.typing = null;
    setDraft(target);
  }
  function discard() {
    history.current = { past: [], future: [], typing: null };
    setDraft(structuredClone(revision.guide));
  }
  const undo = () => travel("past");
  const redo = () => travel("future");

  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  // ⌘Z outside the text fields (they report their own undo).
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "z") return;
      if (event.target instanceof HTMLElement && event.target.closest("input, textarea, [contenteditable=true]")) return;
      event.preventDefault();
      if (event.shiftKey) redo();
      else undo();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  async function save(prefer?: "ours" | "theirs") {
    setSaving(true);
    setError(null);
    const result = await saveGuideEdit({
      data: {
        guideId: revision.guideId,
        baseRevision: revision.id,
        guide: { ...draft, updatedAt: new Date().toISOString() },
        prefer,
      },
    });
    setSaving(false);
    setConflicts(result.ok ? [] : result.conflicts);
    if (!result.ok) return setError(result.conflicts.length ? null : result.error);
    // The page reloads the new head and starts over from it.
    await router.invalidate();
  }

  const imageUrl = (assets: Revision["assets"]) => (path: string) =>
    assets[path] ? `/api/workspaces/${workspaceId}/assets/${assets[path]}` : null;
  const text = (field: GuideTextField) => (value: string) => {
    change((g) => setGuideText(g, field, value), `guide.${field}`);
    return true;
  };
  const annotated = draft.kind === "annotated";
  const noun = annotated ? "Page" : "Step";
  let stepNumber = 0;

  return (
    <div className="mx-auto grid w-full max-w-3xl gap-7 px-5 py-8 md:px-9">
      <header className="flex flex-wrap items-center justify-between gap-4">
        <Link to="/guides/$guideId" params={{ guideId: revision.guideId }} className="text-sm text-muted-foreground hover:underline">
          ← Back to the guide
        </Link>
        <span className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground" aria-live="polite">
            {dirty ? "Unsaved changes" : `Saved ${new Date(revision.createdAt).toLocaleString()}`}
          </span>
          <Button variant="outline" size="lg" onClick={discard} disabled={!dirty || saving}>
            Discard
          </Button>
          <Button variant="brand" size="lg" onClick={() => save()} disabled={!dirty || saving}>
            {saving ? "Saving…" : "Save"}
          </Button>
        </span>
      </header>
      <ErrorText>{error}</ErrorText>
      {conflicts.length > 0 && (
        <Card role="alert" className="p-6 grid gap-3 border-destructive/40 p-4 text-sm">
          <p>Someone changed the same text since you started editing:</p>
          <ul className="list-disc pl-5">
            {conflicts.map((c) => (
              <li key={c}>{c}</li>
            ))}
          </ul>
          <span className="flex flex-wrap gap-2">
            <Button variant="brand" size="lg" onClick={() => save("ours")} disabled={saving}>
              Keep my changes
            </Button>
            <Button variant="outline" size="lg" onClick={() => save("theirs")} disabled={saving}>
              Keep theirs
            </Button>
          </span>
        </Card>
      )}

      <Card className="p-6 grid gap-4">
        <Field>
          Title
          <Input
            className="h-11 bg-card px-3 font-serif text-2xl"
            value={draft.title}
            onChange={(e) => change((g) => setGuideText(g, "title", e.target.value), "guide.title")}
          />
        </Field>
        <div className="grid gap-1.5 text-sm font-medium">
          Introduction
          <MarkdownEditor value={draft.intro} onChange={text("intro")} onUndo={undo} onRedo={redo} readOnly={false} label="Introduction" placeholder="Introduce the guide" />
        </div>
        <div className="grid gap-1.5 text-sm font-medium">
          Prerequisites
          <MarkdownEditor
            value={draft.prerequisites}
            onChange={text("prerequisites")}
            onUndo={undo}
            onRedo={redo}
            readOnly={false}
            label="Prerequisites"
            placeholder="What the reader needs first"
          />
        </div>
      </Card>

      {draft.items.map((item, index) => {
        if (item.type === "step") stepNumber += 1;
        const name = item.type === "step" ? `${noun} ${stepNumber}` : item.type === "section" ? "Section" : item.kind;
        return (
          <Card key={item.id} className="p-6 grid gap-3" aria-label={name}>
            <ItemToolbar
              name={name}
              first={index === 0}
              last={index === draft.items.length - 1}
              onMove={(by) => change((g) => moveItem(g, item.id, by))}
              onDelete={() => change((g) => deleteItem(g, item.id))}
            />
            <ItemView
              item={item}
              noun={noun}
              image={
                item.type === "step" && (
                  <AnnotatedImage image={item.image} annotations={item.annotations} src={imageUrl(revision.assets)} label={`${name} screenshot`} />
                )
              }
              text={(props) => (
                <MarkdownEditor
                  key={`${item.id}.${props.field}`}
                  {...props}
                  label={`${name} ${props.field === "body" ? "text" : props.field}`}
                  readOnly={false}
                  bare
                  onChange={(markdown) => {
                    change((g) => setItemText(g, item.id, props.field, markdown), `${item.id}.${props.field}`);
                    return true;
                  }}
                  onUndo={undo}
                  onRedo={redo}
                />
              )}
            />
          </Card>
        );
      })}
    </div>
  );
}

function ItemToolbar({
  name,
  first,
  last,
  onMove,
  onDelete,
}: {
  name: string;
  first: boolean;
  last: boolean;
  onMove(by: -1 | 1): void;
  onDelete(): void;
}) {
  // Keep the focus in a text field: its toolbar hiding on blur would move this one away mid-click.
  const keepFocus = (e: MouseEvent) => e.preventDefault();
  return (
    <div className="flex items-center gap-2" role="toolbar" aria-label={`${name} actions`}>
      <span className="text-[11px] font-semibold tracking-[0.08em] text-brand uppercase">{name}</span>
      <span className="ml-auto flex gap-1">
        <Button variant="outline" size="lg" className="h-8 px-2.5" aria-label={`Move ${name} up`} disabled={first} onMouseDown={keepFocus} onClick={() => onMove(-1)}>
          ↑
        </Button>
        <Button variant="outline" size="lg" className="h-8 px-2.5" aria-label={`Move ${name} down`} disabled={last} onMouseDown={keepFocus} onClick={() => onMove(1)}>
          ↓
        </Button>
        <Button variant="destructive" size="lg" className="h-8 px-2.5" aria-label={`Delete ${name}`} onMouseDown={keepFocus} onClick={onDelete}>
          Delete
        </Button>
      </span>
    </div>
  );
}
