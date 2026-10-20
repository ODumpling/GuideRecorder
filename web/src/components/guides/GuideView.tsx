import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import type { Revision } from "@guiderecorder/sync";
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
        <header className="flex flex-wrap items-center justify-between gap-4 empty:hidden print:hidden">
          {header}
          {canReview ? (
            <Link
              to="/guides/$guideId/review"
              params={{ guideId: revision.guideId }}
              className="inline-flex h-9 items-center rounded-lg border border-border bg-card px-4 text-sm font-medium hover:bg-muted"
            >
              Review
            </Link>
          ) : null}
          {canEdit ? (
            <Link
              to="/guides/$guideId/edit"
              params={{ guideId: revision.guideId }}
              className="inline-flex h-9 items-center rounded-lg border border-border bg-card px-4 text-sm font-medium hover:bg-muted"
            >
              Edit
            </Link>
          ) : null}
        </header>
      }
      meta={
        <>
          <p className="text-xs text-muted-foreground">{changed}</p>
          {unpublished && canReview ? (
            <p className="text-sm text-muted-foreground">This version isn&rsquo;t published yet: viewers see the last approved one.</p>
          ) : null}
        </>
      }
    />
  );
}
