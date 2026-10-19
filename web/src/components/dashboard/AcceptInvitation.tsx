"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";
import { authClient } from "@/lib/auth-client";
import { Button, Card, ErrorText } from "../ui";

export function AcceptInvitation({ invitationId }: { invitationId: string }) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);

  async function respond(accept: boolean) {
    setError(null);
    const { error } = accept
      ? await authClient.organization.acceptInvitation({ invitationId })
      : await authClient.organization.rejectInvitation({ invitationId });
    if (error) return setError(error.message ?? "This invitation can't be used.");
    router.push("/dashboard");
    router.refresh();
  }

  return (
    <Card className="w-full max-w-sm">
      <h1 className="mb-4 font-serif text-3xl">Join the workspace?</h1>
      <div className="flex gap-2">
        <Button onClick={() => respond(true)}>Accept</Button>
        <Button variant="secondary" onClick={() => respond(false)}>
          Decline
        </Button>
      </div>
      <div className="mt-2">
        <ErrorText>{error}</ErrorText>
      </div>
    </Card>
  );
}
