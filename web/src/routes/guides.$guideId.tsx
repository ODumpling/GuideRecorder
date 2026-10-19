import { createFileRoute } from "@tanstack/react-router";
import { GuideView } from "@/components/guides/GuideView";
import { title } from "@/lib/title";
import { getSyncedGuide } from "@/server/guides";

export const Route = createFileRoute("/guides/$guideId")({
  loader: ({ params }) => getSyncedGuide({ data: { guideId: params.guideId } }),
  head: ({ loaderData }) => title(loaderData?.revision.guide.title ?? "Guide"),
  component: Page,
});

function Page() {
  const data = Route.useLoaderData();
  // Remount after a save so the form starts from the new revision.
  return <GuideView key={data.revision.id} {...data} />;
}
