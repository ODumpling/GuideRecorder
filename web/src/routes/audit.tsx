import { createFileRoute, Link } from "@tanstack/react-router";
import { AuditLog } from "@/components/audit/AuditLog";
import { Card } from "@/components/ui";
import { auditFilters, type AuditSearch } from "@/lib/audit-filters";
import { title } from "@/lib/title";
import { getAuditLog } from "@/server/audit";

export const Route = createFileRoute("/audit")({
  validateSearch: (search: Record<string, unknown>): AuditSearch => {
    const parsed = auditFilters.safeParse(search);
    return parsed.success ? parsed.data : {};
  },
  loaderDeps: ({ search }) => search,
  loader: ({ deps }) => getAuditLog({ data: deps }),
  head: () => title("Audit log"),
  component: Page,
});

function Page() {
  const data = Route.useLoaderData();
  const search = Route.useSearch();
  if (!data.allowed) {
    return (
      <div className="mx-auto grid w-full max-w-3xl gap-6 p-6">
        <Card className="grid gap-3">
          <h1 className="font-serif text-3xl">Audit log</h1>
          <p className="text-sm text-muted-foreground">{data.error}</p>
          <Link to="/dashboard" className="text-sm text-brand hover:underline">
            Back to the dashboard
          </Link>
        </Card>
      </div>
    );
  }
  return <AuditLog data={data} search={search} />;
}
