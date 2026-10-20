import { createFileRoute, Link } from "@tanstack/react-router";
import { Library } from "@/components/guides/Library";
import { librarySearch } from "@/lib/library-search";
import { title } from "@/lib/title";
import { getLibrary } from "@/server/guides";

export const Route = createFileRoute("/_app/library")({
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
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-7 px-5 py-8 md:px-9">
        <h1 className="font-serif text-[40px] leading-none tracking-tight">All guides</h1>
        <p className="rounded-xl border border-dashed border-border px-6 py-14 text-center text-sm text-muted-foreground">
          {data.reason}{" "}
          <Link to="/dashboard" className="text-brand hover:underline">
            Workspace settings
          </Link>
        </p>
      </div>
    );
  }
  return <Library data={data} search={search} />;
}
