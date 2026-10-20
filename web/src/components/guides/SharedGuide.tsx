import { useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { unlockSharedGuide, type getSharedGuide } from "@/server/shares";
import { Button, Card, ErrorText, Input } from "../ui";
import { GuideView } from "./GuideView";

// The public page of a share link: the guide read-only, or a password
// prompt, or why the link doesn't work. The guide renders with GuideView.

type Result = Awaited<ReturnType<typeof getSharedGuide>>;

export function SharedGuide({ token, result }: { token: string; result: Result }) {
  if (result.status === 401) return <PasswordPrompt token={token} />;
  if (result.status !== 200) {
    return (
      <Message title={result.status === 410 ? "Link expired" : result.status === 429 ? "Slow down" : "Link not found"} text={result.error} />
    );
  }
  const { guide, assets, allowDownload, updatedAt, source } = result.shared;
  return (
    <div className="flex flex-1 flex-col">
      <GuideView
        revision={{ id: token, guideId: guide.id, guide, assets, source, createdAt: updatedAt }}
        canEdit={false}
        assetUrl={(sha) => `/api/share/${token}/assets/${sha}`}
        header={
          allowDownload ? (
            // TODO(#48): download a real export (PDF, HTML) from the export worker once it lands.
            <Button variant="secondary" className="ml-auto print:hidden" onClick={() => window.print()}>
              Download PDF
            </Button>
          ) : null
        }
      />
      <footer className="p-6 text-center text-xs text-muted-foreground print:hidden">
        Shared with{" "}
        <a href="/" className="underline-offset-4 hover:underline">
          GuideRecorder
        </a>
      </footer>
    </div>
  );
}

function PasswordPrompt({ token }: { token: string }) {
  const router = useRouter();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const result = await unlockSharedGuide({ data: { token, password } });
    setBusy(false);
    if (!result.ok) return setError(result.error);
    await router.invalidate();
  }

  return (
    <main className="mx-auto grid w-full max-w-sm flex-1 place-content-center p-6">
      <Card className="grid gap-4">
        <h1 className="font-serif text-2xl">Password protected</h1>
        <p className="text-sm text-muted-foreground">Enter the password you were given to see this guide.</p>
        <form className="grid gap-3" onSubmit={submit}>
          <Input type="password" aria-label="Password" autoFocus required value={password} onChange={(e) => setPassword(e.target.value)} />
          <Button type="submit" disabled={busy}>
            {busy ? "Checking…" : "View guide"}
          </Button>
        </form>
        <ErrorText>{error}</ErrorText>
      </Card>
    </main>
  );
}

function Message({ title, text }: { title: string; text: string }) {
  return (
    <main className="mx-auto grid max-w-md flex-1 place-content-center gap-3 p-8 text-center">
      <h1 className="font-serif text-4xl">{title}</h1>
      <p className="text-sm text-muted-foreground">{text}</p>
    </main>
  );
}
