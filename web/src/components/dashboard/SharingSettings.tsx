import { useRouter } from "@tanstack/react-router";
import { useState } from "react";
import type { SharingSettings as Settings } from "@/lib/sharing";
import { saveWorkspaceSharing } from "@/server/shares";
import { Button, Card, ErrorText, Input, Label } from "../ui";

const MODES: { value: Settings["shareLinks"]; label: string }[] = [
  { value: "anyone", label: "Anyone with the link" },
  { value: "password", label: "Only with a password" },
  { value: "off", label: "Off: no share links" },
];

/** Who share links work for, for workspace admins. Changing it needs Enterprise. */
export function SharingSettings({ settings, canManage }: { settings: Settings; canManage: boolean }) {
  const router = useRouter();
  const [draft, setDraft] = useState(settings);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const changed = draft.shareLinks !== settings.shareLinks || draft.shareLinkMaxDays !== settings.shareLinkMaxDays;

  async function save() {
    setSaving(true);
    setError(null);
    const result = await saveWorkspaceSharing({ data: draft });
    setSaving(false);
    if (!result.ok) setError(result.error);
    else await router.invalidate();
  }

  return (
    <Card className="grid gap-4">
      <div>
        <h2 className="text-lg font-semibold">Share links</h2>
        <p className="text-sm text-muted-foreground">
          {canManage
            ? "Applies to every link in the workspace, including existing ones."
            : "Requiring passwords, limiting how long links last, or turning them off comes with Enterprise."}
        </p>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <Label>
          Links work for
          <select
            className="h-9 rounded-lg border border-input bg-card px-3 text-sm"
            disabled={!canManage}
            value={draft.shareLinks}
            onChange={(e) => setDraft({ ...draft, shareLinks: e.target.value as Settings["shareLinks"] })}
          >
            {MODES.map((m) => (
              <option key={m.value} value={m.value}>
                {m.label}
              </option>
            ))}
          </select>
        </Label>
        <Label>
          Longest a link lasts (days)
          <Input
            type="number"
            min={1}
            max={3650}
            placeholder="No limit"
            disabled={!canManage || draft.shareLinks === "off"}
            value={draft.shareLinkMaxDays ?? ""}
            onChange={(e) => setDraft({ ...draft, shareLinkMaxDays: e.target.value ? Math.max(1, Math.round(Number(e.target.value))) : null })}
          />
        </Label>
      </div>
      {canManage ? (
        <Button className="justify-self-start" disabled={!changed || saving} onClick={save}>
          {saving ? "Saving…" : "Save"}
        </Button>
      ) : null}
      <ErrorText>{error}</ErrorText>
    </Card>
  );
}
