import { Link } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { Badge } from "@guiderecorder/ui/badge";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { NativeSelect } from "@guiderecorder/ui/native-select";
import { useAction } from "@/components/admin/useAction";
import { ErrorText, Field } from "@/components/ui";
import { endLicence, grantLicence, restoreGuideForUser, type getStaffUser } from "@/server/staff";
import { dateTime, day, isoDay } from "./format";

type Data = Awaited<ReturnType<typeof getStaffUser>>;
type Workspace = Data["workspaces"][number];

const PLAN_LABEL: Record<string, string> = { free: "Free", pro: "Pro", enterprise: "Enterprise" };
/** The last day (UTC, as issued) a grant is valid: the day before its exclusive end. */
const lastDay = (endsAt: Date | string) =>
  new Date(new Date(endsAt).getTime() - 1).toLocaleDateString(undefined, { dateStyle: "medium", timeZone: "UTC" });

/** One user, their workspaces' plans and licences, and guides they can get back. */
export function StaffUser({ data }: { data: Data }) {
  const { user, signInMethods, sessions, workspaces } = data;
  return (
    <>
      <div>
        <Link to="/staff" className="text-sm text-muted-foreground hover:underline">
          ← Users
        </Link>
        <h1 className="mt-2 font-serif text-[40px] leading-none tracking-tight">{user.name || user.email}</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          {user.email} {user.emailVerified ? "" : "(unverified)"} · {user.id}
          {data.staff ? " · staff" : ""}
        </p>
      </div>
      <dl className="grid gap-3 sm:grid-cols-4">
        <Stat label="Signed up" value={day(user.createdAt)} />
        <Stat label="Last seen" value={sessions.lastSeenAt ? dateTime(sessions.lastSeenAt) : "Never"} />
        <Stat label="Active sessions" value={String(sessions.active)} />
        <Stat label="Signs in with" value={signInMethods.join(", ") || "None"} />
      </dl>
      {workspaces.length ? (
        workspaces.map((w) => <WorkspaceCard key={w.id} workspace={w} />)
      ) : (
        <p className="rounded-xl border border-dashed border-border px-6 py-14 text-center text-sm text-muted-foreground">Not in any workspace.</p>
      )}
    </>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-xl border border-border p-4">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="mt-1 truncate text-sm font-medium">{value}</dd>
    </div>
  );
}

function WorkspaceCard({ workspace: w }: { workspace: Workspace }) {
  const { run, error, busy } = useAction();
  return (
    <Card className="block p-6">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="text-lg font-semibold">{w.name}</h2>
        <span className="text-sm text-muted-foreground">
          {PLAN_LABEL[w.plan]} · {w.role} · {w.members} {w.members === 1 ? "member" : "members"}
        </span>
      </div>
      <p className="text-xs text-muted-foreground">{w.id}</p>

      <h3 className="mt-5 text-sm font-semibold">Licences</h3>
      {w.subscriptions.length || w.grants.length ? (
        <ul className="mt-2 divide-y divide-border text-sm">
          {w.subscriptions.map((s) => (
            <li key={s.id} className="flex items-center justify-between gap-4 py-2">
              <span>
                {PLAN_LABEL[s.plan] ?? s.plan} subscription {s.stripe ? "(Stripe)" : "(manual)"}
              </span>
              <span className="text-muted-foreground">
                {s.status}
                {s.periodEnd ? ` · renews ${day(s.periodEnd)}` : ""}
              </span>
            </li>
          ))}
          {w.grants.map((g) => (
            <li key={g.id} className="flex items-center justify-between gap-4 py-2">
              <span>
                {PLAN_LABEL[g.plan]} licence{g.seats ? `, ${g.seats} seats` : ""} through {lastDay(g.endsAt)}
                <span className="block text-xs text-muted-foreground">
                  {g.note} · by {g.issuedBy ?? "deleted user"} on {day(g.startsAt)}
                </span>
              </span>
              <span className="flex items-center gap-3">
                {g.revokedAt ? (
                  <Badge variant="outline">Revoked {day(g.revokedAt)}</Badge>
                ) : g.current ? (
                  <>
                    <Badge variant="success">Current</Badge>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      onClick={() => {
                        if (confirm(`End this ${PLAN_LABEL[g.plan]} licence now?`)) void run(() => endLicence({ data: { grantId: g.id } }));
                      }}
                    >
                      Revoke
                    </Button>
                  </>
                ) : (
                  <Badge variant="outline">Expired</Badge>
                )}
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-2 text-sm text-muted-foreground">None: on the Free plan.</p>
      )}
      <ErrorText>{error}</ErrorText>
      <IssueLicence organizationId={w.id} />

      <h3 className="mt-6 text-sm font-semibold">Deleted guides</h3>
      <Trash workspace={w} />
    </Card>
  );
}

function IssueLicence({ organizationId }: { organizationId: string }) {
  const { run, error, busy } = useAction();
  const [plan, setPlan] = useState<"pro" | "enterprise">("pro");

  function issue(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const seats = Number(data.get("seats"));
    void run(() =>
      grantLicence({
        data: {
          organizationId,
          plan,
          until: String(data.get("until")),
          seats: plan === "enterprise" && seats ? seats : undefined,
          note: String(data.get("note")),
        },
      }),
    ).then((done) => done && form.reset());
  }

  return (
    <form onSubmit={issue} className="mt-3 flex flex-wrap items-end gap-3 rounded-lg bg-muted/40 p-3">
      <Field>
        <span className="text-xs">Plan</span>
        <NativeSelect value={plan} onChange={(e) => setPlan(e.target.value as "pro" | "enterprise")}>
          <option value="pro">Pro</option>
          <option value="enterprise">Enterprise</option>
        </NativeSelect>
      </Field>
      <Field>
        <span className="text-xs">Through (UTC)</span>
        <Input name="until" type="date" required min={isoDay(0)} defaultValue={isoDay(365)} className="h-8" />
      </Field>
      {plan === "enterprise" ? (
        <Field>
          <span className="text-xs">Seats</span>
          <Input name="seats" type="number" min={1} placeholder="1000" className="h-8 w-24" />
        </Field>
      ) : null}
      <Field className="min-w-48 flex-1">
        <span className="text-xs">Reason</span>
        <Input name="note" required placeholder="Ticket number, deal, trial…" className="h-8" />
      </Field>
      <Button type="submit" size="sm" disabled={busy}>
        Issue licence
      </Button>
      <ErrorText>{error}</ErrorText>
    </form>
  );
}

function Trash({ workspace: w }: { workspace: Workspace }) {
  const { run, error, busy } = useAction();
  if (!w.trash.length) return <p className="mt-2 text-sm text-muted-foreground">Nothing deleted in the last 30 days.</p>;
  return (
    <>
      <ul className="mt-2 divide-y divide-border text-sm">
        {w.trash.map((g) => (
          <li key={g.id} className="flex items-center justify-between gap-4 py-2">
            <span>
              <span className="font-medium">{g.title || "(untitled)"}</span>
              {g.visibility === "private" ? <span className="ml-2 text-xs text-muted-foreground">private</span> : null}
              <span className="block text-xs text-muted-foreground">
                Deleted {dateTime(g.deletedAt)}
                {g.deletedBy ? ` by ${g.deletedBy}` : ""} · owner {g.ownerEmail ?? "none"} · purged {day(g.purgeAt)}
              </span>
            </span>
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              onClick={() => void run(() => restoreGuideForUser({ data: { organizationId: w.id, guideId: g.id } }))}
            >
              Restore
            </Button>
          </li>
        ))}
      </ul>
      <ErrorText>{error}</ErrorText>
    </>
  );
}
