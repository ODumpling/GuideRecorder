import { Link, useRouter } from "@tanstack/react-router";
import { useState, type ReactNode } from "react";
import type { Guide } from "@guiderecorder/guide-schema";
import type { Revision } from "@guiderecorder/sync";
import { saveSyncedGuide } from "@/server/guides";
import { Button, Card, ErrorText, Input } from "../ui";

// A synced guide on the web: read it, or edit its text. Edits become a new
// revision the desktop app pulls and merges. Share links show it read-only.

type Item = { id: string; type: "step" | "section" | "note"; title?: string; description?: string; body?: string; image?: StepImage };
type StepImage = { full: string; crop?: string; show?: "crop" | "full" };

interface Props {
  revision: Pick<Revision, "id" | "guideId" | "guide" | "assets" | "source" | "createdAt">;
  /** Where to load an image from, by its SHA-256. */
  assetUrl: (sha256: string) => string;
  canEdit: boolean;
  /** Shown above the guide, e.g. the back link or the share links. */
  header?: ReactNode;
}

export function GuideView({ revision, assetUrl, canEdit, header }: Props) {
  const router = useRouter();
  const [draft, setDraft] = useState<Guide | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const guide = draft ?? revision.guide;
  const items = (guide.items ?? []) as Item[];

  const imageUrl = (image: StepImage) => {
    const path = image.show === "full" || !image.crop ? image.full : image.crop;
    const sha = revision.assets[path];
    return sha ? assetUrl(sha) : null;
  };

  const setField = (field: "title" | "intro", value: string) => setDraft({ ...guide, [field]: value });
  const setItem = (id: string, field: "title" | "description" | "body", value: string) =>
    setDraft({ ...guide, items: items.map((i) => (i.id === id ? { ...i, [field]: value } : i)) as Guide["items"] });

  async function save() {
    if (!draft) return;
    setSaving(true);
    setError(null);
    const result = await saveSyncedGuide({
      data: { guideId: revision.guideId, baseRevision: revision.id, guide: { ...draft, updatedAt: new Date().toISOString() }, assets: revision.assets },
    });
    setSaving(false);
    if (!result.ok) return setError(result.error);
    setDraft(null);
    await router.invalidate();
  }

  const editing = draft !== null;
  let stepNumber = 0;

  return (
    <div className="mx-auto grid w-full max-w-3xl gap-6 p-6">
      <header className="flex items-center justify-between gap-4 empty:hidden">
        {header}
        {canEdit ? (
          editing ? (
            <span className="flex gap-2">
              <Button variant="secondary" onClick={() => setDraft(null)} disabled={saving}>
                Cancel
              </Button>
              <Button onClick={save} disabled={saving}>
                {saving ? "Saving…" : "Save"}
              </Button>
            </span>
          ) : (
            <Link
              to="/guides/$guideId/edit"
              params={{ guideId: revision.guideId }}
              className="inline-flex h-9 items-center rounded-lg border border-border bg-card px-4 text-sm font-medium hover:bg-muted"
            >
              Edit
            </Link>
          )
        ) : null}
      </header>
      <ErrorText>{error}</ErrorText>

      <Card className="grid gap-3">
        {editing ? (
          <Input aria-label="Title" className="h-11 font-serif text-2xl" value={guide.title} onChange={(e) => setField("title", e.target.value)} />
        ) : (
          <h1 className="font-serif text-3xl">{guide.title}</h1>
        )}
        {editing ? (
          <TextArea label="Introduction" value={guide.intro ?? ""} onChange={(v) => setField("intro", v)} />
        ) : guide.intro ? (
          <p className="whitespace-pre-wrap text-sm">{guide.intro}</p>
        ) : null}
        <p className="text-xs text-muted-foreground">
          Last changed {new Date(revision.createdAt).toLocaleString()} {revision.source === "web" ? "on the web" : "in the desktop app"}
        </p>
      </Card>

      {items.map((item) => {
        if (item.type === "section") {
          return editing ? (
            <Input key={item.id} aria-label="Section title" className="font-semibold" value={item.title ?? ""} onChange={(e) => setItem(item.id, "title", e.target.value)} />
          ) : (
            <h2 key={item.id} className="mt-2 text-xl font-semibold">
              {item.title}
            </h2>
          );
        }
        if (item.type === "note") {
          return (
            <Card key={item.id} className="bg-muted/50 p-4 text-sm">
              {editing ? (
                <TextArea label="Note" value={item.body ?? ""} onChange={(v) => setItem(item.id, "body", v)} />
              ) : (
                <p className="whitespace-pre-wrap">{item.body}</p>
              )}
            </Card>
          );
        }
        stepNumber += 1;
        const src = item.image && imageUrl(item.image);
        return (
          <Card key={item.id} className="grid gap-3">
            {editing ? (
              <>
                <Input aria-label={`Step ${stepNumber} title`} value={item.title ?? ""} onChange={(e) => setItem(item.id, "title", e.target.value)} />
                <TextArea label="Description" value={item.description ?? ""} onChange={(v) => setItem(item.id, "description", v)} />
              </>
            ) : (
              <>
                <h3 className="font-medium">
                  <span className="mr-2 text-muted-foreground">{stepNumber}.</span>
                  {item.title}
                </h3>
                {item.description ? <p className="whitespace-pre-wrap text-sm">{item.description}</p> : null}
              </>
            )}
            {src ? <img src={src} alt="" className="rounded-lg border border-border" loading="lazy" /> : null}
          </Card>
        );
      })}
    </div>
  );
}

function TextArea({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return (
    <textarea
      aria-label={label}
      className="min-h-20 w-full rounded-lg border border-input bg-card px-3 py-2 text-sm outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
