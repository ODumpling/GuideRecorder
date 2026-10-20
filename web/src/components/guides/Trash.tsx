import { useRouter } from "@tanstack/react-router";
import { useState } from "react";
import { setGuideTrashed, type getSyncedGuides } from "@/server/guides";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { ErrorText } from "@/components/ui";

type Data = Extract<Awaited<ReturnType<typeof getSyncedGuides>>, { available: true }>;

const date = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/** Guides moved to the trash (from the library), which can be restored for 30 days. */
export function Trash({ trash, canDelete }: Data) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);

  async function restore(guideId: string) {
    setError(null);
    const result = await setGuideTrashed({ data: { guideId, trashed: false } });
    if (!result.ok) setError(result.error);
    else await router.invalidate();
  }

  return (
    <Card className="block p-6">
      <h2 className="text-lg font-semibold">Trash</h2>
      <p className="mt-1 text-sm text-muted-foreground">Deleted guides are kept for 30 days.</p>
      {trash.length ? (
        <ul className="mt-4 divide-y divide-border text-sm">
          {trash.map((g) => (
            <li key={g.id} className="flex items-center justify-between gap-4 py-2">
              <span className="font-medium">{g.title}</span>
              <span className="flex items-center gap-3 text-muted-foreground">
                Deleted {date(g.deletedAt!)}
                {canDelete ? (
                  <Button variant="outline" onClick={() => restore(g.id)}>
                    Restore
                  </Button>
                ) : null}
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-4 text-sm text-muted-foreground">The trash is empty.</p>
      )}
      <ErrorText>{error}</ErrorText>
    </Card>
  );
}
