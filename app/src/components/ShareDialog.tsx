import { useEffect, useState, type FormEvent } from "react";
import { Dialog } from "radix-ui";
import { Copy, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useEditor } from "@/editor/editor";
import type { ShareLink } from "@/sync/share";
import { shareStore, useShare } from "@/sync/sync";

// Share a synced guide with a link: make one, copy it, see its views,
// revoke it. The links live on the web app; this calls its share API.

const DAY = 24 * 60 * 60 * 1000;
const EXPIRY_DAYS = [1, 7, 30, 90, 365];
const SELECT =
  "h-8 w-full min-w-0 rounded-md border border-input bg-transparent px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60";

const date = (iso: string) => new Date(iso).toLocaleDateString(undefined, { dateStyle: "medium" });

function describe(link: ShareLink) {
  return [
    link.views === 1 ? "1 view" : `${link.views} views`,
    link.revisionId ? "this version" : "latest version",
    link.hasPassword ? "password" : null,
    link.allowDownload ? "download allowed" : null,
    link.state === "expired"
      ? `expired ${date(link.expiresAt!)}`
      : link.state === "blocked"
        ? "blocked by workspace settings"
        : link.expiresAt
          ? `expires ${date(link.expiresAt)}`
          : "never expires",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** The open share dialog (Share… in a guide's menu). */
export function ShareDialog() {
  const guideId = useShare((s) => s.guideId);
  const title = useEditor((s) => s.guides.find((g) => g.id === guideId)?.title);
  const { links, settings, loading, busy, error, revisionId } = useShare((s) => s);
  const store = shareStore.getState();

  const maxDays = settings?.shareLinkMaxDays ?? null;
  const choices = EXPIRY_DAYS.filter((d) => !maxDays || d <= maxDays);
  if (maxDays && !choices.includes(maxDays)) choices.push(maxDays);
  const [pinned, setPinned] = useState(false);
  const [days, setDays] = useState<number | null>(null);
  const [password, setPassword] = useState("");
  const [allowDownload, setAllowDownload] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  useEffect(() => {
    // The longest the workspace allows, or never.
    setDays(maxDays);
  }, [maxDays, guideId]);
  useEffect(() => {
    setPinned(false);
    setPassword("");
    setAllowDownload(false);
    setCopied(null);
  }, [guideId]);

  const off = settings?.shareLinks === "off";
  const needsPassword = settings?.shareLinks === "password";

  async function copy(url: string) {
    await navigator.clipboard.writeText(url).catch(() => {});
    setCopied(url);
  }

  async function create(e: FormEvent) {
    e.preventDefault();
    const link = await store.create({
      revisionId: pinned ? revisionId : null,
      expiresAt: days ? new Date(Date.now() + days * DAY).toISOString() : null,
      password: password || null,
      allowDownload,
    });
    if (!link) return;
    setPassword("");
    await copy(link.url);
  }

  return (
    <Dialog.Root open={guideId !== null} onOpenChange={(o) => !o && store.close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/30" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex max-h-[80vh] w-[560px] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-xl border border-border bg-background p-5 shadow-xl">
          <div className="flex items-start justify-between gap-4">
            <div>
              <Dialog.Title className="text-base font-semibold">Share “{title ?? "guide"}”</Dialog.Title>
              <Dialog.Description className="mt-1 text-xs text-muted-foreground">
                Anyone with a link can read the guide on the web, without an account. Links show the version on the web app.
              </Dialog.Description>
            </div>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close">
                <X />
              </Button>
            </Dialog.Close>
          </div>

          {loading ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : !settings ? null : off ? (
            <p className="text-sm text-muted-foreground">An admin turned share links off for this workspace.</p>
          ) : (
            <form className="flex flex-col gap-3" onSubmit={(e) => void create(e)}>
              <div className="flex gap-4 text-sm">
                <label className="flex items-center gap-2">
                  <input type="radio" checked={!pinned} onChange={() => setPinned(false)} /> Always the latest version
                </label>
                <label className="flex items-center gap-2" title={revisionId ? undefined : "Sync the guide first"}>
                  <input type="radio" checked={pinned} disabled={!revisionId} onChange={() => setPinned(true)} /> This version only
                </label>
              </div>
              <div className="grid grid-cols-2 gap-3">
                <label className="flex flex-col gap-1 text-xs text-muted-foreground">
                  Expires
                  <select className={SELECT} value={days ?? ""} onChange={(e) => setDays(e.target.value ? Number(e.target.value) : null)}>
                    {maxDays ? null : <option value="">Never</option>}
                    {choices.map((d) => (
                      <option key={d} value={d}>
                        In {d} {d === 1 ? "day" : "days"}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="flex flex-col gap-1 text-xs text-muted-foreground">
                  Password{needsPassword ? "" : " (optional)"}
                  <Input
                    type="password"
                    autoComplete="new-password"
                    minLength={4}
                    required={needsPassword}
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                </label>
              </div>
              <label className="flex items-center gap-2 text-sm">
                <input type="checkbox" checked={allowDownload} onChange={(e) => setAllowDownload(e.target.checked)} />
                Allow downloading
              </label>
              <Button type="submit" className="self-start" disabled={busy}>
                {busy ? "Creating…" : "Create and Copy Link"}
              </Button>
            </form>
          )}
          {error && <p className="text-sm text-[var(--status-danger)]">{error}</p>}

          {links.length > 0 && (
            <ul className="flex flex-col divide-y divide-border overflow-y-auto border-t border-border text-sm">
              {links.map((link) => (
                <li key={link.id} className="flex flex-col gap-1.5 py-2.5">
                  <div className="flex gap-2">
                    <Input readOnly value={link.url} aria-label="Link" onFocus={(e) => e.target.select()} className={link.state === "active" ? "" : "line-through opacity-60"} />
                    <Button variant="outline" onClick={() => void copy(link.url)}>
                      <Copy /> {copied === link.url ? "Copied" : "Copy"}
                    </Button>
                    <Button variant="outline" disabled={busy} onClick={() => void store.revoke(link.id)}>
                      Revoke
                    </Button>
                  </div>
                  <span className="text-xs text-muted-foreground">{describe(link)}</span>
                </li>
              ))}
            </ul>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
