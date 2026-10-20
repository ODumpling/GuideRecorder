import { useEffect, useState } from "react";
import { Dialog } from "radix-ui";
import { Check, Copy, X } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { isWindows, thisComputer } from "@/lib/platform";
import { Row, Select, Toggle } from "@/components/settings/controls";
import { WEB_APP_URL, syncStore, useSync } from "@/sync/sync";

/**
 * Signing in to the web app (WEB_APP_URL), start to finish: the code to
 * approve in the browser, then the workspace and what syncs.
 */
export function SignInDialog() {
  const open = useSync((s) => s.signInOpen);
  const account = useSync((s) => s.account);
  const signIn = useSync((s) => s.signIn);
  const error = useSync((s) => s.error);
  const store = syncStore.getState();

  // Opening the dialog opens the browser.
  useEffect(() => {
    const { account, signIn } = syncStore.getState();
    if (open && !account?.signedIn && !signIn) void syncStore.getState().startSignIn(WEB_APP_URL);
  }, [open]);

  return (
    <Dialog.Root open={open} onOpenChange={(o) => store.openSignIn(o)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/30" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-[480px] max-w-[calc(100vw-2rem)] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-xl border border-border bg-background p-5 shadow-xl">
          <div className="flex items-start justify-between gap-3">
            <div>
              <Dialog.Title className="text-base font-semibold">
                {account?.signedIn ? "Signed in" : "Sign in to sync"}
              </Dialog.Title>
              <Dialog.Description className="mt-1 text-xs text-muted-foreground">
                Sync guides with your GuideRecorder workspace on the web (Pro and Enterprise). Guides stay on {thisComputer()}; only the ones you
                choose are uploaded, with blurred areas burned into their screenshots.
              </Dialog.Description>
            </div>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close">
                <X />
              </Button>
            </Dialog.Close>
          </div>
          {account?.signedIn ? <SignedIn /> : signIn ? <Approve userCode={signIn.userCode} url={signIn.verificationUriComplete} /> : <Start />}
          {error && <p className="text-xs break-words text-[var(--status-danger)]">{error}</p>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** Signing in didn't start, or stopped (expired, denied, cancelled). */
function Start() {
  return (
    <div className="flex items-center justify-between gap-3">
      <p className="text-xs text-muted-foreground">Your browser opens so you can sign in there and approve {thisComputer()}.</p>
      <Button onClick={() => void syncStore.getState().startSignIn(WEB_APP_URL)}>Open browser</Button>
    </div>
  );
}

/** Approve the code in the browser; the store polls until then. */
function Approve({ userCode, url }: { userCode: string; url: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => setCopied(false), [url]);
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm">In the browser window that opened, check that it shows this code, then approve it:</p>
      <p className="rounded-lg bg-foreground/[0.05] py-3 text-center font-mono text-2xl font-semibold tracking-[0.3em]">{userCode}</p>
      <div className="flex flex-col gap-1.5">
        <p className="text-xs text-muted-foreground">No window opened? Open this link in your browser:</p>
        <div className="flex items-center gap-2">
          <span className="min-w-0 flex-1 rounded-md border border-border px-2.5 py-1.5 font-mono text-xs break-all select-all">{url}</span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void navigator.clipboard.writeText(url).then(() => setCopied(true), () => {})}
          >
            {copied ? <Check /> : <Copy />} {copied ? "Copied" : "Copy"}
          </Button>
        </div>
      </div>
      <div className="flex items-center justify-between">
        <span className="text-xs text-muted-foreground">Waiting for approval…</span>
        <Button variant="ghost" size="sm" onClick={() => syncStore.getState().cancelSignIn()}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

/** Signed in; pick the workspace and what syncs. */
function SignedIn() {
  const server = useSync((s) => s.account?.server);
  const workspaces = useSync((s) => s.workspaces);
  const state = useSync((s) => s.state);
  const store = syncStore.getState();
  return (
    <div className="flex flex-col">
      <p className="pb-1 text-sm">
        {isWindows() ? "This PC" : "This Mac"} is signed in to <span className="font-mono text-xs break-all">{server}</span>.
      </p>
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
            : "Or turn on single guides: right-click a guide → Sync to Web."
        }
      >
        <Toggle label="Sync the whole library" checked={state.library} onChange={(on) => void store.setLibrary(on)} />
      </Row>
      <div className="flex justify-end pt-3">
        <Button onClick={() => store.openSignIn(false)}>Done</Button>
      </div>
    </div>
  );
}
