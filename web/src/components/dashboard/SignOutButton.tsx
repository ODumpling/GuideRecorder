import { useRouter } from "@tanstack/react-router";
import { authClient } from "@/lib/auth-client";
import { Button } from "../ui";

export function SignOutButton() {
  const router = useRouter();
  return (
    <Button
      variant="secondary"
      onClick={async () => {
        await authClient.signOut();
        await router.navigate({ to: "/sign-in" });
      }}
    >
      Sign out
    </Button>
  );
}
