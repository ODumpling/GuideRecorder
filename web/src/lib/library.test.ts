import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { eq, sql } from "drizzle-orm";
import { beforeEach, describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { asset, guide, organization, subscription, user } from "@/db/schema";
import { addMonths, library, reviewDue, reviewState, setFolder } from "./library";
import { getGuide, push, trashGuide, type Actor } from "./sync";

const fixture = (name: string): Guide =>
  JSON.parse(readFileSync(new URL(`../../../fixtures/guides/${name}.guiderecorder/guide.json`, import.meta.url), "utf8"));
const apiKey = fixture("Rotate an API key");
const floorPlan = fixture("Office floor plan");
const printer = fixture("Kurzanleitung Drucker");
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const assetsFor = (guide: Guide) => Object.fromEntries(imagePaths(guide).map((p) => [p, hash(p)]));

let db: Database;
const ada: Actor = { userId: "ada", role: "owner" };
const vic: Actor = { userId: "vic", role: "viewer" };
const today = new Date("2026-10-19T12:00:00Z");

async function pushed(actor: Actor, guide: Guide) {
  const result = await push(db, "org", actor, pushRequest.parse({ guideId: guide.id, baseRevision: null, guide, assets: assetsFor(guide) }), "app");
  if (result.status !== 200) throw new Error(`push failed: ${JSON.stringify(result)}`);
  return result.revision;
}

const titles = async (reader: Actor, filter: Parameters<typeof library>[3] = {}) =>
  (await library(db, "org", reader, filter, today)).guides.map((g) => g.title);

beforeEach(async () => {
  db = await testDb();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: new Date() });
  await db.insert(user).values(["ada", "vic"].map((id) => ({ id, name: id === "ada" ? "Ada Lovelace" : "Vic", email: `${id}@example.com` })));
  const paths = [...new Set([apiKey, floorPlan, printer].flatMap(imagePaths))];
  await db.insert(asset).values(paths.map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp" })));
  await pushed(ada, apiKey);
  await pushed(ada, floorPlan);
  await pushed(ada, { ...printer, status: "needsUpdate", owner: "IT", tags: ["Drucker"], reviewIntervalMonths: 6, lastReviewed: "2026-03-01" });
});

describe("library", () => {
  it("lists the workspace's guides, newest change first, with what the list shows", async () => {
    const { guides, tags, counts } = await library(db, "org", vic, {}, today);
    expect(guides.map((g) => g.title)).toEqual(["Kurzanleitung Drucker", "Office floor plan", "Rotate an API key"]);
    const firstStep = apiKey.items?.find((i) => i.type === "step");
    expect(guides[2]!.cover).toBe(hash(firstStep!.type === "step" ? firstStep!.image.full : ""));
    expect(counts).toEqual({ all: 3, reviewDue: 1, unfiled: 3, statuses: { draft: 0, inReview: 0, published: 0, needsUpdate: 1 } });
    expect(guides[2]).toMatchObject({ locked: false, steps: 5, tags: ["security", "api", "console"], owner: "Ada Lovelace", status: null, review: null, private: false });
    expect(guides[0]).toMatchObject({ owner: "IT", status: "needsUpdate", reviewDue: "2026-09-01", review: "overdue" });
    expect(tags.map((t) => t.name)).toEqual(["api", "console", "Drucker", "office", "security"]);
  });

  it("finds guides by title, step text, notes, hotspots and tags, by word prefix", async () => {
    expect(await titles(vic, { query: "rotat" })).toEqual(["Rotate an API key"]);
    expect(await titles(vic, { query: "production" })).toEqual(["Rotate an API key"]);
    expect(await titles(vic, { query: "revokes" })).toEqual(["Rotate an API key"]);
    expect(await titles(vic, { query: "kitchen" })).toEqual(["Office floor plan"]);
    expect(await titles(vic, { query: "papierfach" })).toEqual(["Kurzanleitung Drucker"]);
    expect(await titles(vic, { query: "security console" })).toEqual(["Rotate an API key"]);
    expect(await titles(vic, { query: "reception" })).toEqual(["Office floor plan"]);
    expect(await titles(vic, { query: "nothing like this" })).toEqual([]);
    // Only words count; tsquery syntax is ignored, not an error.
    expect(await titles(vic, { query: "!&|:*" })).toEqual([]);
  });

  it("ranks title matches above matches in the text", async () => {
    await pushed(ada, { ...structuredClone(printer), id: "01M4TZVZ000000000000000399", title: "Reception desk", tags: [], items: [] });
    expect(await titles(vic, { query: "reception" })).toEqual(["Reception desk", "Office floor plan"]);
  });

  it("filters by folder, tag, status and review", async () => {
    expect(await setFolder(db, "org", ada, floorPlan.id, "  Office   life ")).toEqual({ status: 200, folder: "Office life" });
    expect(await titles(vic, { folder: "Office life" })).toEqual(["Office floor plan"]);
    expect(await titles(vic, { folder: "" })).toEqual(["Kurzanleitung Drucker", "Rotate an API key"]);
    expect(await titles(vic, { tag: "API" })).toEqual(["Rotate an API key"]);
    expect(await titles(vic, { status: "needsUpdate" })).toEqual(["Kurzanleitung Drucker"]);
    expect(await titles(vic, { reviewDue: true })).toEqual(["Kurzanleitung Drucker"]);
    expect(await titles(vic, { folder: "Office life", query: "kitchen" })).toEqual(["Office floor plan"]);
    expect((await library(db, "org", vic)).folders).toEqual([{ name: "Office life", count: 1 }]);
  });

  it("only shows private guides to their owner, and never trashed ones", async () => {
    await db.update(guide).set({ visibility: "private" }).where(eq(guide.id, floorPlan.id));
    expect(await titles(vic)).not.toContain("Office floor plan");
    expect(await titles(vic, { query: "kitchen" })).toEqual([]);
    expect((await library(db, "org", ada)).guides.find((g) => g.title === "Office floor plan")?.private).toBe(true);
    await trashGuide(db, "org", ada, apiKey.id);
    expect(await titles(ada, { query: "rotate" })).toEqual([]);
  });

  it("shows viewers the published revision when a guide is locked", async () => {
    await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: "org", status: "active", seats: 10 });
    await db.update(guide).set({ locked: true }).where(eq(guide.id, printer.id));
    const head = (await getGuide(db, "org", ada, printer.id))!.head;
    await push(db, "org", ada, pushRequest.parse({ guideId: printer.id, baseRevision: head.id, guide: { ...head.guide, title: "Entwurf" }, assets: head.assets }), "web");
    expect(await titles(ada)).toContain("Entwurf");
    expect(await titles(vic)).toContain("Kurzanleitung Drucker");
    expect(await titles(vic)).not.toContain("Entwurf");
    expect((await library(db, "org", ada)).guides.find((g) => g.id === printer.id)).toMatchObject({ locked: true, owners: [] });
  });

  it("keeps the search document current with the head revision", async () => {
    const head = (await getGuide(db, "org", ada, printer.id))!.head;
    const next = { ...head.guide, title: "Toner wechseln" };
    await push(db, "org", ada, pushRequest.parse({ guideId: printer.id, baseRevision: head.id, guide: next, assets: head.assets }), "web");
    expect(await titles(vic, { query: "toner" })).toEqual(["Toner wechseln"]);
    expect(await titles(vic, { query: "kurzanleitung" })).toEqual([]);
  });
});

describe("migration 0002", () => {
  it("backfills search documents the way push writes them", async () => {
    const documents = () => db.select({ id: guide.id, search: sql<string>`${guide.search}::text` }).from(guide).orderBy(guide.id);
    const written = await documents();
    await db.update(guide).set({ search: null });
    const migration = readFileSync(new URL("../../drizzle/0002_library.sql", import.meta.url), "utf8");
    await db.execute(sql.raw(migration.split("--> statement-breakpoint").at(-1)!));
    expect(await documents()).toEqual(written);
  });
});

describe("setFolder", () => {
  it("needs a role that may change guides", async () => {
    expect(await setFolder(db, "org", vic, apiKey.id, "Security")).toMatchObject({ status: 403 });
    expect(await setFolder(db, "org", ada, "01M4WZCE2000000000000009ZZ", "Security")).toMatchObject({ status: 404 });
    expect(await setFolder(db, "org", ada, apiKey.id, "x".repeat(101))).toMatchObject({ status: 400 });
    expect(await setFolder(db, "org", ada, apiKey.id, "  ")).toEqual({ status: 200, folder: null });
  });
});

describe("review dates", () => {
  it("adds months like chrono, clamping to the month's end", () => {
    expect(addMonths("2026-01-31", 1)).toBe("2026-02-28");
    expect(addMonths("2024-01-31", 1)).toBe("2024-02-29");
    expect(addMonths("2026-11-15", 3)).toBe("2027-02-15");
  });

  it("are due an interval after the last review, or creation", () => {
    expect(reviewDue({ createdAt: "2026-01-10T09:00:00Z", reviewIntervalMonths: 12 })).toBe("2027-01-10");
    expect(reviewDue({ createdAt: "2026-01-10T09:00:00Z", lastReviewed: "2026-05-02", reviewIntervalMonths: 1 })).toBe("2026-06-02");
    expect(reviewDue({ createdAt: "2026-01-10T09:00:00Z", reviewIntervalMonths: 0 })).toBeNull();
    expect(reviewDue({ createdAt: "2026-01-10T09:00:00Z" })).toBeNull();
  });

  it("are overdue after the due day and due soon within 30 days", () => {
    expect(reviewState("2026-10-19", today)).toBe("soon");
    expect(reviewState("2026-10-18", today)).toBe("overdue");
    expect(reviewState("2026-11-18", today)).toBe("soon");
    expect(reviewState("2026-12-01", today)).toBeNull();
    expect(reviewState(null, today)).toBeNull();
  });
});
