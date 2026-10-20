import { createFileRoute } from "@tanstack/react-router";
import { PolicySettings } from "@/components/admin/PolicySettings";
import { ScimConnections } from "@/components/admin/ScimConnections";
import { SsoProviders } from "@/components/admin/SsoProviders";
import { Members } from "@/components/dashboard/Members";
import { title } from "@/lib/title";
import { getAdmin } from "@/server/admin";

export const Route = createFileRoute("/_app/admin")({
  loader: () => getAdmin(),
  head: () => title("Admin"),
  component: Admin,
});

function Admin() {
  const data = Route.useLoaderData();
  const { seats } = data;

  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-7 px-5 py-8 md:px-9">
      <div>
        <h1 className="font-serif text-[40px] leading-none tracking-tight">Admin</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          {seats.used} of {seats.total} seats used
        </p>
      </div>

      <Members
        workspaceId={data.workspaceId}
        currentUserId={data.userId}
        members={data.members}
        invitations={data.invitations}
        canInvite
        canManage
        seatsLeft={seats.total - seats.used}
      />

      <SsoProviders providers={data.sso} baseURL={data.baseURL} />
      <ScimConnections connections={data.scim} endpoint={`${data.baseURL}/api/auth/scim/v2`} />
      <PolicySettings policy={data.policy} hasSso={data.sso.some((p) => p.domainVerified)} />
    </div>
  );
}
