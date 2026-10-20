import { createFileRoute } from "@tanstack/react-router";
import { AuditLog } from "@/components/audit/AuditLog";
import { auditFilters, type AuditSearch } from "@/lib/audit-filters";
import { title } from "@/lib/title";
import { getAuditLog } from "@/server/audit";

export const Route = createFileRoute("/_app/audit")({
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
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-7 px-5 py-8 md:px-9">
        <h1 className="font-serif text-[40px] leading-none tracking-tight">Audit log</h1>
        <p className="rounded-xl border border-dashed border-border px-6 py-14 text-center text-sm text-muted-foreground">{data.error}</p>
      </div>
    );
  }
  return <AuditLog data={data} search={search} />;
}
