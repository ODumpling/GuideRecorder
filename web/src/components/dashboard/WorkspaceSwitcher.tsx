import { useRouter } from "@tanstack/react-router";
import { authClient } from "@/lib/auth-client";

export function WorkspaceSwitcher({ workspaces, activeId }: { workspaces: { id: string; name: string }[]; activeId: string | null }) {
  const router = useRouter();
  if (workspaces.length < 2) return null;
  return (
    <select
      aria-label="Workspace"
      className="h-9 rounded-lg border border-input bg-card px-2 text-sm"
      value={activeId ?? ""}
      onChange={async (event) => {
        await authClient.organization.setActive({ organizationId: event.target.value });
        await router.invalidate();
      }}
    >
      {workspaces.map((w) => (
        <option key={w.id} value={w.id}>
          {w.name}
        </option>
      ))}
    </select>
  );
}
