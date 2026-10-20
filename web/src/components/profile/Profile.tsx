import { useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { authClient } from "@/lib/auth-client";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { ErrorText, Field } from "@/components/ui";

type Result = { error: { message?: string } | null };

/** A form's submit state: busy, an error, or a confirmation. */
function useSubmit() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  async function submit(action: () => Promise<Result>, done: string) {
    setBusy(true);
    setError(null);
    setNotice(null);
    const { error } = await action();
    setBusy(false);
    if (error) setError(error.message ?? "Something went wrong.");
    else setNotice(done);
  }
  return { busy, error, notice, submit };
}

export function AccountDetails({ name, email }: { name: string; email: string }) {
  const router = useRouter();
  const { busy, error, notice, submit } = useSubmit();

  function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const next = String(new FormData(event.currentTarget).get("name")).trim();
    void submit(async () => {
      const result = await authClient.updateUser({ name: next });
      if (!result.error) await router.invalidate();
      return result;
    }, "Saved.");
  }

  return (
    <Card className="block p-6">
      <h2 className="mb-4 text-lg font-semibold">Account</h2>
      <form onSubmit={save} className="grid max-w-sm gap-4">
        <Field>
          Name
          <Input name="name" defaultValue={name} required maxLength={100} autoComplete="name" />
        </Field>
        <Field>
          Email
          <Input value={email} readOnly disabled />
        </Field>
        <div className="flex items-center gap-3">
          <Button type="submit" variant="brand" disabled={busy}>
            Save
          </Button>
          {notice ? <span className="text-sm text-muted-foreground">{notice}</span> : null}
        </div>
        <ErrorText>{error}</ErrorText>
      </form>
    </Card>
  );
}

export function ChangePassword() {
  const { busy, error, notice, submit } = useSubmit();

  function change(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    void submit(async () => {
      const result = await authClient.changePassword({
        currentPassword: String(data.get("current")),
        newPassword: String(data.get("next")),
        revokeOtherSessions: true,
      });
      if (!result.error) form.reset();
      return result;
    }, "Password changed. Other devices were signed out.");
  }

  return (
    <Card className="block p-6">
      <h2 className="text-lg font-semibold">Password</h2>
      <p className="mt-1 mb-4 text-sm text-muted-foreground">Changing it signs you out everywhere else.</p>
      <form onSubmit={change} className="grid max-w-sm gap-4">
        <Field>
          Current password
          <Input name="current" type="password" required autoComplete="current-password" />
        </Field>
        <Field>
          New password
          <Input name="next" type="password" required minLength={10} autoComplete="new-password" />
        </Field>
        <div className="flex items-center gap-3">
          <Button type="submit" variant="brand" disabled={busy}>
            Change password
          </Button>
          {notice ? <span className="text-sm text-muted-foreground">{notice}</span> : null}
        </div>
        <ErrorText>{error}</ErrorText>
      </form>
    </Card>
  );
}
