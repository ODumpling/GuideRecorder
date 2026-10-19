import { createFileRoute, redirect } from "@tanstack/react-router";
import { AcceptInvitation } from "@/components/dashboard/AcceptInvitation";
import { title } from "@/lib/title";
import { getViewer } from "@/server/session";

export const Route = createFileRoute("/invitations/$id")({
  beforeLoad: async ({ params }) => {
    if (!(await getViewer())) throw redirect({ to: "/sign-in", search: { next: `/invitations/${params.id}` } });
  },
  head: () => title("Invitation"),
  component: Page,
});

function Page() {
  const { id } = Route.useParams();
  return (
    <main className="flex flex-1 items-center justify-center p-6">
      <AcceptInvitation invitationId={id} />
    </main>
  );
}
