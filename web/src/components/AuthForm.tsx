import { Link, useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { authClient } from "@/lib/auth-client";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { ErrorText, Field } from "@/components/ui";

export interface AuthFormProps {
  mode: "sign-in" | "sign-up";
  providers: { google: boolean; microsoft: boolean };
  /** Where to go after signing in. */
  next?: string;
  /** An error code from a redirect back here, e.g. `SSO_REQUIRED`. */
  error?: string;
}

const ERRORS: Record<string, string> = {
  SSO_REQUIRED: "Your organization requires single sign-on. Enter your work email and choose Sign in with SSO.",
  SSO_NOT_AVAILABLE: "Single sign-on isn't available for this workspace's plan.",
};

export function AuthForm({ mode, providers, next = "/library", error: initialError }: AuthFormProps) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(initialError ? (ERRORS[initialError] ?? "Sign-in failed. Try again.") : null);
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

  // Finds the identity provider for the email's domain and goes there.
  function singleSignOn(event: FormEvent<HTMLButtonElement>) {
    const email = (event.currentTarget.form?.elements.namedItem("email") as HTMLInputElement | null)?.value;
    if (!email) return setError("Enter your work email first.");
    void run(
      () => authClient.signIn.sso({ email, callbackURL: next, errorCallbackURL: "/sign-in" }),
      () => {},
    );
  }

  const social = (provider: "google" | "microsoft") =>
    run(() => authClient.signIn.social({ provider, callbackURL: next }), () => {});

  return (
    <Card className="p-6 block w-full max-w-sm">
      <h1 className="mb-1 font-serif text-3xl">{mode === "sign-in" ? "Sign in" : "Create your account"}</h1>
      <p className="mb-6 text-sm text-muted-foreground">
        {mode === "sign-in" ? "Welcome back to GuideRecorder." : "Share and sync your guides."}
      </p>
      {/* POST: a submit before hydration must not put the password in the URL. */}
      <form method="post" onSubmit={submit} className="grid gap-4">
        {mode === "sign-up" && (
          <Field>
            Name
            <Input name="name" autoComplete="name" required />
          </Field>
        )}
        <Field>
          Email
          <Input name="email" type="email" autoComplete="email" required />
        </Field>
        <Field>
          Password
          <Input
            name="password"
            type="password"
            autoComplete={mode === "sign-in" ? "current-password" : "new-password"}
            minLength={10}
            required
          />
        </Field>
        <ErrorText>{error}</ErrorText>
        {notice && <p className="text-sm text-success">{notice}</p>}
        <Button variant="brand" size="lg" type="submit" disabled={busy}>
          {mode === "sign-in" ? "Sign in" : "Create account"}
        </Button>
        {mode === "sign-in" && (
          <Button type="button" variant="outline" size="lg" disabled={busy} onClick={magicLink}>
            Email me a sign-in link
          </Button>
        )}
        {mode === "sign-in" && (
          <Button type="button" variant="outline" size="lg" disabled={busy} onClick={singleSignOn}>
            Sign in with SSO
          </Button>
        )}
      </form>
      {(providers.google || providers.microsoft) && (
        <div className="mt-4 grid gap-2 border-t border-border pt-4">
          {providers.google && (
            <Button variant="outline" size="lg" disabled={busy} onClick={() => social("google")}>
              Continue with Google
            </Button>
          )}
          {providers.microsoft && (
            <Button variant="outline" size="lg" disabled={busy} onClick={() => social("microsoft")}>
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
