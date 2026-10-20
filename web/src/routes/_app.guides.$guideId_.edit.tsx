import { createFileRoute, Link } from "@tanstack/react-router";
import { GuideEditor } from "@/components/guides/GuideEditor";
import { Card } from "@guiderecorder/ui/card";
import { title } from "@/lib/title";
import { getGuideToEdit } from "@/server/editor";

export const Route = createFileRoute("/_app/guides/$guideId_/edit")({
  loader: ({ params }) => getGuideToEdit({ data: { guideId: params.guideId } }),
  head: ({ loaderData }) => title(loaderData?.allowed ? `Edit ${loaderData.revision.guide.title}` : "Edit guide"),
  // The editors (TipTap) only run in the browser.
  ssr: "data-only",
  component: Page,
});

function Page() {
  const data = Route.useLoaderData();
  const { guideId } = Route.useParams();
  if (!data.allowed) {
    return (
      <div className="mx-auto grid w-full max-w-3xl gap-6 p-6">
        <Card className="p-6 grid gap-3">
          <h1 className="font-serif text-3xl">You can't edit this guide</h1>
          <p className="text-sm text-muted-foreground">{data.error}</p>
          <Link to="/guides/$guideId" params={{ guideId }} className="text-sm text-brand hover:underline">
            Back to the guide
          </Link>
        </Card>
      </div>
    );
  }
  // Remount after a save so the editor starts from the new revision.
  return <GuideEditor key={data.revision.id} workspaceId={data.workspaceId} revision={data.revision} />;
}
