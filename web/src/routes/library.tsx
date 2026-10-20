import { createFileRoute, Link } from "@tanstack/react-router";
import { Library } from "@/components/guides/Library";
import { Card } from "@/components/ui";
import { librarySearch } from "@/lib/library-search";
import { title } from "@/lib/title";
import { getLibrary } from "@/server/guides";

export const Route = createFileRoute("/library")({
  validateSearch: librarySearch,
  loaderDeps: ({ search }) => search,
  loader: ({ deps }) => getLibrary({ data: deps }),
  head: () => title("Library"),
  component: Page,
});

function Page() {
  const data = Route.useLoaderData();
  const search = Route.useSearch();
  if (!data.available) {
    return (
      <main className="mx-auto grid w-full max-w-md gap-3 p-6">
        <Card className="grid gap-3 text-sm">
          <p>{data.reason}</p>
          <Link to="/dashboard" className="text-brand hover:underline">
            Go to the dashboard
          </Link>
        </Card>
      </main>
    );
  }
  return <Library data={data} search={search} />;
}
