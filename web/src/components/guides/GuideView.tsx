import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import type { Revision } from "@guiderecorder/sync";
import { ClipboardCheck, Pencil } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { StatusPill } from "./bits";
import { GuideViewer } from "./GuideViewer";

// A synced guide on the web, read with <GuideViewer>. Editing is at
// /guides/:id/edit; share links show it read-only.

interface Props {
  revision: Pick<Revision, "id" | "guideId" | "guide" | "assets" | "source" | "createdAt">;
  /** Where to load an image from, by its SHA-256. */
  assetUrl: (sha256: string) => string;
  canEdit: boolean;
  /** Approvals: the review page is open to the user. */
  canReview?: boolean;
  /** The revision shown (the head) isn't what viewers see yet. */
  unpublished?: boolean;
  /** Shown above the guide, e.g. the back link, share links and downloads. */
  header?: ReactNode;
}

export function GuideView({ revision, assetUrl, canEdit, canReview = false, unpublished = false, header }: Props) {
  const imageUrl = (path: string) => {
    const sha = revision.assets[path];
    return sha ? assetUrl(sha) : null;
  };
  const changed = `Last changed ${new Date(revision.createdAt).toLocaleString()} ${revision.source === "web" ? "on the web" : "in the desktop app"}`;

  return (
    <GuideViewer
      guide={revision.guide}
      imageUrl={imageUrl}
      header={
        <header className="flex flex-wrap items-center gap-1.5 empty:hidden print:hidden">
          {header}
          {canReview ? (
            <Button asChild variant="ghost">
              <Link to="/guides/$guideId/review" params={{ guideId: revision.guideId }}>
                <ClipboardCheck /> Review
              </Link>
            </Button>
          ) : null}
          {canEdit ? (
            <Button asChild>
              <Link to="/guides/$guideId/edit" params={{ guideId: revision.guideId }}>
                <Pencil /> Edit
              </Link>
            </Button>
          ) : null}
        </header>
      }
      meta={
        <>
          <p className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            {revision.guide.status ? <StatusPill status={revision.guide.status} /> : null}
            {changed}
          </p>
          {unpublished && canReview ? (
            <p className="text-sm text-muted-foreground">This version isn&rsquo;t published yet: viewers see the last approved one.</p>
          ) : null}
        </>
      }
    />
  );
}
