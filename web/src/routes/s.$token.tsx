import { createFileRoute } from "@tanstack/react-router";
import { SharedGuide } from "@/components/guides/SharedGuide";
import { getSharedGuide } from "@/server/shares";

const NOINDEX = "noindex, nofollow";

/** A guide shared with a link. Public: no session, nothing of the workspace but the guide. */
export const Route = createFileRoute("/s/$token")({
  loader: ({ params }) => getSharedGuide({ data: { token: params.token } }),
  // Fresh on every visit: views are counted, and links get revoked.
  staleTime: 0,
  gcTime: 0,
  headers: () => ({ "X-Robots-Tag": NOINDEX, "Cache-Control": "private, no-store", "Referrer-Policy": "no-referrer" }),
  head: ({ loaderData }) => ({
    meta: [
      { title: loaderData?.status === 200 ? `${loaderData.shared.guide.title} · GuideRecorder` : "Shared guide · GuideRecorder" },
      { name: "robots", content: NOINDEX },
      // The token is in the URL: don't hand it to image hosts.
      { name: "referrer", content: "no-referrer" },
    ],
  }),
  component: Page,
});

function Page() {
  const { token } = Route.useParams();
  return <SharedGuide token={token} result={Route.useLoaderData()} />;
}
