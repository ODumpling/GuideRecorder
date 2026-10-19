import type { Metadata } from "next";
import { redirect } from "next/navigation";
import { connection } from "next/server";
import { Billing } from "@/components/dashboard/Billing";
import { Members } from "@/components/dashboard/Members";
import { SignOutButton } from "@/components/dashboard/SignOutButton";
import { WorkspaceSwitcher } from "@/components/dashboard/WorkspaceSwitcher";
import { Card } from "@/components/ui";
import { authFeatures, getSession } from "@/lib/auth";
import { loadDashboard } from "@/lib/dashboard";
import { roles, type RoleName } from "@/lib/permissions";

export const metadata: Metadata = { title: "Dashboard" };

const PLAN_LABEL = { free: "Free", pro: "Pro", enterprise: "Enterprise" } as const;

function formatBytes(bytes: number) {
  if (bytes < 1024 ** 2) return `${Math.round(bytes / 1024)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export default async function Dashboard() {
  await connection();
  const session = await getSession();
  if (!session) redirect("/sign-in?next=/dashboard");
  const { workspaces, active } = await loadDashboard(session.user.id, session.session.activeOrganizationId);

  const can = (permission: Parameters<(typeof roles)[RoleName]["authorize"]>[0]) =>
    Boolean(active && active.role in roles && roles[active.role as RoleName].authorize(permission).success);

  return (
    <div className="mx-auto grid w-full max-w-4xl gap-6 p-6">
      <header className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex items-center gap-3">
          <span className="font-serif text-2xl">GuideRecorder</span>
          <WorkspaceSwitcher workspaces={workspaces} activeId={active?.id ?? null} />
        </div>
        <div className="flex items-center gap-3 text-sm text-muted-foreground">
          {session.user.email}
          <SignOutButton />
        </div>
      </header>

      {active ? (
        <>
          <Card>
            <h1 className="font-serif text-3xl">{active.name}</h1>
            <dl className="mt-4 grid grid-cols-2 gap-4 text-sm sm:grid-cols-4">
              <div>
                <dt className="text-muted-foreground">Plan</dt>
                <dd className="font-medium">{PLAN_LABEL[active.plan.plan]}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Your role</dt>
                <dd className="font-medium capitalize">{active.role}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Seats</dt>
                <dd className="font-medium">
                  {active.members.length} of {active.plan.entitlements.seats}
                </dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Storage</dt>
                <dd className="font-medium">
                  {formatBytes(active.storageUsed)} of {formatBytes(active.plan.entitlements.storageBytes)}
                </dd>
              </div>
            </dl>
          </Card>
          <Members
            workspaceId={active.id}
            currentUserId={session.user.id}
            members={active.members}
            invitations={active.invitations.map((i) => ({ ...i, expiresAt: i.expiresAt.toISOString() }))}
            canInvite={can({ invitation: ["create"] })}
            canManage={can({ member: ["update"] })}
            seatsLeft={active.plan.entitlements.seats - active.members.length - active.invitations.length}
          />
          <Billing
            workspaceId={active.id}
            plan={active.plan.plan}
            enabled={authFeatures().billing}
            canManage={can({ billing: ["manage"] })}
          />
        </>
      ) : (
        <Card>You aren&rsquo;t in any workspace.</Card>
      )}
    </div>
  );
}
