import { useState, type FormEvent } from "react";
import { authClient } from "@/lib/auth-client";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { ErrorText, Field } from "@/components/ui";

type Step = "enter" | "confirm" | "approved" | "denied";

/** Approves the code the desktop app shows, signing the app in as this user. */
export function ApproveDevice({ initialCode }: { initialCode: string }) {
  const [code, setCode] = useState(initialCode);
  const [step, setStep] = useState<Step>("enter");
  const [error, setError] = useState<string | null>(null);

  async function check(event: FormEvent) {
    event.preventDefault();
    setError(null);
    // Looking the code up while signed in claims it for this user.
    const { data, error } = await authClient.device({ query: { user_code: code.trim() } });
    if (error || !data) return setError(error?.error_description ?? "That code isn't valid. Check the desktop app.");
    if (data.status !== "pending") return setError("That code has already been used. Start again in the desktop app.");
    setStep("confirm");
  }

  async function respond(approve: boolean) {
    setError(null);
    const userCode = code.trim();
    const { error } = approve ? await authClient.device.approve({ userCode }) : await authClient.device.deny({ userCode });
    if (error) return setError(error.error_description ?? "Something went wrong.");
    setStep(approve ? "approved" : "denied");
  }

  return (
    <Card className="p-6 block w-full max-w-sm">
      <h1 className="mb-4 font-serif text-3xl">Sign in the desktop app</h1>
      {step === "enter" ? (
        <form onSubmit={check} className="grid gap-4">
          <Field>
            Code shown in the app
            <Input value={code} onChange={(e) => setCode(e.target.value)} autoFocus autoComplete="off" className="font-mono uppercase tracking-widest" />
          </Field>
          <Button variant="brand" size="lg" type="submit" disabled={!code.trim()}>
            Continue
          </Button>
        </form>
      ) : step === "confirm" ? (
        <div className="grid gap-4">
          <p className="text-sm">
            Let GuideRecorder on your computer sync guides with your workspaces? Only approve this if you started signing in from the app and the
            code matches: <span className="font-mono font-semibold">{code.trim().toUpperCase()}</span>
          </p>
          <div className="flex gap-2">
            <Button variant="brand" size="lg" onClick={() => respond(true)}>Approve</Button>
            <Button variant="outline" size="lg" onClick={() => respond(false)}>
              Deny
            </Button>
          </div>
        </div>
      ) : (
        <p className="text-sm">{step === "approved" ? "Done. You can go back to the app." : "Denied. The app wasn't signed in."}</p>
      )}
      <div className="mt-2">
        <ErrorText>{error}</ErrorText>
      </div>
    </Card>
  );
}
