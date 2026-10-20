import { createFileRoute, Link } from "@tanstack/react-router";
import { ChevronRight } from "lucide-react";
import { ExportMenu } from "@/components/guides/ExportMenu";
import { GuideView } from "@/components/guides/GuideView";
import { ShareLinks } from "@/components/guides/ShareLinks";
import { title } from "@/lib/title";
import { getSyncedGuide } from "@/server/guides";
import { getShareLinks } from "@/server/shares";

export const Route = createFileRoute("/_app/guides/$guideId")({
  loader: async ({ params }) => {
    const [guide, shares] = await Promise.all([
      getSyncedGuide({ data: { guideId: params.guideId } }),
      getShareLinks({ data: { guideId: params.guideId } }),
    ]);
    return { ...guide, shares };
  },
  head: ({ loaderData }) => title(loaderData?.revision.guide.title ?? "Guide"),
  component: Page,
});

function Page() {
  const { workspaceId, revision, folder, canEdit, canReview, unpublished, exportFormats, shares } = Route.useLoaderData();
  return (
    // Remount for a new revision so the viewer starts afresh.
    <GuideView
      key={revision.id}
      revision={revision}
      canEdit={canEdit}
      canReview={canReview}
      unpublished={unpublished}
      assetUrl={(sha) => `/api/workspaces/${workspaceId}/assets/${sha}`}
      header={
        <>
          <nav aria-label="Breadcrumb" className="mr-auto flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground">
            <Link to="/library" search={{}} className="hover:text-foreground">
              All guides
            </Link>
            {folder ? (
              <>
                <ChevronRight className="size-3.5 shrink-0 text-muted-foreground/60" />
                <Link to="/library" search={{ folder }} className="truncate hover:text-foreground">
                  {folder}
                </Link>
              </>
            ) : null}
          </nav>
          <ExportMenu guideId={revision.guideId} revisionId={revision.id} formats={exportFormats} />
          <ShareLinks guideId={revision.guideId} revisionId={revision.id} shares={shares} />
        </>
      }
    />
  );
}
