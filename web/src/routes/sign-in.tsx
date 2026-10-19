import { createFileRoute, redirect } from "@tanstack/react-router";
import { AuthForm } from "@/components/AuthForm";
import { safeNext } from "@/lib/redirect";
import { title } from "@/lib/title";
import { getProviders, getViewer } from "@/server/session";

export const Route = createFileRoute("/sign-in")({
  validateSearch: (search: Record<string, unknown>): { next?: string } =>
    typeof search.next === "string" ? { next: search.next } : {},
  beforeLoad: async ({ search }) => {
    if (await getViewer()) throw redirect({ href: safeNext(search.next) });
  },
  loader: () => getProviders(),
  head: () => title("Sign in"),
  component: Page,
});

function Page() {
  const providers = Route.useLoaderData();
  const { next } = Route.useSearch();
  return (
    <main className="flex flex-1 items-center justify-center p-6">
      <AuthForm mode="sign-in" providers={providers} next={safeNext(next)} />
    </main>
  );
}
