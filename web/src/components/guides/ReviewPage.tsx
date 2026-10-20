import { Link, useRouter } from "@tanstack/react-router";
import { useState } from "react";
import type { GuideStatus, Item } from "@guiderecorder/guide-schema";
import type { Revision } from "@guiderecorder/sync";
import { AnnotatedImage } from "@guiderecorder/ui/annotated-image";
import type { ItemDiff } from "@/lib/diff";
import { reviewGuideChange, submitGuideForReview, withdrawGuideChange, type getGuideReview } from "@/server/reviews";
import { Textarea } from "@guiderecorder/ui/textarea";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@guiderecorder/ui/table";
import { ErrorText } from "@/components/ui";

// The review page of a gated guide: what its change request changes against
// the published revision, item by item (matched by ULID) with the old and
// new screenshots side by side, the reviews so far, and the reviewer's
// verdict. Comments can be on the whole change or one item.

type Data = Extract<Awaited<ReturnType<typeof getGuideReview>>, { allowed: true }>;
type Verdict = "approve" | "reject" | "requestChanges" | "comment";

export const STATUS_LABEL: Record<GuideStatus, string> = {
  draft: "Draft",
  inReview: "In review",
  published: "Published",
  needsUpdate: "Changes requested",
};

const VERDICT_LABEL: Record<Verdict, string> = {
  approve: "approved",
  reject: "rejected",
  requestChanges: "requested changes",
  comment: "commented",
};

const CHANGE_LABEL: Record<ItemDiff["change"], string> = {
  added: "Added",
  removed: "Removed",
  changed: "Changed",
  moved: "Moved",
  unchanged: "Unchanged",
};

const date = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

export function StatusBadge({ status }: { status: GuideStatus }) {
  const tone =
    status === "published"
      ? "bg-success/10 text-success"
      : status === "needsUpdate"
        ? "bg-destructive/10 text-destructive"
        : "bg-muted text-muted-foreground";
  return <span className={`rounded-full px-2 py-0.5 text-xs font-medium ${tone}`}>{STATUS_LABEL[status]}</span>;
}

export function ReviewPage(data: Data) {
  const router = useRouter();
  const { workspaceId, guideId, state, policy, changeRequest: cr, reviews, evaluation, before, after, diff, can } = data;
  const open = cr?.status === "open";
  const [body, setBody] = useState("");
  const [itemId, setItemId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showUnchanged, setShowUnchanged] = useState(false);

  async function act(action: () => Promise<{ ok: boolean; error?: string }>) {
    setBusy(true);
    setError(null);
    const result = await action();
    setBusy(false);
    if (!result.ok) return setError(result.error ?? "Something went wrong.");
    setBody("");
    setItemId(null);
    await router.invalidate();
  }

  const verdict = (v: Verdict) =>
    act(() => reviewGuideChange({ data: { guideId, changeRequestId: cr!.id, verdict: v, revisionId: cr!.revisionId, body, itemId: v === "comment" ? itemId : null } }));

  // Removed items keep the names they had.
  const names = new Map([...itemNames(before?.guide.items ?? []), ...itemNames(after.guide.items)]);
  const visible = diff.items.filter((i) => showUnchanged || i.change !== "unchanged");
  const unchanged = diff.items.length - diff.items.filter((i) => i.change !== "unchanged").length;
  const general = reviews.filter((r) => !r.itemId);

  return (
    <div className="mx-auto grid w-full max-w-5xl gap-7 px-5 py-8 md:px-9">
      <header className="flex items-center justify-between gap-4">
        <Link to="/guides/$guideId" params={{ guideId }} className="text-sm text-muted-foreground hover:underline">
          ← {after.guide.title}
        </Link>
        <StatusBadge status={state.status} />
      </header>

      <Card className="p-6 grid gap-3">
        <h1 className="font-serif text-3xl">Review changes</h1>
        {!data.enabled ? (
          <p className="text-sm text-muted-foreground">Your plan doesn&rsquo;t include approvals.</p>
        ) : !data.gated ? (
          <p className="text-sm text-muted-foreground">This guide is unlocked: edits publish straight away.</p>
        ) : (
          <p className="text-sm text-muted-foreground">
            This guide is locked. Publishing needs {policy.requiredApprovals} {policy.requiredApprovals === 1 ? "approval" : "approvals"}
            {data.approvers.length ? ` from its owners (${data.approvers.map((a) => a.name).join(", ")})` : ""}
            {policy.allowSelfApproval ? "" : "; authors can't approve their own changes"}
            {policy.resetOnNewRevision ? "; new edits reset approvals" : ""}.
          </p>
        )}
        {cr ? (
          <p className="text-sm">
            {cr.author?.name ?? "Someone"} submitted this {date(cr.createdAt)}
            {cr.note ? <>: &ldquo;{cr.note}&rdquo;</> : "."}{" "}
            {open ? (
              evaluation ? (
                <strong>
                  {evaluation.approvedBy.length} of {evaluation.required} approvals
                  {evaluation.changesRequestedBy.length ? `, changes requested` : ""}.
                </strong>
              ) : null
            ) : (
              <strong>
                {cr.status === "approved" ? "Approved and published" : cr.status === "rejected" ? "Rejected" : "Withdrawn"}
                {cr.closedBy ? ` by ${cr.closedBy.name}` : ""}
                {cr.closedAt ? `, ${date(cr.closedAt)}` : ""}.
              </strong>
            )}
          </p>
        ) : null}
        {can.submit ? <Submit guideId={guideId} revisionId={after.id} onDone={() => router.invalidate()} /> : null}
        {open && can.withdraw ? (
          <div>
            <Button variant="outline" size="lg" disabled={busy} onClick={() => act(() => withdrawGuideChange({ data: { guideId, changeRequestId: cr!.id } }))}>
              Withdraw
            </Button>
          </div>
        ) : null}
      </Card>

      {diff.fields.length ? (
        <Card className="p-6 grid gap-2">
          <h2 className="text-lg font-semibold">Guide details</h2>
          <Table className="table-fixed">
            <TableHeader className="[&_th]:text-muted-foreground">
              <TableRow>
                <TableHead className="w-40 py-1">Field</TableHead>
                <TableHead className="py-1">Published</TableHead>
                <TableHead className="py-1">This change</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {diff.fields.map((f) => (
                <TableRow key={f.field} className="align-top">
                  <TableCell className="py-2 font-medium">{f.field}</TableCell>
                  <TableCell className="py-2 whitespace-pre-wrap text-muted-foreground line-through decoration-destructive/40">{show(f.before)}</TableCell>
                  <TableCell className="py-2 whitespace-pre-wrap">{show(f.after)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </Card>
      ) : null}

      <section className="grid gap-4" aria-label="Changes by item">
        {!diff.changed ? <Card className="p-6 block text-sm text-muted-foreground">No changes against the published guide.</Card> : null}
        {visible.map((d) => (
          <ItemChange
            key={d.id}
            diff={d}
            name={names.get(d.id) ?? "Item"}
            workspaceId={workspaceId}
            before={before}
            after={after}
            comments={reviews.filter((r) => r.itemId === d.id)}
            onComment={can.comment && d.after ? () => setItemId(d.id) : undefined}
          />
        ))}
        {unchanged ? (
          <button className="justify-self-start text-sm text-muted-foreground hover:underline" onClick={() => setShowUnchanged((s) => !s)}>
            {showUnchanged ? "Hide" : "Show"} {unchanged} unchanged {unchanged === 1 ? "item" : "items"}
          </button>
        ) : null}
      </section>

      <Card className="p-6 grid gap-4">
        <h2 className="text-lg font-semibold">Reviews</h2>
        {general.length ? (
          <ul className="grid gap-3">
            {general.map((r) => (
              <ReviewEntry key={r.id} review={r} stale={r.revisionId !== cr?.revisionId} />
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted-foreground">No reviews yet.</p>
        )}
        {open && (can.comment || can.approve || can.reject) ? (
          <div className="grid gap-2">
            {itemId ? (
              <p className="text-sm">
                Commenting on <strong>{names.get(itemId)}</strong>{" "}
                <button className="text-muted-foreground hover:underline" onClick={() => setItemId(null)}>
                  (the whole change instead)
                </button>
              </p>
            ) : null}
            <Textarea
              aria-label="Review comment"
              className="min-h-24"
              placeholder={itemId ? "Your comment on this item" : "Leave a comment, or say what needs to change"}
              value={body}
              onChange={(e) => setBody(e.target.value)}
            />
            <div className="flex flex-wrap gap-2">
              {can.approve ? (
                <Button variant="brand" size="lg" disabled={busy || !!itemId} onClick={() => verdict("approve")}>
                  Approve
                </Button>
              ) : null}
              {can.reject ? (
                <>
                  <Button variant="outline" size="lg" disabled={busy || !!itemId || !body.trim()} onClick={() => verdict("requestChanges")}>
                    Request changes
                  </Button>
                  <Button variant="destructive" size="lg" disabled={busy || !!itemId} onClick={() => verdict("reject")}>
                    Reject
                  </Button>
                </>
              ) : null}
              <Button variant="outline" size="lg" disabled={busy || !body.trim()} onClick={() => verdict("comment")}>
                Comment
              </Button>
            </div>
            {!can.approve && data.approveProblem ? <p className="text-xs text-muted-foreground">{data.approveProblem}</p> : null}
          </div>
        ) : null}
        <ErrorText>{error}</ErrorText>
      </Card>
    </div>
  );
}

function Submit({ guideId, revisionId, onDone }: { guideId: string; revisionId: string; onDone: () => void }) {
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function submit() {
    setBusy(true);
    setError(null);
    const result = await submitGuideForReview({ data: { guideId, revisionId, note } });
    setBusy(false);
    if (!result.ok) return setError(result.error);
    onDone();
  }
  return (
    <div className="grid gap-2">
      <Textarea
        aria-label="Note for reviewers"
        placeholder="What changed, for the reviewers (optional)"
        value={note}
        onChange={(e) => setNote(e.target.value)}
      />
      <div>
        <Button variant="brand" size="lg" disabled={busy} onClick={submit}>
          Submit for review
        </Button>
      </div>
      <ErrorText>{error}</ErrorText>
    </div>
  );
}

type ReviewData = Data["reviews"][number];

function ReviewEntry({ review: r, stale }: { review: ReviewData; stale: boolean }) {
  return (
    <li className="grid gap-1 text-sm">
      <span>
        <strong>{r.reviewer?.name ?? "Someone"}</strong> {VERDICT_LABEL[r.verdict]}{" "}
        <span className="text-muted-foreground">
          {date(r.createdAt)}
          {stale ? " · on an earlier version" : ""}
        </span>
      </span>
      {r.body ? <p className="whitespace-pre-wrap">{r.body}</p> : null}
    </li>
  );
}

function ItemChange({
  diff: d,
  name,
  workspaceId,
  before,
  after,
  comments,
  onComment,
}: {
  diff: ItemDiff;
  name: string;
  workspaceId: string;
  before: Revision | null;
  after: Revision;
  comments: ReviewData[];
  onComment?: () => void;
}) {
  const src = (assets: Revision["assets"]) => (path: string) => (assets[path] ? `/api/workspaces/${workspaceId}/assets/${assets[path]}` : null);
  const changedImage = d.fields.includes("image") || d.fields.includes("annotations") || d.change === "added" || d.change === "removed";
  return (
    <Card className="p-6 grid gap-3" aria-label={name}>
      <div className="flex items-center gap-2">
        <span className="text-[11px] font-semibold tracking-[0.08em] text-brand uppercase">{name}</span>
        <span className="text-xs text-muted-foreground">
          {CHANGE_LABEL[d.change]}
          {d.fields.length ? `: ${d.fields.join(", ")}` : ""}
        </span>
        {onComment ? (
          <button className="ml-auto text-xs text-muted-foreground hover:underline" onClick={onComment}>
            Comment on this
          </button>
        ) : null}
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <Side label="Published" item={d.before} assets={before?.assets ?? {}} src={src} showImage={changedImage} faded />
        <Side label="This change" item={d.after} assets={after.assets} src={src} showImage={changedImage} />
      </div>
      {comments.length ? (
        <ul className="grid gap-2 border-t border-border pt-3">
          {comments.map((c) => (
            <ReviewEntry key={c.id} review={c} stale={c.revisionId !== after.id} />
          ))}
        </ul>
      ) : null}
    </Card>
  );
}

function Side({
  label,
  item,
  assets,
  src,
  showImage,
  faded,
}: {
  label: string;
  item: Item | null;
  assets: Revision["assets"];
  src: (assets: Revision["assets"]) => (path: string) => string | null;
  showImage: boolean;
  faded?: boolean;
}) {
  return (
    <div className={`grid content-start gap-2 ${faded ? "opacity-80" : ""}`}>
      <span className="text-xs text-muted-foreground">{label}</span>
      {!item ? (
        <p className="text-sm text-muted-foreground italic">Not there</p>
      ) : (
        <>
          {"title" in item && item.title ? <p className="font-medium whitespace-pre-wrap">{item.title}</p> : null}
          {item.type === "step" && item.description ? <p className="text-sm whitespace-pre-wrap">{item.description}</p> : null}
          {item.type === "note" ? <p className="text-sm whitespace-pre-wrap">{item.body}</p> : null}
          {item.type === "step" && showImage ? (
            <AnnotatedImage image={item.image} annotations={item.annotations} src={src(assets)} label={`${label} screenshot`} />
          ) : null}
        </>
      )}
    </div>
  );
}

/** "Step 3", "Section", "Note" by item id. */
function itemNames(items: Item[]): Map<string, string> {
  let step = 0;
  return new Map(
    items.map((item) => {
      if (item.type === "step") step += 1;
      return [item.id, item.type === "step" ? `Step ${step}` : item.type === "section" ? "Section" : "Note"];
    }),
  );
}

function show(value: unknown): string {
  if (value == null || value === "") return "—";
  if (Array.isArray(value)) return value.join(", ");
  if (typeof value === "object") return JSON.stringify(value);
  return String(value);
}
