import { createFileRoute } from "@tanstack/react-router";
import { z } from "zod";
import { UserSearch } from "@/components/staff/UserSearch";
import { searchUsers } from "@/server/staff";

export const Route = createFileRoute("/staff/")({
  validateSearch: z.object({ q: z.string().optional() }),
  loaderDeps: ({ search }) => search,
  loader: ({ deps }) => searchUsers({ data: deps }),
  component: Page,
});

function Page() {
  return <UserSearch users={Route.useLoaderData()} query={Route.useSearch().q ?? ""} />;
}
