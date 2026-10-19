import { useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { authClient } from "@/lib/auth-client";
import { ROLE_NAMES, type RoleName } from "@/lib/permissions";
import { Button, Card, ErrorText, Input } from "../ui";

interface Props {
  workspaceId: string;
  currentUserId: string;
  members: { id: string; userId: string; role: string; name: string; email: string }[];
  invitations: { id: string; email: string; role: string | null }[];
  canInvite: boolean;
  canManage: boolean;
  seatsLeft: number;
}

/** Roles that can be given to others; ownership isn't handed out from here. */
const ASSIGNABLE = ROLE_NAMES.filter((r) => r !== "owner");

export function Members({ workspaceId, currentUserId, members, invitations, canInvite, canManage, seatsLeft }: Props) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);

  async function act(action: () => Promise<{ error: { message?: string } | null }>) {
    setError(null);
    const { error } = await action();
    if (error) setError(error.message ?? "Something went wrong.");
    else await router.invalidate();
  }

  function invite(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    void act(async () => {
      const result = await authClient.organization.inviteMember({
        organizationId: workspaceId,
        email: String(data.get("email")),
        role: data.get("role") as Exclude<RoleName, "owner">,
      });
      if (!result.error) form.reset();
      return result;
    });
  }

  return (
    <Card>
      <h2 className="mb-4 text-lg font-semibold">Members</h2>
      <ul className="divide-y divide-border">
        {members.map((m) => (
          <li key={m.id} className="flex items-center justify-between gap-4 py-2 text-sm">
            <span>
              <span className="font-medium">{m.name}</span> <span className="text-muted-foreground">{m.email}</span>
            </span>
            {canManage && m.role !== "owner" && m.userId !== currentUserId ? (
              <span className="flex items-center gap-2">
                <select
                  aria-label={`Role of ${m.email}`}
                  className="h-8 rounded-md border border-input bg-card px-2"
                  value={m.role}
                  onChange={(e) =>
                    act(() =>
                      authClient.organization.updateMemberRole({
                        organizationId: workspaceId,
                        memberId: m.id,
                        role: e.target.value as Exclude<RoleName, "owner">,
                      }),
                    )
                  }
                >
                  {ASSIGNABLE.map((r) => (
                    <option key={r} value={r}>
                      {r}
                    </option>
                  ))}
                </select>
                <Button
                  variant="danger"
                  className="h-8 px-3"
                  onClick={() => act(() => authClient.organization.removeMember({ organizationId: workspaceId, memberIdOrEmail: m.id }))}
                >
                  Remove
                </Button>
              </span>
            ) : (
              <span className="capitalize text-muted-foreground">{m.role}</span>
            )}
          </li>
        ))}
        {invitations.map((i) => (
          <li key={i.id} className="flex items-center justify-between gap-4 py-2 text-sm text-muted-foreground">
            <span>
              {i.email} <em>invited as {i.role}</em>
            </span>
            {canInvite && (
              <Button variant="secondary" className="h-8 px-3" onClick={() => act(() => authClient.organization.cancelInvitation({ invitationId: i.id }))}>
                Cancel
              </Button>
            )}
          </li>
        ))}
      </ul>
      {canInvite &&
        (seatsLeft > 0 ? (
          <form onSubmit={invite} className="mt-4 flex flex-wrap gap-2">
            <Input name="email" type="email" placeholder="colleague@example.com" required className="max-w-xs" />
            <select name="role" aria-label="Role" defaultValue="editor" className="h-9 rounded-lg border border-input bg-card px-2 text-sm">
              {ASSIGNABLE.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
            <Button type="submit">Invite</Button>
          </form>
        ) : (
          <p className="mt-4 text-sm text-muted-foreground">No seats left. Upgrade to invite more people.</p>
        ))}
      <div className="mt-2">
        <ErrorText>{error}</ErrorText>
      </div>
    </Card>
  );
}
