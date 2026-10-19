import { Link, useRouter } from "@tanstack/react-router";
import { useState } from "react";
import type { Guide } from "@guiderecorder/guide-schema";
import type { Revision } from "@guiderecorder/sync";
import type { ExportFormat } from "@/lib/exports";
import { saveSyncedGuide } from "@/server/guides";
import { Button, Card, ErrorText, Input } from "../ui";
import { ExportMenu } from "./ExportMenu";
import { GuideViewer } from "./GuideViewer";

// A synced guide on the web: read it (<GuideViewer>), download it, or edit
// its text. Edits become a new revision the desktop app pulls and merges.

type Item = { id: string; type: "step" | "section" | "note"; title?: string; description?: string; body?: string; image?: StepImage };
type StepImage = { full: string; crop?: string; show?: "crop" | "full" };

interface Props {
  workspaceId: string;
  revision: Revision;
  canEdit: boolean;
  folder?: string | null;
  exportFormats?: { id: ExportFormat; label: string }[];
}

/** URLs for a revision's images: the workspace asset endpoint, which redirects to a short-lived signed URL. */
export function revisionImageUrl(workspaceId: string, revision: Pick<Revision, "assets">) {
  return (path: string) => {
    const sha = revision.assets[path];
    return sha ? `/api/workspaces/${workspaceId}/assets/${sha}` : null;
  };
}

export function GuideView({ workspaceId, revision, canEdit, folder = null, exportFormats = [] }: Props) {
  const router = useRouter();
  const [draft, setDraft] = useState<Guide | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const guide = draft ?? revision.guide;
  const items = (guide.items ?? []) as Item[];

  const assetUrl = revisionImageUrl(workspaceId, revision);
  const imageUrl = (image: StepImage) => assetUrl(image.show === "full" || !image.crop ? image.full : image.crop);

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

  let stepNumber = 0;
  const changed = `Last changed ${new Date(revision.createdAt).toLocaleString()} ${revision.source === "web" ? "on the web" : "in the desktop app"}`;

  if (!draft) {
    return (
      <GuideViewer
        guide={revision.guide}
        imageUrl={assetUrl}
        header={
          <header className="flex flex-wrap items-center justify-between gap-4">
            <Link to="/library" search={folder ? { folder } : {}} className="text-sm text-muted-foreground hover:underline">
              ← {folder ?? "Library"}
            </Link>
            <span className="flex gap-2">
              <ExportMenu guideId={revision.guideId} revisionId={revision.id} formats={exportFormats} />
              {canEdit ? (
                <Button variant="secondary" onClick={() => setDraft(structuredClone(revision.guide))}>
                  Edit
                </Button>
              ) : null}
            </span>
          </header>
        }
        meta={<p className="text-xs text-muted-foreground">{changed}</p>}
      />
    );
  }

  return (
    <div className="mx-auto grid w-full max-w-3xl gap-6 p-6">
      <header className="flex items-center justify-between gap-4">
        <Link to="/library" className="text-sm text-muted-foreground hover:underline">
          ← Library
        </Link>
        <span className="flex gap-2">
          <Button variant="secondary" onClick={() => setDraft(null)} disabled={saving}>
            Cancel
          </Button>
          <Button onClick={save} disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </Button>
        </span>
      </header>
      <ErrorText>{error}</ErrorText>

      <Card className="grid gap-3">
        <Input aria-label="Title" className="h-11 font-serif text-2xl" value={guide.title} onChange={(e) => setField("title", e.target.value)} />
        <TextArea label="Introduction" value={guide.intro ?? ""} onChange={(v) => setField("intro", v)} />
        <p className="text-xs text-muted-foreground">{changed}</p>
      </Card>

      {items.map((item) => {
        if (item.type === "section") {
          return (
            <Input key={item.id} aria-label="Section title" className="font-semibold" value={item.title ?? ""} onChange={(e) => setItem(item.id, "title", e.target.value)} />
          );
        }
        if (item.type === "note") {
          return (
            <Card key={item.id} className="bg-muted/50 p-4 text-sm">
              <TextArea label="Note" value={item.body ?? ""} onChange={(v) => setItem(item.id, "body", v)} />
            </Card>
          );
        }
        stepNumber += 1;
        const src = item.image && imageUrl(item.image);
        return (
          <Card key={item.id} className="grid gap-3">
            <Input aria-label={`Step ${stepNumber} title`} value={item.title ?? ""} onChange={(e) => setItem(item.id, "title", e.target.value)} />
            <TextArea label="Description" value={item.description ?? ""} onChange={(v) => setItem(item.id, "description", v)} />
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
