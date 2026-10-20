import { createFileRoute, Link } from "@tanstack/react-router";
import { ExportMenu } from "@/components/guides/ExportMenu";
import { GuideView } from "@/components/guides/GuideView";
import { ShareLinks } from "@/components/guides/ShareLinks";
import { title } from "@/lib/title";
import { getSyncedGuide } from "@/server/guides";
import { getShareLinks } from "@/server/shares";

export const Route = createFileRoute("/guides/$guideId")({
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
  const { workspaceId, revision, folder, canEdit, exportFormats, shares } = Route.useLoaderData();
  return (
    // Remount for a new revision so the viewer starts afresh.
    <GuideView
      key={revision.id}
      revision={revision}
      canEdit={canEdit}
      assetUrl={(sha) => `/api/workspaces/${workspaceId}/assets/${sha}`}
      header={
        <>
          <Link to="/library" search={folder ? { folder } : {}} className="mr-auto text-sm text-muted-foreground hover:underline">
            ← {folder ?? "Library"}
          </Link>
          <ExportMenu guideId={revision.guideId} revisionId={revision.id} formats={exportFormats} />
          <ShareLinks guideId={revision.guideId} revisionId={revision.id} shares={shares} />
        </>
      }
    />
  );
}
