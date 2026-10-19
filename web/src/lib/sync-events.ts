import "@tanstack/react-start/server-only";
import postgres from "postgres";
import { env } from "./env";
import { SYNC_CHANNEL } from "./sync";

// Change nudges for the sync event stream. Pushes `pg_notify` on commit;
// each server process listens once and fans out to its open streams, so
// every instance hears every change.

type Listener = (seq: string) => void;

const listeners = new Map<string, Set<Listener>>();
let listening: Promise<unknown> | undefined;

function listen() {
  listening ??= postgres(env().DATABASE_URL, { max: 1 })
    .listen(SYNC_CHANNEL, (payload) => {
      const [organizationId, seq] = payload.split(" ");
      for (const listener of listeners.get(organizationId!) ?? []) listener(seq!);
    })
    .catch((error) => {
      listening = undefined;
      throw error;
    });
  return listening;
}

/** Calls `listener` with the new seq whenever a guide in the workspace changes. */
export async function onWorkspaceChange(organizationId: string, listener: Listener): Promise<() => void> {
  await listen();
  const set = listeners.get(organizationId) ?? new Set();
  set.add(listener);
  listeners.set(organizationId, set);
  return () => {
    set.delete(listener);
    if (!set.size) listeners.delete(organizationId);
  };
}
