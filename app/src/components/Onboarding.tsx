import { useCallback, useEffect, useState } from "react";
import { Dialog } from "radix-ui";
import { Check, CircleAlert, FolderOpen, KeyRound, ShieldCheck } from "lucide-react";
import { PermissionList } from "@/components/settings/sections";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { errorMessage, tauriSettingsApi, type SettingsApi } from "@/lib/ipc";
import type { LibraryInfo, Permission, PermissionStatus } from "@/lib/types";
import { cn } from "@/lib/utils";
import { allGranted, setupStore, STEPS, useSetupStep, type SetupStep } from "@/onboarding/onboarding";
import { switchLibrary } from "@/settings/library";
import { settingsStore } from "@/settings/settings";

const TITLES: Record<SetupStep, { title: string; icon: typeof Check }> = {
  permissions: { title: "Permissions", icon: ShieldCheck },
  library: { title: "Library folder", icon: FolderOpen },
  apiKey: { title: "API key", icon: KeyRound },
};

function PermissionsStep({ api, onNext }: { api: SettingsApi; onNext(): void }) {
  const [granted, setGranted] = useState(false);
  const onRows = useCallback(
    (rows: { permission: Permission; status: PermissionStatus }[]) => setGranted(allGranted(rows)),
    [],
  );
  return (
    <>
      <p className="text-sm text-muted-foreground">
        To record a guide, GuideRecorder needs to see the screen and notice your clicks and typing. Allow each one in
        System Settings. You can edit and export guides without them; recording stays off until all three are
        allowed. Screen Recording may need GuideRecorder to restart.
      </p>
      <PermissionList api={api} poll onRows={onRows} />
      <p className={cn("text-xs", granted ? "text-[var(--status-success)]" : "text-muted-foreground")}>
        {granted ? "All set: recording is available." : "Recording is off until every permission is allowed."}
      </p>
      <div className="flex justify-end">
        <Button onClick={onNext}>{granted ? "Continue" : "Continue without recording"}</Button>
      </div>
    </>
  );
}

function LibraryStep({ api, onNext }: { api: SettingsApi; onNext(): void }) {
  const [info, setInfo] = useState<LibraryInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(() => {
    api.libraryInfo().then(setInfo, (e) => setError(errorMessage(e)));
  }, [api]);
  useEffect(refresh, [refresh]);

  const use = async (pick: () => Promise<string | null | undefined>) => {
    setError(null);
    setBusy(true);
    try {
      const path = await pick();
      if (path !== undefined) await switchLibrary(api, path);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
      refresh();
    }
  };
  // `undefined`: cancelled; `null`: the default folder.
  const choose = () => use(async () => (await api.chooseLibrary()) ?? undefined);
  const useDefault = () => use(async () => null);
  const ready = !!info && !info.error && !info.starting && !!info.path;

  return (
    <>
      <p className="text-sm text-muted-foreground">
        Guides are stored as folders in your library. Keep the default, or choose a folder, for example one that
        is synced or shared with your team.
      </p>
      <div className="flex flex-col gap-1 rounded-md border border-border p-3">
        <span className="text-xs text-muted-foreground">Library</span>
        <span className="truncate text-sm font-medium" title={info?.path ?? undefined}>
          {info?.path ?? "Not set"}
        </span>
        {info?.error && (
          <span className="flex items-start gap-1.5 text-xs text-[var(--status-danger)]">
            <CircleAlert className="mt-0.5 size-3.5 shrink-0" /> {info.error}
          </span>
        )}
        {info?.fromEnv && (
          <span className="text-xs text-muted-foreground">GUIDERECORDER_LIBRARY sets this folder at startup.</span>
        )}
      </div>
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" disabled={busy} onClick={() => void choose()}>
          Choose a folder…
        </Button>
        {info?.defaultPath && (info.path !== info.defaultPath || info.error) && (
          <Button variant="outline" disabled={busy} onClick={() => void useDefault()} title={info.defaultPath}>
            Use ~/Documents/GuideRecorder
          </Button>
        )}
      </div>
      {error && <p className="text-xs text-[var(--status-danger)]">{error}</p>}
      <div className="flex justify-end">
        <Button disabled={!ready || busy} onClick={onNext}>
          Continue
        </Button>
      </div>
    </>
  );
}

function ApiKeyStep({ api, onDone }: { api: SettingsApi; onDone(): void }) {
  const [key, setKey] = useState("");
  const [stored, setStored] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.apiKeyStored().then(setStored, () => setStored(false));
  }, [api]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.setApiKey(key);
      setKey("");
      onDone();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (key.trim()) void save();
      }}
    >
      <p className="text-sm text-muted-foreground">
        With an Anthropic API key, GuideRecorder writes the step text for you from the screenshots. It's optional: you
        can write guides yourself, or add a key later in Settings. The key is kept in your Keychain.
      </p>
      {stored && <p className="text-xs text-[var(--status-success)]">A key is already stored.</p>}
      <Input
        type="password"
        aria-label="API key"
        autoComplete="off"
        placeholder={stored ? "Replace the stored key" : "sk-ant-…"}
        value={key}
        disabled={busy}
        onChange={(e) => setKey(e.target.value)}
      />
      {error && <p className="text-xs text-[var(--status-danger)]">{error}</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="outline" disabled={busy} onClick={onDone}>
          {stored ? "Keep the stored key" : "Skip"}
        </Button>
        <Button type="submit" disabled={busy || !key.trim()}>
          Save and finish
        </Button>
      </div>
    </form>
  );
}

/**
 * First-run setup: permissions, the library folder, an optional API key.
 * Shown on first launch or when the library can't be opened; Settings
 * reopens it. Finishing (or skipping to the end) marks it done.
 */
export function Onboarding({ api = tauriSettingsApi }: { api?: SettingsApi }) {
  const step = useSetupStep();
  const [finishError, setFinishError] = useState<string | null>(null);
  const go = (next: SetupStep) => setupStore.getState().open(next);

  const finish = async () => {
    setFinishError(null);
    const s = settingsStore.getState();
    if (s.settings && !s.settings.general.onboarded && !s.readOnly) {
      const ok = await s.change((x) => ({ ...x, general: { ...x.general, onboarded: true } }));
      if (!ok) {
        setFinishError(settingsStore.getState().error ?? "Setup could not be saved.");
        return;
      }
    }
    setupStore.getState().close();
  };

  const index = step ? STEPS.indexOf(step) : 0;
  return (
    <Dialog.Root open={step !== null} onOpenChange={(open) => !open && setupStore.getState().close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <Dialog.Content
          // Setup is finished with its buttons, not by clicking outside.
          onPointerDownOutside={(e) => e.preventDefault()}
          className="fixed top-1/2 left-1/2 z-50 flex max-h-[90vh] w-[34rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 overflow-y-auto rounded-lg border border-border bg-background p-6 text-foreground shadow-lg"
        >
          <Dialog.Title className="text-lg font-semibold">Set up GuideRecorder</Dialog.Title>
          <Dialog.Description className="sr-only">First-run setup in three steps.</Dialog.Description>
          <ol aria-label="Setup steps" className="flex gap-2">
            {STEPS.map((s, i) => {
              const Icon = TITLES[s].icon;
              return (
                <li key={s} className="flex-1">
                  <button
                    type="button"
                    aria-current={s === step ? "step" : undefined}
                    onClick={() => go(s)}
                    className={cn(
                      "flex w-full items-center gap-1.5 rounded-md border px-2 py-1.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring",
                      s === step
                        ? "border-[var(--status-info)] font-medium"
                        : i < index
                          ? "border-border text-muted-foreground"
                          : "border-border",
                    )}
                  >
                    {i < index ? <Check className="size-3.5" /> : <Icon className="size-3.5" />}
                    {i + 1}. {TITLES[s].title}
                  </button>
                </li>
              );
            })}
          </ol>
          {step === "permissions" && <PermissionsStep api={api} onNext={() => go("library")} />}
          {step === "library" && <LibraryStep api={api} onNext={() => go("apiKey")} />}
          {step === "apiKey" && <ApiKeyStep api={api} onDone={() => void finish()} />}
          {finishError && <p className="text-xs text-[var(--status-danger)]">{finishError}</p>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
