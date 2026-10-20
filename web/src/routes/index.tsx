import { createFileRoute } from "@tanstack/react-router";
import { Landing } from "@/components/landing/Landing";
import { getViewer } from "@/server/session";

export const Route = createFileRoute("/")({
  loader: async () => ({ signedIn: (await getViewer()) !== null }),
  head: () => ({
    meta: [
      { title: "GuideRecorder · Do it once, get the guide" },
      { name: "description", content: "Record a task on your Mac and get a step-by-step guide with screenshots. Edit it, then share it as a PDF, doc, video or link." },
    ],
  }),
  component: Home,
});

function Home() {
  return <Landing signedIn={Route.useLoaderData().signedIn} />;
}
