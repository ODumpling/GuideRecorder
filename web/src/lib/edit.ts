import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, mergeGuides, type AssetMap, type Conflict, type Revision, type Side } from "@guiderecorder/sync";
import type { Database } from "@/db";
import { checkAccess } from "./access";
import { textOnlyProblem } from "./draft";
import { getRevision, push, type Actor, type PushResult } from "./sync";

// Saving an edit made in the web editor. It goes through `push` like a sync
// from the desktop app, so it becomes a revision like any other and the app
// pulls and merges it as usual. If the guide changed since the edit began,
// the edit is merged into the head here the way the app merges
// (`mergeGuides`, based on the revision the edit began on) and pushed again.

/** Who may use the web editor: `guide:update` on a plan with `webEditor`. */
export function editorAccess(db: Database, userId: string | undefined, organizationId: string) {
  return checkAccess(db, userId, organizationId, { guide: ["update"] }, "webEditor");
}

export interface EditRequest {
  guideId: string;
  /** The revision the edit began on. */
  baseRevision: string;
  guide: Guide;
  /**
   * How to settle edits that clash with changes made meanwhile: keep this
   * edit (`ours`) or the other (`theirs`). Unset: clashes are returned.
   */
  prefer?: Side;
}

export type EditResult =
  | { status: 200; revision: Revision; merged: boolean }
  | { status: 409; error: string; conflicts: string[]; head: Revision }
  | Exclude<PushResult, { status: 200 } | { status: 409 }>;

/** Pushes after this many merges give up: the guide is changing too fast. */
const ATTEMPTS = 3;

export async function saveEdit(db: Database, organizationId: string, actor: Actor, request: EditRequest): Promise<EditResult> {
  const base = await getRevision(db, organizationId, actor.userId, request.guideId, request.baseRevision);
  if (!base) return { status: 404, error: "Guide not found." };
  const problem = textOnlyProblem(base.guide, request.guide);
  if (problem) return { status: 400, error: problem };

  let guide = request.guide;
  let onto = base;
  let assets = base.assets;
  for (let attempt = 0; attempt < ATTEMPTS; attempt += 1) {
    const result = await push(
      db,
      organizationId,
      actor,
      { guideId: request.guideId, baseRevision: onto.id, guide: guide as unknown as Record<string, unknown>, assets: pick(assets, guide) },
      "web",
    );
    if (result.status === 200) return { status: 200, revision: result.revision, merged: onto !== base };
    if (result.status !== 409) return result;
    const head = result.head;
    const merged = mergeGuides(base.guide, request.guide, head.guide, request.prefer ?? "ours");
    if (merged.conflicts.length && !request.prefer) {
      return {
        status: 409,
        error: "Someone changed the same text since you started editing.",
        conflicts: merged.conflicts.map((c) => describeConflict(c, base.guide)),
        head,
      };
    }
    guide = merged.guide;
    onto = head;
    // The head's images win: a step's image may have changed in the app.
    assets = { ...base.assets, ...head.assets };
  }
  return { status: 400, error: "The guide keeps changing. Try saving again." };
}

/** The assets of the guide's image paths. */
function pick(assets: AssetMap, guide: Guide): AssetMap {
  return Object.fromEntries(imagePaths(guide).flatMap((path) => (assets[path] ? [[path, assets[path]]] : [])));
}

const FIELD_NAMES: Record<string, string> = { intro: "introduction", body: "text" };

/** A conflict in words, e.g. "Step 2 title", naming items as in `guide`. */
export function describeConflict(conflict: Conflict, guide: Guide): string {
  if (conflict.kind === "order") return "The order of the steps";
  const name = itemName(guide, conflict.itemId);
  if (conflict.kind === "deleted") return `${name} (deleted ${conflict.deletedBy === "ours" ? "here" : "elsewhere"})`;
  const field = FIELD_NAMES[conflict.field] ?? conflict.field;
  return conflict.itemId ? `${name} ${field}` : field[0]!.toUpperCase() + field.slice(1);
}

function itemName(guide: Guide, id: string | null): string {
  let step = 0;
  for (const item of guide.items) {
    if (item.type === "step") step += 1;
    if (item.id !== id) continue;
    return item.type === "step" ? `Step ${step}` : item.type === "section" ? "Section" : "Note";
  }
  return "An item";
}
