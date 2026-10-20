import { Link, useRouter } from "@tanstack/react-router";
import { useState } from "react";
import { setGuideTrashed, type getSyncedGuides } from "@/server/guides";
import { Button, Card, ErrorText } from "../ui";

type Data = Extract<Awaited<ReturnType<typeof getSyncedGuides>>, { available: true }>;

const date = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/** Guides synced from the desktop app, with the trash. */
export function SyncedGuides({ guides, trash, canDelete }: Data) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);

  async function setTrashed(guideId: string, trashed: boolean) {
    setError(null);
    const result = await setGuideTrashed({ data: { guideId, trashed } });
    if (!result.ok) setError(result.error);
    else await router.invalidate();
  }

  return (
    <Card>
      <div className="mb-4 flex items-center justify-between gap-4">
        <h2 className="text-lg font-semibold">Guides</h2>
        <Link to="/library" className="text-sm text-brand hover:underline">
          Open the library →
        </Link>
      </div>
      {guides.length ? (
        <ul className="divide-y divide-border">
          {guides.map((g) => (
            <li key={g.id} className="flex items-center justify-between gap-4 py-2 text-sm">
              <Link to="/guides/$guideId" params={{ guideId: g.id }} className="font-medium hover:underline">
                {g.title}
              </Link>
              <span className="flex items-center gap-3 text-muted-foreground">
                {g.steps} {g.steps === 1 ? "step" : "steps"} · {date(g.updatedAt)} {g.source === "web" ? "on the web" : "in the app"}
                {canDelete ? (
                  <Button variant="danger" className="h-8 px-3" onClick={() => setTrashed(g.id, true)}>
                    Delete
                  </Button>
                ) : null}
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-sm text-muted-foreground">
          No synced guides yet. In the desktop app, sign in from Settings → Sync and turn sync on for a guide.
        </p>
      )}
      {trash.length ? (
        <details className="mt-4 text-sm">
          <summary className="cursor-pointer text-muted-foreground">Trash ({trash.length})</summary>
          <ul className="mt-2 divide-y divide-border">
            {trash.map((g) => (
              <li key={g.id} className="flex items-center justify-between gap-4 py-2">
                <span>{g.title}</span>
                <span className="flex items-center gap-3 text-muted-foreground">
                  Deleted {date(g.deletedAt!)}, kept for 30 days
                  {canDelete ? (
                    <Button variant="secondary" className="h-8 px-3" onClick={() => setTrashed(g.id, false)}>
                      Restore
                    </Button>
                  ) : null}
                </span>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
      <ErrorText>{error}</ErrorText>
    </Card>
  );
}
