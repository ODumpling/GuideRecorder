import { createFileRoute, Link } from "@tanstack/react-router";
import { PolicySettings } from "@/components/admin/PolicySettings";
import { ScimConnections } from "@/components/admin/ScimConnections";
import { SsoProviders } from "@/components/admin/SsoProviders";
import { Members } from "@/components/dashboard/Members";
import { Card } from "@/components/ui";
import { title } from "@/lib/title";
import { getAdmin } from "@/server/admin";

export const Route = createFileRoute("/admin")({
  loader: () => getAdmin(),
  head: () => title("Admin"),
  component: Admin,
});

function Admin() {
  const data = Route.useLoaderData();
  const { enterprise, seats } = data;

  return (
    <div className="mx-auto grid w-full max-w-4xl gap-6 p-6">
      <header className="flex flex-wrap items-center justify-between gap-4">
        <div>
          <Link to="/dashboard" className="text-sm text-muted-foreground hover:underline">
            ← Dashboard
          </Link>
          <h1 className="font-serif text-3xl">Admin</h1>
        </div>
        <p className="text-sm text-muted-foreground">
          {seats.used} of {seats.total} seats used
        </p>
      </header>

      <Members
        workspaceId={data.workspaceId}
        currentUserId={data.userId}
        members={data.members}
        invitations={data.invitations}
        canInvite
        canManage
        seatsLeft={seats.total - seats.used}
      />

      {enterprise ? (
        <>
          <SsoProviders providers={data.sso} baseURL={data.baseURL} />
          <ScimConnections connections={data.scim} endpoint={`${data.baseURL}/api/auth/scim/v2`} />
          <PolicySettings policy={data.policy} hasSso={data.sso.some((p) => p.domainVerified)} />
        </>
      ) : (
        <Card>
          <h2 className="text-lg font-semibold">Single sign-on, provisioning and policies</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            SSO with your identity provider (OIDC or SAML), SCIM provisioning, sharing policy and data retention come with Enterprise.
          </p>
        </Card>
      )}
    </div>
  );
}
