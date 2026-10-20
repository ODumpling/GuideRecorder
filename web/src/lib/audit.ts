import { createHash } from "node:crypto";
import { and, asc, desc, eq, gt, gte, inArray, isNotNull, like, lt, lte, max, sql, type SQL } from "drizzle-orm";
import type { Database } from "@/db";
import { auditEvent, auditSettings, user } from "@/db/schema";
import { auditFilters, type AuditSearch } from "./audit-filters";

// The audit log (Enterprise). Every workspace has one chain of events: each
// event's hash covers its fields and the previous event's hash, so editing
// or deleting an event breaks the chain from there on. The table refuses
// updates and deletes (a trigger, see migration 0004), except retention
// pruning, which deletes the oldest events and records where the chain now
// starts in an `audit.pruned` event. Events are written whatever the plan;
// reading them needs `audit:read` and the `audit` entitlement.

export const AUDIT_ACTIONS = [
  "user.signIn",
  "member.add",
  "member.remove",
  "member.role",
  "invitation.create",
  "invitation.accept",
  "invitation.cancel",
  "billing.subscription",
  "licence.grant",
  "licence.revoke",
  "guide.create",
  "guide.edit",
  "guide.delete",
  "guide.restore",
  "guide.publish",
  "guide.export",
  "guide.lock",
  "guide.unlock",
  "guide.owner.add",
  "guide.owner.remove",
  "review.submit",
  "review.approve",
  "review.reject",
  "review.requestChanges",
  "review.comment",
  "review.withdraw",
  "policy.update",
  "policy.delete",
  "share.create",
  "share.update",
  "share.revoke",
  "audit.export",
  "audit.retention",
  "audit.pruned",
] as const;
export type AuditAction = (typeof AUDIT_ACTIONS)[number];

/** The first event's `prevHash`. */
export const GENESIS = "0".repeat(64);

export const MIN_RETENTION_DAYS = 30;
export const MAX_RETENTION_DAYS = 3650;
const DAY = 24 * 60 * 60 * 1000;

/** Where a request came from, for the log. */
export interface RequestMeta {
  ip?: string | null;
  userAgent?: string | null;
}

/** The client's address (first `X-Forwarded-For` hop, as set by the proxy) and user agent. */
export function requestMeta(headers: Headers | null | undefined): RequestMeta {
  if (!headers) return {};
  const forwarded = headers.get("x-forwarded-for")?.split(",")[0]?.trim();
  return { ip: forwarded || headers.get("x-real-ip") || null, userAgent: headers.get("user-agent") };
}

export interface AuditInput extends RequestMeta {
  actorId: string | null;
  action: AuditAction;
  /** e.g. `guide`, `member`, `changeRequest`. */
  targetType: string;
  targetId: string | null;
  /** The target's state before and after; stored as hashes (`stateHash`). */
  before?: unknown;
  after?: unknown;
  metadata?: Record<string, unknown>;
  at?: Date;
}

export type AuditEvent = typeof auditEvent.$inferSelect;

/** JSON with object keys sorted, so equal values serialize the same. */
export function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, v: unknown) =>
    v && typeof v === "object" && !Array.isArray(v)
      ? Object.fromEntries(Object.entries(v as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : v,
  );
}

const sha256 = (text: string) => createHash("sha256").update(text).digest("hex");

/** SHA-256 of a value's canonical JSON; null for undefined. */
export function stateHash(value: unknown): string | null {
  return value === undefined ? null : sha256(canonical(value));
}

/** The hash of an event: its fields and the previous event's hash. */
export function eventHash(e: Omit<AuditEvent, "hash">): string {
  return sha256(
    canonical([
      e.organizationId,
      e.seq,
      e.prevHash,
      e.actorId,
      e.action,
      e.targetType,
      e.targetId,
      e.beforeHash,
      e.afterHash,
      e.ip,
      e.userAgent,
      e.metadata,
      e.createdAt.toISOString(),
    ]),
  );
}

/**
 * Appends an event to the workspace's chain. Pass the transaction of the
 * change it records, so both land or neither does.
 */
export async function recordAudit(db: Database, organizationId: string, input: AuditInput): Promise<AuditEvent> {
  return db.transaction(async (tx) => {
    await tx.execute(sql`select pg_advisory_xact_lock(hashtext(${`audit:${organizationId}`}))`);
    const [last] = await tx
      .select({ seq: auditEvent.seq, hash: auditEvent.hash })
      .from(auditEvent)
      .where(eq(auditEvent.organizationId, organizationId))
      .orderBy(desc(auditEvent.seq))
      .limit(1);
    // Millisecond precision, so the time hashes the same once read back.
    const createdAt = new Date(Math.floor((input.at ?? new Date()).getTime()));
    const fields = {
      organizationId,
      seq: (last?.seq ?? 0) + 1,
      prevHash: last?.hash ?? GENESIS,
      actorId: input.actorId,
      action: input.action,
      targetType: input.targetType,
      targetId: input.targetId,
      beforeHash: stateHash(input.before),
      afterHash: stateHash(input.after),
      ip: input.ip ?? null,
      userAgent: input.userAgent ?? null,
      metadata: input.metadata ?? {},
      createdAt,
    };
    const event = { ...fields, hash: eventHash(fields) };
    await tx.insert(auditEvent).values(event);
    return event;
  });
}

export type ChainCheck =
  | { ok: true; count: number; firstSeq: number | null }
  | { ok: false; count: number; seq: number; reason: string };

/**
 * Checks a run of a workspace's events, oldest first. The run starts at the
 * chain's start, or right after what was pruned (`pruned`: the latest
 * `audit.pruned` event's record of it).
 */
export function verifyChain(events: AuditEvent[], pruned?: { throughSeq: number; lastHash: string } | null): ChainCheck {
  let previous: AuditEvent | undefined;
  for (const e of events) {
    const fail = (reason: string) => ({ ok: false as const, count: events.length, seq: e.seq, reason });
    if (!previous) {
      const startsChain = e.seq === 1 && e.prevHash === GENESIS;
      const followsPrune = pruned && e.seq === pruned.throughSeq + 1 && e.prevHash === pruned.lastHash;
      if (!startsChain && !followsPrune) return fail("Events before this one are missing.");
    } else {
      if (e.seq !== previous.seq + 1) return fail(`Events ${previous.seq + 1}–${e.seq - 1} are missing.`);
      if (e.prevHash !== previous.hash) return fail("It doesn't follow the event before it.");
    }
    if (eventHash(e) !== e.hash) return fail("It was changed after it was written.");
    previous = e;
  }
  return { ok: true, count: events.length, firstSeq: events[0]?.seq ?? null };
}

const PAGE = 1000;

/** Verifies a workspace's whole chain. */
export async function verifyWorkspaceChain(db: Database, organizationId: string): Promise<ChainCheck> {
  const [prune] = await db
    .select({ metadata: auditEvent.metadata })
    .from(auditEvent)
    .where(and(eq(auditEvent.organizationId, organizationId), eq(auditEvent.action, "audit.pruned")))
    .orderBy(desc(auditEvent.seq))
    .limit(1);
  const pruned = prune?.metadata as { throughSeq: number; lastHash: string } | undefined;
  // Verified in pages, carrying the last event over so each page links to the one before.
  let count = 0;
  let firstSeq: number | null = null;
  let carry: AuditEvent | undefined;
  for (;;) {
    const page = await db
      .select()
      .from(auditEvent)
      .where(and(eq(auditEvent.organizationId, organizationId), gt(auditEvent.seq, carry?.seq ?? 0)))
      .orderBy(asc(auditEvent.seq))
      .limit(PAGE);
    if (!page.length) break;
    const check = verifyChain(carry ? [carry, ...page] : page, carry ? { throughSeq: carry.seq - 1, lastHash: carry.prevHash } : pruned);
    if (!check.ok) return { ...check, count: count + page.length };
    firstSeq ??= page[0]!.seq;
    count += page.length;
    carry = page.at(-1);
    if (page.length < PAGE) break;
  }
  return { ok: true, count, firstSeq };
}

export interface AuditFilters {
  /** An action, or a category such as `guide` for every `guide.*` action. */
  action?: string;
  actorId?: string;
  targetId?: string;
  from?: Date;
  to?: Date;
  /** Paging: events before this seq. */
  before?: number;
  limit?: number;
}

export { auditFilters };

/** Filters from the URL (`auditFilters`) as `listAudit` takes them. */
export function parseFilters(input: AuditSearch): AuditFilters {
  return {
    ...input,
    from: input.from ? new Date(`${input.from}T00:00:00Z`) : undefined,
    to: input.to ? new Date(`${input.to}T23:59:59.999Z`) : undefined,
  };
}

function filterWhere(organizationId: string, f: AuditFilters): SQL {
  const conditions: (SQL | undefined)[] = [eq(auditEvent.organizationId, organizationId)];
  if (f.action) conditions.push(f.action.includes(".") ? eq(auditEvent.action, f.action) : like(auditEvent.action, `${f.action}.%`));
  if (f.actorId) conditions.push(eq(auditEvent.actorId, f.actorId));
  if (f.targetId) conditions.push(eq(auditEvent.targetId, f.targetId));
  if (f.from) conditions.push(gte(auditEvent.createdAt, f.from));
  if (f.to) conditions.push(lte(auditEvent.createdAt, f.to));
  if (f.before) conditions.push(lt(auditEvent.seq, f.before));
  return and(...conditions)!;
}

export type AuditRow = AuditEvent & { actorName: string | null; actorEmail: string | null };

/** Events matching the filters, newest first, with their actors' names. */
export async function listAudit(db: Database, organizationId: string, filters: AuditFilters = {}): Promise<AuditRow[]> {
  const rows = await db
    .select()
    .from(auditEvent)
    .where(filterWhere(organizationId, filters))
    .orderBy(desc(auditEvent.seq))
    .limit(filters.limit ?? 100);
  const actorIds = [...new Set(rows.flatMap((r) => (r.actorId ? [r.actorId] : [])))];
  const actors = actorIds.length
    ? await db.select({ id: user.id, name: user.name, email: user.email }).from(user).where(inArray(user.id, actorIds))
    : [];
  const byId = new Map(actors.map((a) => [a.id, a]));
  return rows.map((r) => ({ ...r, actorName: byId.get(r.actorId ?? "")?.name ?? null, actorEmail: byId.get(r.actorId ?? "")?.email ?? null }));
}

const CSV_COLUMNS = [
  "seq",
  "createdAt",
  "actorId",
  "actorEmail",
  "action",
  "targetType",
  "targetId",
  "beforeHash",
  "afterHash",
  "ip",
  "userAgent",
  "metadata",
  "prevHash",
  "hash",
] as const;

function csvCell(value: unknown): string {
  const text = value == null ? "" : value instanceof Date ? value.toISOString() : typeof value === "object" ? canonical(value) : String(value);
  return /[",\r\n]/.test(text) || /^[=+\-@]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

/** Events as CSV, one row each, oldest first. */
export function toCsv(rows: AuditRow[]): string {
  const ordered = [...rows].sort((a, b) => a.seq - b.seq);
  return [CSV_COLUMNS.join(","), ...ordered.map((r) => CSV_COLUMNS.map((c) => csvCell(r[c])).join(","))].join("\r\n") + "\r\n";
}

/** Events as JSON, oldest first, with what's needed to check their hashes. */
export function toJson(rows: AuditRow[]) {
  return [...rows]
    .sort((a, b) => a.seq - b.seq)
    .map((r) => ({ ...r, createdAt: r.createdAt.toISOString() }));
}

/** How many days the workspace keeps events; null: forever. */
export async function auditRetention(db: Database, organizationId: string): Promise<number | null> {
  const [row] = await db.select().from(auditSettings).where(eq(auditSettings.organizationId, organizationId));
  return row?.retentionDays ?? null;
}

export async function setAuditRetention(db: Database, organizationId: string, actor: RequestMeta & { userId: string }, days: number | null) {
  if (days !== null && (!Number.isInteger(days) || days < MIN_RETENTION_DAYS || days > MAX_RETENTION_DAYS)) {
    return { ok: false as const, error: `Keep events for ${MIN_RETENTION_DAYS} to ${MAX_RETENTION_DAYS} days, or forever.` };
  }
  return db.transaction(async (tx) => {
    const before = await auditRetention(tx, organizationId);
    await tx
      .insert(auditSettings)
      .values({ organizationId, retentionDays: days })
      .onConflictDoUpdate({ target: auditSettings.organizationId, set: { retentionDays: days, updatedAt: new Date() } });
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: "audit.retention",
      targetType: "workspace",
      targetId: organizationId,
      before: { retentionDays: before },
      after: { retentionDays: days },
      metadata: { retentionDays: days },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    return { ok: true as const };
  });
}

/**
 * Deletes events older than each workspace's retention, then records the
 * prune: the last deleted seq and hash, where the chain now starts.
 * Returns how many events went. Every workspace, or just `organizationId`.
 */
export async function pruneAudit(db: Database, now = new Date(), organizationId?: string): Promise<number> {
  const settings = await db
    .select()
    .from(auditSettings)
    .where(and(isNotNull(auditSettings.retentionDays), organizationId ? eq(auditSettings.organizationId, organizationId) : undefined));
  let total = 0;
  for (const { organizationId, retentionDays } of settings) {
    const cutoff = new Date(now.getTime() - retentionDays! * DAY);
    total += await db.transaction(async (tx) => {
      await tx.execute(sql`select pg_advisory_xact_lock(hashtext(${`audit:${organizationId}`}))`);
      const [{ through } = { through: null }] = await tx
        .select({ through: max(auditEvent.seq) })
        .from(auditEvent)
        .where(and(eq(auditEvent.organizationId, organizationId), lt(auditEvent.createdAt, cutoff)));
      if (through == null) return 0;
      const [last] = await tx
        .select({ hash: auditEvent.hash })
        .from(auditEvent)
        .where(and(eq(auditEvent.organizationId, organizationId), eq(auditEvent.seq, Number(through))));
      await tx.execute(sql`select set_config('guiderecorder.audit_prune', 'on', true)`);
      const deleted = await tx
        .delete(auditEvent)
        .where(and(eq(auditEvent.organizationId, organizationId), lte(auditEvent.seq, Number(through))))
        .returning({ seq: auditEvent.seq });
      await tx.execute(sql`select set_config('guiderecorder.audit_prune', 'off', true)`);
      await recordAudit(tx, organizationId, {
        actorId: null,
        action: "audit.pruned",
        targetType: "workspace",
        targetId: organizationId,
        metadata: { throughSeq: Number(through), lastHash: last!.hash, before: cutoff.toISOString(), count: deleted.length },
        at: now,
      });
      return deleted.length;
    });
  }
  return total;
}
