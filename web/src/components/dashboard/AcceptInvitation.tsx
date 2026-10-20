import { useRouter } from "@tanstack/react-router";
import { useState } from "react";
import { authClient } from "@/lib/auth-client";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { ErrorText } from "@/components/ui";

export function AcceptInvitation({ invitationId }: { invitationId: string }) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);

  async function respond(accept: boolean) {
    setError(null);
    const { error } = accept
      ? await authClient.organization.acceptInvitation({ invitationId })
      : await authClient.organization.rejectInvitation({ invitationId });
    if (error) return setError(error.message ?? "This invitation can't be used.");
    await router.navigate({ to: "/dashboard" });
  }

  return (
    <Card className="p-6 block w-full max-w-sm">
      <h1 className="mb-4 font-serif text-3xl">Join the workspace?</h1>
      <div className="flex gap-2">
        <Button variant="brand" size="lg" onClick={() => respond(true)}>Accept</Button>
        <Button variant="outline" size="lg" onClick={() => respond(false)}>
          Decline
        </Button>
      </div>
      <div className="mt-2">
        <ErrorText>{error}</ErrorText>
      </div>
    </Card>
  );
}
