import { createFileRoute, getRouteApi, useRouter } from "@tanstack/react-router";
import { LogOut } from "lucide-react";
import { AccountDetails, ChangePassword } from "@/components/profile/Profile";
import { authClient } from "@/lib/auth-client";
import { title } from "@/lib/title";
import { Button } from "@guiderecorder/ui/button";

export const Route = createFileRoute("/_app/profile")({
  head: () => title("Profile"),
  component: Profile,
});

const app = getRouteApi("/_app");

function Profile() {
  const { user } = app.useLoaderData();
  const router = useRouter();

  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-7 px-5 py-8 md:px-9">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="truncate font-serif text-[40px] leading-none tracking-tight">{user.name || "Profile"}</h1>
          <p className="mt-2 text-sm text-muted-foreground">{user.email}</p>
        </div>
        <Button
          variant="outline"
          onClick={async () => {
            await authClient.signOut();
            await router.navigate({ to: "/sign-in" });
          }}
        >
          <LogOut /> Sign out
        </Button>
      </div>
      <AccountDetails name={user.name} email={user.email} />
      <ChangePassword />
    </div>
  );
}
