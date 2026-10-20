import { Cloud, CloudAlert, CloudOff, CloudUpload } from "lucide-react";
import type { GuideStatus, GuideSync } from "@/sync/engine";
import { guideSyncStatus } from "@/sync/store";
import { useSync } from "@/sync/sync";
import { cn } from "@guiderecorder/ui/utils";

const BADGES: Record<GuideStatus, { icon: typeof Cloud; label: string; className: string }> = {
  synced: { icon: Cloud, label: "Synced with the web", className: "text-muted-foreground" },
  pending: { icon: CloudUpload, label: "Waiting to sync", className: "text-muted-foreground" },
  conflict: { icon: CloudAlert, label: "Changed here and on the web: right-click to resolve", className: "text-[var(--status-warning)]" },
  error: { icon: CloudOff, label: "Sync failed", className: "text-[var(--status-danger)]" },
  deleted: { icon: CloudOff, label: "Deleted on the web; no longer syncing", className: "text-muted-foreground" },
};

/** Where an approval-gated guide stands in review. */
const REVIEW_LABEL: Record<NonNullable<GuideSync["review"]>["status"], string> = {
  draft: "draft",
  inReview: "in review",
  published: "published",
  needsUpdate: "changes requested",
};

/** The guide's sync status on its card; nothing if it doesn't sync. */
export function SyncBadge({ guideId }: { guideId: string }) {
  const status = useSync((s) => guideSyncStatus(s, guideId));
  const error = useSync((s) => s.state.guides[guideId]?.error);
  const review = useSync((s) => s.state.guides[guideId]?.review);
  if (!status) return null;
  const { icon: Icon, label, className } = BADGES[status];
  const reviewed = review?.gated ? `${label} · ${REVIEW_LABEL[review.status]} on the web` : label;
  const title = error ? `${reviewed}: ${error}` : reviewed;
  return (
    <span title={title} role="img" aria-label={title} className="shrink-0">
      <Icon className={cn("size-3.5", className)} />
    </span>
  );
}
