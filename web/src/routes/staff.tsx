import { createFileRoute, Link, Outlet, useRouter } from "@tanstack/react-router";
import { authClient } from "@/lib/auth-client";
import { getStaffViewer } from "@/server/staff";

// The staff console: for GuideRecorder's developers and helpdesk, not
// customers (`lib/staff.ts`). Non-staff get a 404.
export const Route = createFileRoute("/staff")({
  loader: ({ location }) => getStaffViewer({ data: { next: location.href } }),
  head: () => ({ meta: [{ title: "Staff · GuideRecorder" }, { name: "robots", content: "noindex" }] }),
  component: Layout,
});

function Layout() {
  const viewer = Route.useLoaderData();
  const router = useRouter();
  return (
    <div className="flex min-h-dvh flex-col">
      <header className="flex h-12 shrink-0 items-center gap-4 border-b border-border px-5">
        <Link to="/staff" className="font-semibold">
          GuideRecorder <span className="rounded bg-status-warning/15 px-1.5 py-0.5 text-xs font-medium text-status-warning">Staff</span>
        </Link>
        <span className="flex-1" />
        <span className="text-sm text-muted-foreground">{viewer.email}</span>
        <button
          className="text-sm text-muted-foreground hover:text-foreground"
          onClick={async () => {
            await authClient.signOut();
            await router.navigate({ to: "/sign-in" });
          }}
        >
          Sign out
        </button>
      </header>
      <main className="mx-auto flex w-full max-w-5xl flex-col gap-7 px-5 py-8 md:px-9">
        <Outlet />
      </main>
    </div>
  );
}
