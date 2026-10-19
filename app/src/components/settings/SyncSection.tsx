import { useState } from "react";
import { RotateCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Row, Section, Select, Toggle } from "@/components/settings/controls";
import { timeAgo } from "@/editor/library";
import { syncStore, useSync } from "@/sync/sync";

const DEFAULT_SERVER = "https://app.guiderecorder.app";

/** Settings → Sync: sign in to the web app, pick a workspace, choose what syncs. */
export function SyncSection() {
  const account = useSync((s) => s.account);
  const signIn = useSync((s) => s.signIn);
  const workspaces = useSync((s) => s.workspaces);
  const state = useSync((s) => s.state);
  const running = useSync((s) => s.running);
  const error = useSync((s) => s.error);
  const lastSynced = useSync((s) => s.lastSynced);
  const [server, setServer] = useState(account?.server ?? DEFAULT_SERVER);
  const store = syncStore.getState();
  const enabledCount = Object.values(state.guides).filter((g) => g.enabled).length;

  return (
    <Section
      title="Sync"
      intro="Sync guides with your GuideRecorder workspace on the web (Pro and Enterprise). Guides stay on this Mac; only the ones you choose are uploaded, with blurred areas burned into their screenshots."
    >
      {account?.signedIn ? (
        <>
          <Row label="Signed in" note={account.server}>
            <Button variant="outline" size="sm" onClick={() => void store.signOut()}>
              Sign out
            </Button>
          </Row>
          <Row label="Workspace" note="Guides sync with this workspace.">
            <Select
              label="Workspace"
              value={state.workspaceId ?? ""}
              options={workspaces.map((w) => ({ value: w.id, label: w.name }))}
              onChange={(id) => void store.chooseWorkspace(id)}
            />
          </Row>
          <Row
            label="Sync the whole library"
            note={
              state.library
                ? "Every guide syncs, and guides made on the web appear here."
                : `Only guides you turn on sync (right-click a guide → Sync to Web). ${enabledCount ? `${enabledCount} on.` : ""}`
            }
          >
            <Toggle label="Sync the whole library" checked={state.library} onChange={(on) => void store.setLibrary(on)} />
          </Row>
          <Row label="Status" note={running ? "Syncing…" : lastSynced ? `Last synced ${timeAgo(lastSynced)}` : "Not synced yet"}>
            <Button variant="outline" size="sm" disabled={running} onClick={() => void store.syncNow()}>
              <RotateCw className={running ? "animate-spin" : undefined} /> Sync now
            </Button>
          </Row>
        </>
      ) : signIn ? (
        <div className="flex flex-col gap-3 py-3">
          <p className="text-sm">Approve this code in the browser window that opened:</p>
          <p className="font-mono text-2xl font-semibold tracking-[0.3em]">{signIn.userCode}</p>
          <p className="text-xs text-muted-foreground">
            Waiting for approval… If no window opened, go to <span className="select-all font-mono">{signIn.verificationUri}</span>.
          </p>
          <div>
            <Button variant="ghost" size="sm" onClick={() => store.cancelSignIn()}>
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <Row label="Web app" note="Sign in opens your browser to approve this Mac.">
          <form
            className="flex items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              void store.startSignIn(server);
            }}
          >
            <Input aria-label="Web app address" className="h-8 w-60" value={server} onChange={(e) => setServer(e.target.value)} />
            <Button size="sm" type="submit" disabled={!server.trim()}>
              Sign in
            </Button>
          </form>
        </Row>
      )}
      {error && <p className="mt-2 text-xs text-[var(--status-danger)]">{error}</p>}
    </Section>
  );
}
