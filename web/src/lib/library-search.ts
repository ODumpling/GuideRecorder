import { z } from "zod";
import type { GuideStatus } from "@guiderecorder/guide-schema";

// The library page's search params, shared by the page (client) and its
// server function, so this module stays free of server code.

export const GUIDE_STATUSES = ["draft", "inReview", "published", "needsUpdate"] as const satisfies readonly GuideStatus[];

export const STATUS_LABEL: Record<GuideStatus, string> = {
  draft: "Draft",
  inReview: "In review",
  published: "Published",
  needsUpdate: "Needs update",
};

export const librarySearch = z.object({
  /** Search words. */
  q: z.string().max(200).optional(),
  /** A folder; empty: unfiled guides. */
  folder: z.string().max(200).optional(),
  tag: z.string().max(200).optional(),
  status: z.enum(GUIDE_STATUSES).optional(),
  /** Only guides whose review is overdue or due soon. */
  review: z.literal("due").optional(),
});

export type LibrarySearch = z.infer<typeof librarySearch>;
