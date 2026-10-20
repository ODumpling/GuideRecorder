import { createFileRoute, Link } from "@tanstack/react-router";
import { Button } from "@guiderecorder/ui/button";

export const Route = createFileRoute("/")({ component: Home });

function Home() {
  return (
    <main className="mx-auto flex w-full max-w-3xl flex-1 flex-col justify-center gap-6 p-8">
      <h1 className="font-serif text-6xl leading-tight">
        Your guides, <em>shared</em>.
      </h1>
      <p className="max-w-xl text-lg text-muted-foreground">
        Sync guides from the GuideRecorder app, share them with a link, and keep your team&rsquo;s library reviewed and
        current.
      </p>
      <div className="flex gap-3">
        <Button asChild variant="brand" size="lg" className="h-10 px-5">
          <Link to="/sign-up">Get started</Link>
        </Button>
        <Button asChild variant="outline" size="lg" className="h-10 px-5">
          <Link to="/sign-in">Sign in</Link>
        </Button>
      </div>
    </main>
  );
}
