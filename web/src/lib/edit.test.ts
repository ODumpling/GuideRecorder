import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it } from "vitest";
import type { Guide, Step } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest, type Revision } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { asset, member, organization, subscription, user } from "@/db/schema";
import { deleteItem, setGuideText, setItemText } from "./draft";
import { editorAccess, saveEdit } from "./edit";
import { getGuide, push, trashGuide, type Actor } from "./sync";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const assetsFor = (guide: Guide) => Object.fromEntries(imagePaths(guide).map((p) => [p, hash(p)]));
const steps = (guide: Guide) => guide.items.filter((i): i is Step => i.type === "step");
const [first, second] = steps(fixture);

let db: Database;
const ada: Actor = { userId: "ada", role: "owner" };
const grace: Actor = { userId: "grace", role: "editor" };

beforeEach(async () => {
  db = await testDb();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: new Date() });
  await db.insert(user).values(["ada", "grace", "vic"].map((id) => ({ id, name: id, email: `${id}@example.com` })));
  await db
    .insert(asset)
    .values(imagePaths(fixture).map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp" })));
});

/** A push from the desktop app. */
async function fromApp(guide: Guide, base: string | null): Promise<Revision> {
  const result = await push(db, "org", ada, pushRequest.parse({ guideId: guide.id, baseRevision: base, guide, assets: assetsFor(guide) }), "app");
  if (result.status !== 200) throw new Error(`push failed: ${JSON.stringify(result)}`);
  return result.revision;
}

describe("saveEdit", () => {
  it("saves a text edit as a web revision on the head", async () => {
    const synced = await fromApp(fixture, null);
    const edited = setItemText(setGuideText(fixture, "intro", "Rotate it yearly."), first!.id, "title", "Open the API page");
    const result = await saveEdit(db, "org", grace, { guideId: fixture.id, baseRevision: synced.id, guide: edited });
    expect(result).toMatchObject({ status: 200, merged: false, revision: { parentId: synced.id, source: "web", authorId: "grace" } });
    const head = (await getGuide(db, "org", ada, fixture.id))!.head;
    expect(head.guide.intro).toBe("Rotate it yearly.");
    expect(steps(head.guide)[0]).toMatchObject({ title: "Open the API page", aiState: { status: "edited" } });
    expect(head.assets).toEqual(synced.assets);
  });

  it("drops a deleted step's images from the revision", async () => {
    const synced = await fromApp(fixture, null);
    const edited = deleteItem(fixture, first!.id);
    const result = await saveEdit(db, "org", grace, { guideId: fixture.id, baseRevision: synced.id, guide: edited });
    expect(result.status).toBe(200);
    if (result.status !== 200) return;
    expect(Object.keys(result.revision.assets).sort()).toEqual(imagePaths(edited).sort());
  });

  it("refuses edits beyond text", async () => {
    const synced = await fromApp(fixture, null);
    const recropped = { ...fixture, items: fixture.items.map((i) => (i.id === first!.id ? { ...first!, image: { ...first!.image, show: "full" as const } } : i)) };
    expect(await saveEdit(db, "org", grace, { guideId: fixture.id, baseRevision: synced.id, guide: recropped })).toMatchObject({ status: 400 });
  });

  it("merges with changes the app made meanwhile", async () => {
    const synced = await fromApp(fixture, null);
    const fromDesktop = await fromApp(setItemText(fixture, second!.id, "description", "From the app"), synced.id);
    const edited = setItemText(fixture, first!.id, "title", "From the web");
    const result = await saveEdit(db, "org", grace, { guideId: fixture.id, baseRevision: synced.id, guide: edited });
    expect(result).toMatchObject({ status: 200, merged: true, revision: { parentId: fromDesktop.id } });
    const head = (await getGuide(db, "org", ada, fixture.id))!.head;
    expect(steps(head.guide).map((s) => [s.title, s.description]).slice(0, 2)).toEqual([
      ["From the web", first!.description],
      [second!.title, "From the app"],
    ]);
  });

  it("returns clashes, then settles them as asked", async () => {
    const synced = await fromApp(fixture, null);
    await fromApp(setItemText(fixture, first!.id, "title", "App title"), synced.id);
    const edited = setItemText(fixture, first!.id, "title", "Web title");
    const request = { guideId: fixture.id, baseRevision: synced.id, guide: edited };

    expect(await saveEdit(db, "org", grace, request)).toMatchObject({ status: 409, conflicts: ["Step 1 title"] });
    expect(await saveEdit(db, "org", grace, { ...request, prefer: "theirs" })).toMatchObject({ status: 200 });
    expect(steps((await getGuide(db, "org", ada, fixture.id))!.head.guide)[0]!.title).toBe("App title");

    // Settled on the head now: a fresh clash, kept as this edit's.
    expect(await saveEdit(db, "org", grace, { ...request, prefer: "ours" })).toMatchObject({ status: 200, merged: true });
    expect(steps((await getGuide(db, "org", ada, fixture.id))!.head.guide)[0]!.title).toBe("Web title");
  });

  it("checks the role, the base revision and the trash", async () => {
    const synced = await fromApp(fixture, null);
    const edited = setGuideText(fixture, "title", "Viewer's");
    expect(await saveEdit(db, "org", { userId: "vic", role: "viewer" }, { guideId: fixture.id, baseRevision: synced.id, guide: edited })).toMatchObject({
      status: 403,
    });
    expect(await saveEdit(db, "org", grace, { guideId: fixture.id, baseRevision: "nope", guide: edited })).toMatchObject({ status: 404 });
    await trashGuide(db, "org", ada, fixture.id);
    expect(await saveEdit(db, "org", grace, { guideId: fixture.id, baseRevision: synced.id, guide: edited })).toMatchObject({ status: 410 });
  });
});

describe("editorAccess", () => {
  beforeEach(async () => {
    const now = new Date();
    for (const [id, role] of [
      ["ada", "owner"],
      ["grace", "editor"],
      ["vic", "viewer"],
    ] as const) {
      await db.insert(member).values({ id: `m-${id}`, organizationId: "org", userId: id, role, createdAt: now });
    }
  });

  it("needs guide:update and a plan with the web editor", async () => {
    expect(await editorAccess(db, "grace", "org")).toMatchObject({ ok: false, status: 402 });
    await db.insert(subscription).values({ id: "s", plan: "pro", referenceId: "org", status: "active" });
    expect(await editorAccess(db, "grace", "org")).toMatchObject({ ok: true, role: "editor" });
    expect(await editorAccess(db, "vic", "org")).toMatchObject({ ok: false, status: 403 });
  });
});
