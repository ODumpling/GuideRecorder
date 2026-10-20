import { createFileRoute, Outlet } from "@tanstack/react-router";
import { AppShell } from "@/components/shell/AppShell";
import { getShell } from "@/server/shell";

// The signed-in pages, in the library's frame (components/shell).
export const Route = createFileRoute("/_app")({
  loader: ({ location }) => getShell({ data: { next: location.href } }),
  // Counts change with the library; pages invalidate after changing it.
  staleTime: 30_000,
  component: Layout,
});

function Layout() {
  return (
    <AppShell shell={Route.useLoaderData()}>
      <Outlet />
    </AppShell>
  );
}
