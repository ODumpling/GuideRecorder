import { useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { deleteApprovalPolicy, saveApprovalPolicy, type getApprovalSettings } from "@/server/reviews";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { ErrorText, Field } from "@/components/ui";

// Approval policies per library folder (Enterprise; admins). A guide follows
// its folder's nearest policy; folders without one follow the workspace's.

type Data = Extract<Awaited<ReturnType<typeof getApprovalSettings>>, { available: true }>;
type Policy = Data["policies"][number];

export function ApprovalPolicies({ policies, defaultPolicy, approvers }: Data) {
  const router = useRouter();
  const [editing, setEditing] = useState<Policy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const workspace = policies.find((p) => p.folder === "");
  const name = (id: string) => approvers.find((a) => a.id === id)?.name ?? id;

  async function remove(folder: string) {
    setError(null);
    const result = await deleteApprovalPolicy({ data: { folder } });
    if (!result.ok) return setError(result.error);
    await router.invalidate();
  }

  const describe = (p: Policy) =>
    p.requiredApprovals === 0
      ? "No approval needed"
      : `${p.requiredApprovals} of ${p.approverIds.length ? p.approverIds.map(name).join(", ") : "any approver"}` +
        `${p.allowSelfApproval ? ", self-approval allowed" : ""}${p.resetOnNewRevision ? ", reset on new edits" : ""}`;

  return (
    <Card className="block p-6">
      <h2 className="mb-1 text-lg font-semibold">Approvals</h2>
      <p className="mb-4 text-sm text-muted-foreground">
        Edits to guides wait for approval before viewers and share links see them. A folder follows its nearest policy.
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
          <PolicyForm key={editing.folder} policy={editing} approvers={approvers} onDone={() => setEditing(null)} />
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

function PolicyForm({ policy, approvers, onDone }: { policy: Policy; approvers: Data["approvers"]; onDone: () => void }) {
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
        approverIds: form.getAll("approverIds").map(String),
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
        Approvals needed (0: publish without review)
        <Input name="requiredApprovals" type="number" min={0} max={20} defaultValue={policy.requiredApprovals} className="w-24" />
      </Field>
      <fieldset className="grid gap-1">
        <legend className="mb-1 font-medium">Approvers (none ticked: anyone whose role can approve)</legend>
        {approvers.map((a) => (
          <label key={a.id} className="flex items-center gap-2">
            <input type="checkbox" name="approverIds" value={a.id} defaultChecked={policy.approverIds.includes(a.id)} />
            {a.name} <span className="text-muted-foreground">{a.email}</span>
          </label>
        ))}
      </fieldset>
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
