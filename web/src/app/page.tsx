import Link from "next/link";

export default function Home() {
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
        <Link href="/sign-up" className="inline-flex h-10 items-center rounded-lg bg-brand px-5 text-sm font-medium text-brand-foreground hover:bg-brand/90">
          Get started
        </Link>
        <Link href="/sign-in" className="inline-flex h-10 items-center rounded-lg border border-border bg-card px-5 text-sm font-medium hover:bg-muted">
          Sign in
        </Link>
      </div>
    </main>
  );
}
