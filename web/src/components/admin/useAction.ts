import { useRouter } from "@tanstack/react-router";
import { useState } from "react";

/** Runs a server function, shows its error, and reloads the page's data on success. */
export function useAction() {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function run<T extends { ok: boolean }>(action: () => Promise<T>): Promise<Extract<T, { ok: true }> | null> {
    setBusy(true);
    setError(null);
    try {
      const result = await action();
      if (!result.ok) {
        setError((result as unknown as { error: string }).error);
        return null;
      }
      await router.invalidate();
      return result as Extract<T, { ok: true }>;
    } catch (e) {
      setError(e instanceof Error ? e.message : "Something went wrong.");
      return null;
    } finally {
      setBusy(false);
    }
  }

  return { run, error, busy };
}
