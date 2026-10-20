import { useRouter } from "@tanstack/react-router";
import { useState } from "react";
import { Lock, LockOpen, X } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from "@guiderecorder/ui/dialog";
import { NativeSelect } from "@guiderecorder/ui/native-select";
import { Separator } from "@guiderecorder/ui/separator";
import { Switch } from "@guiderecorder/ui/switch";
import { ErrorText } from "@/components/ui";
import { addGuideOwner, removeGuideOwner, setGuideLocked, type getGuideAccess } from "@/server/ownership";
import { Avatar } from "./bits";

// A guide's owners and lock, on its page. Locked: edits wait for an owner's
// approval; unlocked: anyone who can edit guides publishes straight away.

type Access = NonNullable<Awaited<ReturnType<typeof getGuideAccess>>>;

export function GuideAccess({ guideId, access }: { guideId: string; access: Access }) {
  const router = useRouter();
  const [adding, setAdding] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { locked, owners, candidates, canManage, canLock } = access;

  async function run(action: () => Promise<{ ok: true } | { ok: false; error: string }>) {
    setBusy(true);
    setError(null);
    const result = await action();
    setBusy(false);
    if (!result.ok) return setError(result.error);
    await router.invalidate();
  }

  return (
    <Dialog>
      <DialogTrigger asChild>
        <Button variant="ghost" title={locked ? "Locked: changes need an owner's approval" : "Unlocked: edits publish straight away"}>
          {locked ? <Lock /> : <LockOpen />}
          {locked ? "Locked" : "Unlocked"}
          <span className="ml-1 flex -space-x-1.5">
            {owners.slice(0, 3).map((o) => (
              <Avatar key={o.id} name={o.name} label={`Owner ${o.name}`} className="ring-2 ring-background" />
            ))}
          </span>
        </Button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Owners and locking</DialogTitle>
          <DialogDescription>Owners approve changes while the guide is locked. They and workspace admins manage this.</DialogDescription>
        </DialogHeader>

        <label className="flex items-start justify-between gap-4">
          <span className="grid gap-0.5">
            <span className="text-sm font-medium">{locked ? "Locked" : "Unlocked"}</span>
            <span className="text-sm text-muted-foreground">
              {locked
                ? "Changes wait for an owner's approval before readers see them."
                : canLock
                  ? "Anyone who can edit guides publishes changes straight away."
                  : "Anyone who can edit guides publishes changes straight away. Locking comes with Enterprise."}
            </span>
          </span>
          <Switch
            checked={locked}
            disabled={!canManage || busy || (!locked && !canLock)}
            onCheckedChange={(on) => run(() => setGuideLocked({ data: { guideId, locked: on } }))}
            aria-label="Locked"
          />
        </label>

        <Separator />

        <div className="grid gap-2">
          <h3 className="text-sm font-medium">Owners</h3>
          <ul className="grid gap-1">
            {owners.map((o) => (
              <li key={o.id} className="flex items-center gap-2.5 rounded-md py-1 text-sm">
                <Avatar name={o.name} className="size-7 text-[10px]" />
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate font-medium">{o.name}</span>
                  <span className="truncate text-xs text-muted-foreground">{o.email}</span>
                </span>
                {canManage && owners.length > 1 ? (
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label={`Remove ${o.name}`}
                    disabled={busy}
                    onClick={() => run(() => removeGuideOwner({ data: { guideId, userId: o.id } }))}
                  >
                    <X />
                  </Button>
                ) : null}
              </li>
            ))}
            {!owners.length ? <li className="text-sm text-muted-foreground">No owners: anyone whose role can approve does.</li> : null}
          </ul>
          {canManage && candidates.length ? (
            <form
              className="flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                if (adding) void run(() => addGuideOwner({ data: { guideId, userId: adding } })).then(() => setAdding(""));
              }}
            >
              <NativeSelect aria-label="New owner" value={adding} onChange={(e) => setAdding(e.target.value)} className="w-56">
                <option value="">Add an owner…</option>
                {candidates.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name} ({c.email})
                  </option>
                ))}
              </NativeSelect>
              <Button type="submit" variant="outline" disabled={!adding || busy}>
                Add
              </Button>
            </form>
          ) : null}
        </div>
        <ErrorText>{error}</ErrorText>
      </DialogContent>
    </Dialog>
  );
}
