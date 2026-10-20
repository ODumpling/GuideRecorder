import { createFileRoute, Outlet } from "@tanstack/react-router";
import { StaffShell } from "@/components/staff/StaffShell";
import { getStaffViewer } from "@/server/staff";

// The staff console: for GuideRecorder's developers and helpdesk, not
// customers (`lib/staff.ts`). Non-staff get a 404.
export const Route = createFileRoute("/staff")({
  loader: ({ location }) => getStaffViewer({ data: { next: location.href } }),
  head: () => ({ meta: [{ title: "Staff · GuideRecorder" }, { name: "robots", content: "noindex" }] }),
  component: Layout,
});

function Layout() {
  return (
    <StaffShell viewer={Route.useLoaderData()}>
      <Outlet />
    </StaffShell>
  );
}
