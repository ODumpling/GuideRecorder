import { useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { deleteApprovalPolicy, saveApprovalPolicy, type getApprovalSettings } from "@/server/reviews";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { ErrorText, Field } from "@/components/ui";

// Approval rules per library folder (Enterprise; admins), for locked guides:
// a locked guide follows its folder's nearest policy (folders without one
// follow the workspace's), and its owners approve.

type Data = Extract<Awaited<ReturnType<typeof getApprovalSettings>>, { available: true }>;
type Policy = Data["policies"][number];

export function ApprovalPolicies({ policies, defaultPolicy }: Data) {
  const router = useRouter();
  const [editing, setEditing] = useState<Policy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const workspace = policies.find((p) => p.folder === "");

  async function remove(folder: string) {
    setError(null);
    const result = await deleteApprovalPolicy({ data: { folder } });
    if (!result.ok) return setError(result.error);
    await router.invalidate();
  }

  const describe = (p: Policy) =>
    `${Math.max(1, p.requiredApprovals)} owner ${Math.max(1, p.requiredApprovals) === 1 ? "approval" : "approvals"}` +
    `${p.allowSelfApproval ? ", self-approval allowed" : ""}${p.resetOnNewRevision ? ", reset on new edits" : ""}`;

  return (
    <Card className="block p-6">
      <h2 className="mb-1 text-lg font-semibold">Approvals</h2>
      <p className="mb-4 text-sm text-muted-foreground">
        Edits to locked guides wait for their owners&rsquo; approval before viewers and share links see them; unlocked guides
        publish straight away. Lock a guide from its page. A folder follows its nearest policy.
      </p>
      <ul className="divide-y divide-border">
        <li className="flex items-center justify-between gap-4 py-2 text-sm">
          <span>
            <span className="font-medium">Whole workspace</span> <span className="text-muted-foreground">{describe(workspace ?? defaultPolicy)}</span>
          </span>
          <Button variant="outline" onClick={() => setEditing(workspace ?? defaultPolicy)}>
            Edit
          </Button>
        </li>
        {policies
          .filter((p) => p.folder)
          .map((p) => (
            <li key={p.folder} className="flex items-center justify-between gap-4 py-2 text-sm">
              <span>
                <span className="font-medium">{p.folder}</span> <span className="text-muted-foreground">{describe(p)}</span>
              </span>
              <span className="flex gap-2">
                <Button variant="outline" onClick={() => setEditing(p)}>
                  Edit
                </Button>
                <Button variant="destructive" onClick={() => remove(p.folder)}>
                  Remove
                </Button>
              </span>
            </li>
          ))}
      </ul>
      <div className="mt-4">
        {editing ? (
          <PolicyForm key={editing.folder} policy={editing} onDone={() => setEditing(null)} />
        ) : (
          <Button variant="outline" size="lg" onClick={() => setEditing({ ...defaultPolicy, folder: "new" })}>
            Add a folder policy
          </Button>
        )}
      </div>
      <ErrorText>{error}</ErrorText>
    </Card>
  );
}

function PolicyForm({ policy, onDone }: { policy: Policy; onDone: () => void }) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const isNew = policy.folder === "new";

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setError(null);
    const result = await saveApprovalPolicy({
      data: {
        folder: isNew ? String(form.get("folder") ?? "") : policy.folder,
        requiredApprovals: Number(form.get("requiredApprovals")),
        allowSelfApproval: form.get("allowSelfApproval") === "on",
        resetOnNewRevision: form.get("resetOnNewRevision") === "on",
      },
    });
    if (!result.ok) return setError(result.error);
    onDone();
    await router.invalidate();
  }

  return (
    <form onSubmit={save} className="grid gap-3 rounded-lg border border-border p-4 text-sm">
      {isNew ? (
        <Field>
          Folder
          <Input name="folder" placeholder="e.g. HR/Payroll" required />
        </Field>
      ) : (
        <p className="font-medium">{policy.folder || "Whole workspace"}</p>
      )}
      <Field>
        Owner approvals needed (at most as many as a guide has owners)
        <Input name="requiredApprovals" type="number" min={1} max={20} defaultValue={Math.max(1, policy.requiredApprovals)} className="w-24" />
      </Field>
      <label className="flex items-center gap-2">
        <input type="checkbox" name="allowSelfApproval" defaultChecked={policy.allowSelfApproval} />
        Authors may approve their own changes
      </label>
      <label className="flex items-center gap-2">
        <input type="checkbox" name="resetOnNewRevision" defaultChecked={policy.resetOnNewRevision} />
        New edits reset approvals
      </label>
      <span className="flex gap-2">
        <Button variant="brand" size="lg" type="submit">Save</Button>
        <Button type="button" variant="outline" size="lg" onClick={onDone}>
          Cancel
        </Button>
      </span>
      <ErrorText>{error}</ErrorText>
    </form>
  );
}
