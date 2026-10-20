import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { PROVISIONING_ROLES } from "@/lib/permissions";
import type { WorkspacePolicy } from "@/lib/policy";
import { saveAdminPolicy } from "@/server/admin";
import { NativeSelect } from "@guiderecorder/ui/native-select";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { ErrorText, Field } from "@/components/ui";
import { useAction } from "./useAction";

const SHARING: { value: WorkspacePolicy["sharing"]; label: string }[] = [
  { value: "public", label: "Anyone with a link" },
  { value: "workspace", label: "Only signed-in members" },
  { value: "off", label: "No sharing outside the workspace" },
];


/** Sharing, sign-in and retention policy. */
export function PolicySettings({ policy, hasSso }: { policy: WorkspacePolicy; hasSso: boolean }) {
  const { run, error, busy } = useAction();
  const [draft, setDraft] = useState(policy);
  const changed = JSON.stringify(draft) !== JSON.stringify(policy);
  const days = (value: string) => (value ? Math.round(Number(value)) : null);

  return (
    <Card className="p-6 grid gap-4">
      <h2 className="text-lg font-semibold">Policies</h2>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field>
          Guides can be shared with
          <NativeSelect value={draft.sharing} onChange={(e) => setDraft({ ...draft, sharing: e.target.value as WorkspacePolicy["sharing"] })}>
            {SHARING.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field>
          New members from SSO and SCIM are
          <NativeSelect
            value={draft.provisioningRole}
            onChange={(e) => setDraft({ ...draft, provisioningRole: e.target.value as WorkspacePolicy["provisioningRole"] })}
          >
            {PROVISIONING_ROLES.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <label className="flex items-center gap-2 text-sm sm:col-span-2">
          <input type="checkbox" checked={draft.requireSso} disabled={!hasSso} onChange={(e) => setDraft({ ...draft, requireSso: e.target.checked })} />
          Require SSO for everyone on our verified domains (owners can still use a password)
        </label>
        <Field>
          Keep old revisions for (days)
          <Input
            type="number"
            min={7}
            max={3650}
            placeholder="Forever"
            value={draft.revisionRetentionDays ?? ""}
            onChange={(e) => setDraft({ ...draft, revisionRetentionDays: days(e.target.value) })}
          />
        </Field>
      </div>
      <p className="text-xs text-muted-foreground">
        Each guide&rsquo;s latest and published revisions, and revisions a share link or review names, are always kept.
        Guides in the trash are deleted after 30 days. How long audit events are kept is set on the{" "}
        <Link to="/audit" className="underline-offset-4 hover:underline">
          audit log
        </Link>
        .
      </p>
      <Button variant="brand" size="lg" className="justify-self-start" disabled={!changed || busy} onClick={() => run(() => saveAdminPolicy({ data: draft }))}>
        {busy ? "Saving…" : "Save"}
      </Button>
      <ErrorText>{error}</ErrorText>
    </Card>
  );
}
