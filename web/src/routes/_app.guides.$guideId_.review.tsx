import { createFileRoute, Link } from "@tanstack/react-router";
import { ReviewPage } from "@/components/guides/ReviewPage";
import { Card } from "@guiderecorder/ui/card";
import { title } from "@/lib/title";
import { getGuideReview } from "@/server/reviews";

export const Route = createFileRoute("/_app/guides/$guideId_/review")({
  loader: ({ params }) => getGuideReview({ data: { guideId: params.guideId } }),
  head: ({ loaderData }) => title(loaderData?.allowed ? `Review ${loaderData.after.guide.title}` : "Review"),
  component: Page,
});

function Page() {
  const data = Route.useLoaderData();
  const { guideId } = Route.useParams();
  if (!data.allowed) {
    return (
      <div className="mx-auto grid w-full max-w-3xl gap-6 p-6">
        <Card className="p-6 grid gap-3">
          <h1 className="font-serif text-3xl">You can't review this guide</h1>
          <p className="text-sm text-muted-foreground">{data.error}</p>
          <Link to="/guides/$guideId" params={{ guideId }} className="text-sm text-brand hover:underline">
            Back to the guide
          </Link>
        </Card>
      </div>
    );
  }
  return <ReviewPage key={`${data.changeRequest?.id}.${data.after.id}.${data.reviews.length}`} {...data} />;
}
