import { createFileRoute } from "@tanstack/react-router";
import { StaffUser } from "@/components/staff/StaffUser";
import { getStaffUser } from "@/server/staff";

export const Route = createFileRoute("/staff/users/$userId")({
  loader: ({ params }) => getStaffUser({ data: { userId: params.userId } }),
  component: Page,
});

function Page() {
  return <StaffUser data={Route.useLoaderData()} />;
}
