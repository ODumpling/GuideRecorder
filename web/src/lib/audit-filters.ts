import { z } from "zod";

/**
 * Audit log filters as the audit page and export take them (in the URL).
 * `action` is an action or a category (`guide` for every `guide.*`); days
 * are `YYYY-MM-DD`, UTC; `before` pages back from a seq. Kept apart from
 * `audit.ts` so the page can parse its URL without server code.
 */
export const auditFilters = z.object({
  action: z.string().max(100).optional(),
  actorId: z.string().max(200).optional(),
  targetId: z.string().max(200).optional(),
  from: z.iso.date().optional(),
  to: z.iso.date().optional(),
  before: z.coerce.number().int().positive().optional(),
});
export type AuditSearch = z.infer<typeof auditFilters>;
