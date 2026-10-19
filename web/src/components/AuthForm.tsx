import { Link, useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { authClient } from "@/lib/auth-client";
import { Button, Card, ErrorText, Input, Label } from "./ui";

export interface AuthFormProps {
  mode: "sign-in" | "sign-up";
  providers: { google: boolean; microsoft: boolean };
  /** Where to go after signing in. */
  next?: string;
}

export function AuthForm({ mode, providers, next = "/dashboard" }: AuthFormProps) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function run(action: () => Promise<{ error: { message?: string } | null }>, onSuccess: () => void) {
    setBusy(true);
    setError(null);
    const { error } = await action();
    setBusy(false);
    if (error) setError(error.message ?? "Something went wrong.");
    else onSuccess();
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const email = String(form.get("email"));
    const password = String(form.get("password"));
    if (mode === "sign-up") {
      void run(() => authClient.signUp.email({ name: String(form.get("name")), email, password }), () => router.navigate({ href: next }));
    } else {
      void run(() => authClient.signIn.email({ email, password }), () => router.navigate({ href: next }));
    }
  }

  function magicLink(event: FormEvent<HTMLButtonElement>) {
    const email = (event.currentTarget.form?.elements.namedItem("email") as HTMLInputElement | null)?.value;
    if (!email) return setError("Enter your email first.");
    void run(
      () => authClient.signIn.magicLink({ email, callbackURL: next }),
      () => setNotice(`We sent a sign-in link to ${email}.`),
    );
  }

  const social = (provider: "google" | "microsoft") =>
    run(() => authClient.signIn.social({ provider, callbackURL: next }), () => {});

  return (
    <Card className="w-full max-w-sm">
      <h1 className="mb-1 font-serif text-3xl">{mode === "sign-in" ? "Sign in" : "Create your account"}</h1>
      <p className="mb-6 text-sm text-muted-foreground">
        {mode === "sign-in" ? "Welcome back to GuideRecorder." : "Share and sync your guides."}
      </p>
      <form onSubmit={submit} className="grid gap-4">
        {mode === "sign-up" && (
          <Label>
            Name
            <Input name="name" autoComplete="name" required />
          </Label>
        )}
        <Label>
          Email
          <Input name="email" type="email" autoComplete="email" required />
        </Label>
        <Label>
          Password
          <Input
            name="password"
            type="password"
            autoComplete={mode === "sign-in" ? "current-password" : "new-password"}
            minLength={10}
            required
          />
        </Label>
        <ErrorText>{error}</ErrorText>
        {notice && <p className="text-sm text-success">{notice}</p>}
        <Button type="submit" disabled={busy}>
          {mode === "sign-in" ? "Sign in" : "Create account"}
        </Button>
        {mode === "sign-in" && (
          <Button type="button" variant="secondary" disabled={busy} onClick={magicLink}>
            Email me a sign-in link
          </Button>
        )}
      </form>
      {(providers.google || providers.microsoft) && (
        <div className="mt-4 grid gap-2 border-t border-border pt-4">
          {providers.google && (
            <Button variant="secondary" disabled={busy} onClick={() => social("google")}>
              Continue with Google
            </Button>
          )}
          {providers.microsoft && (
            <Button variant="secondary" disabled={busy} onClick={() => social("microsoft")}>
              Continue with Microsoft
            </Button>
          )}
        </div>
      )}
      <p className="mt-6 text-center text-sm text-muted-foreground">
        {mode === "sign-in" ? (
          <>
            New here? <Link className="text-brand underline-offset-4 hover:underline" to="/sign-up" search={{ next }}>Create an account</Link>
          </>
        ) : (
          <>
            Have an account? <Link className="text-brand underline-offset-4 hover:underline" to="/sign-in" search={{ next }}>Sign in</Link>
          </>
        )}
      </p>
    </Card>
  );
}
