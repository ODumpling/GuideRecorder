import { createFileRoute, redirect } from "@tanstack/react-router";
import { ApproveDevice } from "@/components/dashboard/ApproveDevice";
import { title } from "@/lib/title";
import { getViewer } from "@/server/session";

/** Where the desktop and mobile apps send people to approve its sign-in code. */
export const Route = createFileRoute("/device")({
  validateSearch: (search: Record<string, unknown>): { user_code?: string } =>
    typeof search.user_code === "string" ? { user_code: search.user_code } : {},
  beforeLoad: async ({ search }) => {
    if (!(await getViewer())) {
      const next = search.user_code ? `/device?user_code=${encodeURIComponent(search.user_code)}` : "/device";
      throw redirect({ to: "/sign-in", search: { next } });
    }
  },
  head: () => title("Sign in the app"),
  component: Page,
});

function Page() {
  const { user_code } = Route.useSearch();
  return (
    <main className="flex flex-1 items-center justify-center p-6">
      <ApproveDevice initialCode={user_code ?? ""} />
    </main>
  );
}
