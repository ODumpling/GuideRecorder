import { createFileRoute } from "@tanstack/react-router";
import { ApprovalPolicies } from "@/components/dashboard/ApprovalPolicies";
import { Billing } from "@/components/dashboard/Billing";
import { Members } from "@/components/dashboard/Members";
import { SharingSettings } from "@/components/dashboard/SharingSettings";
import { Trash } from "@/components/guides/Trash";
import { title } from "@/lib/title";
import { getDashboard } from "@/server/dashboard";
import { getSyncedGuides } from "@/server/guides";
import { getApprovalSettings } from "@/server/reviews";
import { getWorkspaceSharing } from "@/server/shares";

export const Route = createFileRoute("/_app/dashboard")({
  loader: async () => {
    const [dashboard, guides, approvals, sharing] = await Promise.all([
      getDashboard(),
      getSyncedGuides(),
      getApprovalSettings(),
      getWorkspaceSharing(),
    ]);
    return { ...dashboard, guides, approvals, sharing };
  },
  head: () => title("Workspace"),
  component: Dashboard,
});

const PLAN_LABEL = { free: "Free", pro: "Pro", enterprise: "Enterprise" } as const;

function formatBytes(bytes: number) {
  if (bytes < 1024 ** 2) return `${Math.round(bytes / 1024)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

function Dashboard() {
  const { user, active, billingEnabled, can, guides, approvals, sharing } = Route.useLoaderData();

  if (!active) {
    return (
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-7 px-5 py-8 md:px-9">
        <h1 className="font-serif text-[40px] leading-none tracking-tight">Workspace</h1>
        <p className="rounded-xl border border-dashed border-border px-6 py-14 text-center text-sm text-muted-foreground">
          You aren&rsquo;t in any workspace.
        </p>
      </div>
    );
  }

  const seats = active.plan.entitlements.seats;
  const storage = active.plan.entitlements.storageBytes;
  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-7 px-5 py-8 md:px-9">
      <div>
        <h1 className="truncate font-serif text-[40px] leading-none tracking-tight">{active.name}</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          {PLAN_LABEL[active.plan.plan]} plan · you&rsquo;re {/^[aeiou]/.test(active.role) ? "an" : "a"} {active.role}
        </p>
      </div>
      <dl className="grid gap-3 sm:grid-cols-3">
        <Stat label="Plan" value={PLAN_LABEL[active.plan.plan]} />
        <Stat label="Seats" value={`${active.members.length} of ${seats}`} used={active.members.length / seats} />
        <Stat label="Storage" value={`${formatBytes(active.storageUsed)} of ${formatBytes(storage)}`} used={active.storageUsed / storage} />
      </dl>
      <Members
        workspaceId={active.id}
        currentUserId={user.id}
        members={active.members}
        invitations={active.invitations}
        canInvite={can.invite}
        canManage={can.manageMembers}
        seatsLeft={seats - active.members.length - active.invitations.length}
      />
      {approvals.available ? <ApprovalPolicies {...approvals} /> : null}
      {sharing ? <SharingSettings key={active.id} {...sharing} /> : null}
      {guides.available ? <Trash {...guides} /> : null}
      <Billing workspaceId={active.id} plan={active.plan.plan} enabled={billingEnabled} canManage={can.manageBilling} />
    </div>
  );
}

function Stat({ label, value, used }: { label: string; value: string; used?: number }) {
  return (
    <div className="rounded-xl border border-border bg-card px-4 py-3">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="mt-1 text-sm font-semibold">{value}</dd>
      {used !== undefined ? (
        <dd className="mt-2 h-1 overflow-hidden rounded-full bg-muted">
          <span className="block h-full rounded-full bg-brand" style={{ width: `${Math.min(100, Math.max(2, used * 100))}%` }} />
        </dd>
      ) : null}
    </div>
  );
}
