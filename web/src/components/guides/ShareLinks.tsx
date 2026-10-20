import { useRouter } from "@tanstack/react-router";
import { useRef, useState, type FormEvent } from "react";
import { createShare, revokeShare, type getShareLinks } from "@/server/shares";
import { Button, ErrorText, Input, Label } from "../ui";

// Share links for a guide: make one (latest or this version, expiry,
// password, download), copy it, see its views, revoke it.

type Data = Awaited<ReturnType<typeof getShareLinks>>;
type Link = Extract<Data, { available: true }>["links"][number];

const DAY = 24 * 60 * 60 * 1000;
const EXPIRY_DAYS = [1, 7, 30, 90, 365];
const date = (iso: string) => new Date(iso).toLocaleDateString(undefined, { dateStyle: "medium" });

interface Props {
  guideId: string;
  /** The revision on screen, to pin a link to. */
  revisionId: string;
  shares: Data;
}

export function ShareLinks({ guideId, revisionId, shares }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  // Members whose role can't share don't see the button.
  if (!shares.available && !shares.upgrade) return null;

  return (
    <>
      <Button variant="secondary" onClick={() => dialog.current?.showModal()}>
        Share…
      </Button>
      <dialog
        ref={dialog}
        className="m-auto w-full max-w-xl rounded-xl border border-border bg-card p-6 text-card-foreground shadow-xl backdrop:bg-black/30"
        onClick={(e) => e.target === dialog.current && dialog.current.close()}
      >
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-lg font-semibold">Share with a link</h2>
          <Button variant="secondary" className="h-8 px-3" onClick={() => dialog.current?.close()}>
            Close
          </Button>
        </div>
        {shares.available ? (
          <SharePanel guideId={guideId} revisionId={revisionId} links={shares.links} settings={shares.settings} />
        ) : (
          <p className="text-sm text-muted-foreground">{shares.reason}</p>
        )}
      </dialog>
    </>
  );
}

type PanelProps = Omit<Props, "shares"> & Pick<Extract<Data, { available: true }>, "links" | "settings">;

function SharePanel({ guideId, revisionId, links, settings }: PanelProps) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [pinned, setPinned] = useState(false);
  const maxDays = settings.shareLinkMaxDays;
  const choices = EXPIRY_DAYS.filter((d) => !maxDays || d <= maxDays);
  if (maxDays && !choices.includes(maxDays)) choices.push(maxDays);
  // The longest the workspace allows, or never.
  const [days, setDays] = useState<number | null>(maxDays);
  const [password, setPassword] = useState("");
  const [allowDownload, setAllowDownload] = useState(false);
  const off = settings.shareLinks === "off";
  const needsPassword = settings.shareLinks === "password";

  async function create(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const result = await createShare({
      data: {
        guideId,
        revisionId: pinned ? revisionId : null,
        expiresAt: days ? new Date(Date.now() + days * DAY).toISOString() : null,
        password: password || null,
        allowDownload,
      },
    });
    setBusy(false);
    if (!result.ok) return setError(result.error);
    setPassword("");
    await navigator.clipboard?.writeText(result.link.url).catch(() => {});
    await router.invalidate();
  }

  async function revoke(link: Link) {
    setError(null);
    const result = await revokeShare({ data: { linkId: link.id } });
    if (!result.ok) setError(result.error);
    else await router.invalidate();
  }

  return (
    <div className="grid gap-5">
      {off ? (
        <p className="text-sm text-muted-foreground">An admin turned share links off for this workspace.</p>
      ) : (
        <form className="grid gap-3" onSubmit={create}>
          <fieldset className="flex flex-wrap gap-4 text-sm">
            <label className="flex items-center gap-2">
              <input type="radio" checked={!pinned} onChange={() => setPinned(false)} /> Always the latest version
            </label>
            <label className="flex items-center gap-2">
              <input type="radio" checked={pinned} onChange={() => setPinned(true)} /> This version only
            </label>
          </fieldset>
          <div className="grid grid-cols-2 gap-3">
            <Label>
              Expires
              <select
                className="h-9 rounded-lg border border-input bg-card px-3 text-sm"
                value={days ?? ""}
                onChange={(e) => setDays(e.target.value ? Number(e.target.value) : null)}
              >
                {maxDays ? null : <option value="">Never</option>}
                {choices.map((d) => (
                  <option key={d} value={d}>
                    In {d} {d === 1 ? "day" : "days"}
                  </option>
                ))}
              </select>
            </Label>
            <Label>
              Password{needsPassword ? "" : " (optional)"}
              <Input type="password" autoComplete="new-password" minLength={4} required={needsPassword} value={password} onChange={(e) => setPassword(e.target.value)} />
            </Label>
          </div>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={allowDownload} onChange={(e) => setAllowDownload(e.target.checked)} /> Allow downloading
          </label>
          <Button type="submit" disabled={busy} className="justify-self-start">
            {busy ? "Creating…" : "Create link"}
          </Button>
        </form>
      )}
      <ErrorText>{error}</ErrorText>
      {links.length ? (
        <ul className="divide-y divide-border border-t border-border text-sm">
          {links.map((link) => (
            <li key={link.id} className="grid gap-2 py-3">
              <div className="flex gap-2">
                <Input readOnly value={link.url} onFocus={(e) => e.target.select()} aria-label="Link" className={link.state === "active" ? "" : "line-through opacity-60"} />
                <Button variant="secondary" onClick={() => void navigator.clipboard?.writeText(link.url)}>
                  Copy
                </Button>
                <Button variant="danger" onClick={() => revoke(link)}>
                  Revoke
                </Button>
              </div>
              <p className="text-xs text-muted-foreground">{describe(link)}</p>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function describe(link: Link) {
  const parts = [
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
    link.createdBy ? `by ${link.createdBy}` : null,
  ];
  return parts.filter(Boolean).join(" · ");
}
