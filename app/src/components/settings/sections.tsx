import { useCallback, useEffect, useState } from "react";
import { Check, Copy, ExternalLink, RefreshCw, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { CommitInput, HotkeyField, PathText, Row, Section, Select, Toggle } from "@/components/settings/controls";
import { useEditor } from "@/editor/editor";
import { setupStore } from "@/onboarding/onboarding";
import { switchLibrary } from "@/settings/library";
import { FORMATS } from "@/export/export";
import { errorMessage, tauriExportApi, type ExportApi, type SettingsApi } from "@/lib/ipc";
import type { CaptureArea, LibraryInfo, McpSetup, Permission, PermissionStatus, Settings } from "@/lib/types";
import { useSettings, settingsStore } from "@/settings/settings";
import {
  AFTER_FRAME_DELAY_MS,
  bundleIdError,
  inRange,
  isBundleId,
  isHotkey,
  isLanguage,
  isModelId,
  MAX_EXCLUDED_APPS,
  TYPING_IDLE_MS,
} from "@/settings/validate";
import { isAccent } from "@/export/export";
import { appName, DEFAULT_EXCLUDED_IDS } from "@/recording/apps";

type Change = (update: (s: Settings) => Settings) => void;

function useChange(): Change {
  return useCallback((update) => void settingsStore.getState().change(update), []);
}

function ErrorText({ children }: { children: string | null }) {
  if (!children) return null;
  return <p className="mt-2 text-xs text-[var(--status-danger)]">{children}</p>;
}

export function GeneralSection({ api }: { api: SettingsApi }) {
  const settings = useSettings((s) => s.settings)!;
  const readOnly = useSettings((s) => s.readOnly);
  const change = useChange();
  const [library, setLibrary] = useState<LibraryInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loginOn, setLoginOn] = useState<boolean | null>(null);
  const hasDoc = useEditor((s) => s.doc !== null);

  const refresh = useCallback(() => {
    api.libraryInfo().then(setLibrary, (e) => setError(errorMessage(e)));
  }, [api]);
  useEffect(() => {
    refresh();
    api.launchAtLogin().then(setLoginOn, () => setLoginOn(null));
  }, [api, refresh]);

  /** Closes the open guide (saving it), then opens another library. */
  const switchTo = async (path: string | null) => {
    setError(null);
    setBusy(true);
    try {
      await switchLibrary(api, path);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const choose = async () => {
    try {
      const path = await api.chooseLibrary();
      if (path) await switchTo(path);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const setLogin = async (enabled: boolean) => {
    setError(null);
    try {
      setLoginOn(await api.setLaunchAtLogin(enabled));
      await settingsStore.getState().load();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <Section title="General">
      <Row
        label="Library folder"
        note={
          <>
            {library?.error
              ? `Could not open it: ${library.error}`
              : library?.starting
                ? "Opening…"
                : "Where your guides are stored."}
            {library?.fromEnv && " GUIDERECORDER_LIBRARY sets the folder at startup."}
            {hasDoc && " The open guide is saved and closed first."}
          </>
        }
      >
        <PathText path={library?.path ?? null} empty="Not set" />
        <Button variant="outline" size="sm" disabled={busy || readOnly} onClick={() => void choose()}>
          Choose…
        </Button>
        {settings.general.library && (
          <Button variant="ghost" size="sm" disabled={busy || readOnly} onClick={() => void switchTo(null)}>
            Use default
          </Button>
        )}
      </Row>
      <Row label="Launch at login" note="Start GuideRecorder when you log in.">
        <Toggle
          label="Launch at login"
          checked={loginOn ?? settings.general.launchAtLogin}
          disabled={readOnly}
          onChange={(on) => void setLogin(on)}
        />
      </Row>
      <Row label="First-run setup" note="Permissions, the library folder and the API key, step by step.">
        <Button variant="outline" size="sm" onClick={() => setupStore.getState().open()}>
          Run setup again
        </Button>
      </Row>
      <Row label="Menu bar icon" note="Coming with recording; stored for now.">
        <Toggle
          label="Menu bar icon"
          checked={settings.general.menuBarIcon}
          disabled={readOnly}
          onChange={(on) => change((s) => ({ ...s, general: { ...s.general, menuBarIcon: on } }))}
        />
      </Row>
      <ErrorText>{error}</ErrorText>
    </Section>
  );
}

const CAPTURE_AREAS: { value: CaptureArea; label: string }[] = [
  { value: "display", label: "All displays" },
  { value: "window", label: "One window" },
  { value: "region", label: "A region" },
];

export function RecordingSection() {
  const settings = useSettings((s) => s.settings)!;
  const readOnly = useSettings((s) => s.readOnly);
  const change = useChange();
  const r = settings.recording;
  const [app, setApp] = useState("");
  const setRecording = (patch: Partial<Settings["recording"]>) =>
    change((s) => ({ ...s, recording: { ...s.recording, ...patch } }));
  const setHotkey = (name: keyof Settings["recording"]["hotkeys"], value: string) =>
    isHotkey(value) && setRecording({ hotkeys: { ...r.hotkeys, [name]: value } });
  const addApp = () => {
    const id = app.trim();
    if (!isBundleId(id) || r.excludedApps.includes(id) || r.excludedApps.length >= MAX_EXCLUDED_APPS) return;
    setRecording({ excludedApps: [...r.excludedApps, id] });
    setApp("");
  };
  // Adds back the suggested apps that were removed; keeps the user's own.
  const missingDefaults = DEFAULT_EXCLUDED_IDS.filter((id) => !r.excludedApps.includes(id));
  const full = r.excludedApps.length >= MAX_EXCLUDED_APPS;
  const appError = bundleIdError(app.trim());

  return (
    <Section title="Recording" intro="The next recording uses these. With Window or Region as the capture area, you pick the window or draw the region each time you record. The record and pause shortcuts and after frames are not supported yet.">
      <Row label="Start or stop recording">
        <HotkeyField label="Record shortcut" value={r.hotkeys.record} disabled={readOnly} onChange={(v) => setHotkey("record", v)} />
      </Row>
      <Row label="Pause recording">
        <HotkeyField label="Pause shortcut" value={r.hotkeys.pause} disabled={readOnly} onChange={(v) => setHotkey("pause", v)} />
      </Row>
      <Row label="Add a manual step">
        <HotkeyField
          label="Manual step shortcut"
          value={r.hotkeys.manualStep}
          disabled={readOnly}
          onChange={(v) => setHotkey("manualStep", v)}
        />
      </Row>
      <Row label="Capture area">
        <Select
          label="Capture area"
          value={r.captureArea}
          options={CAPTURE_AREAS}
          disabled={readOnly}
          onChange={(captureArea) => setRecording({ captureArea })}
        />
      </Row>
      <Row label="After-frames" note="Also capture the screen shortly after each action.">
        <CommitInput
          label="After-frame delay (ms)"
          type="number"
          className="w-24"
          value={String(r.afterFrameDelayMs)}
          valid={(v) => inRange(Number(v), AFTER_FRAME_DELAY_MS)}
          disabled={readOnly || !r.afterFrames}
          onCommit={(v) => setRecording({ afterFrameDelayMs: Number(v) })}
        />
        <span className="text-xs text-muted-foreground">ms</span>
        <Toggle
          label="After-frames"
          checked={r.afterFrames}
          disabled={readOnly}
          onChange={(afterFrames) => setRecording({ afterFrames })}
        />
      </Row>
      <Row
        label="Typing pause"
        note={`A pause this long ends a typing step (${TYPING_IDLE_MS.min}–${TYPING_IDLE_MS.max} ms).`}
      >
        <CommitInput
          label="Typing pause (ms)"
          type="number"
          className="w-24"
          value={String(r.typingIdleMs)}
          valid={(v) => inRange(Number(v), TYPING_IDLE_MS)}
          disabled={readOnly}
          onCommit={(v) => setRecording({ typingIdleMs: Number(v) })}
        />
        <span className="text-xs text-muted-foreground">ms</span>
      </Row>
      <div className="flex flex-col gap-2 py-3">
        <span className="text-sm font-medium">Excluded apps</span>
        <span className="text-xs text-muted-foreground">
          Nothing is recorded while one of these apps is in front. Use bundle ids, such as com.1password.1password.
        </span>
        {r.excludedApps.length > 0 && (
          <ul className="flex flex-col gap-1">
            {r.excludedApps.map((id) => (
              <li key={id} className="flex items-center justify-between rounded-md bg-muted px-2 py-1 text-sm">
                <span className="flex min-w-0 items-baseline gap-2">
                  {appName(id) !== id && <span className="text-sm">{appName(id)}</span>}
                  <span className="truncate font-mono text-xs text-muted-foreground">{id}</span>
                </span>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Remove ${id}`}
                  disabled={readOnly}
                  onClick={() => setRecording({ excludedApps: r.excludedApps.filter((a) => a !== id) })}
                >
                  <X />
                </Button>
              </li>
            ))}
          </ul>
        )}
        <div className="flex gap-2">
          <Input
            aria-label="App bundle id"
            placeholder="com.example.app"
            value={app}
            disabled={readOnly}
            aria-invalid={appError !== null}
            aria-describedby={appError ? "excluded-app-error" : undefined}
            onChange={(e) => setApp(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addApp()}
          />
          <Button variant="outline" disabled={readOnly || !isBundleId(app.trim())} onClick={addApp}>
            Add
          </Button>
        </div>
        {appError && (
          <p id="excluded-app-error" className="text-xs text-[var(--status-danger)]">
            {appError}
          </p>
        )}
        {missingDefaults.length > 0 && (
          <div className="flex items-center gap-2">
            <Button
              variant="ghost"
              size="sm"
              disabled={readOnly || full}
              onClick={() =>
                setRecording({ excludedApps: [...r.excludedApps, ...missingDefaults].slice(0, MAX_EXCLUDED_APPS) })
              }
            >
              Restore defaults
            </Button>
            {full && (
              <span className="text-xs text-muted-foreground">
                The list is full ({MAX_EXCLUDED_APPS} apps); remove one first.
              </span>
            )}
          </div>
        )}
      </div>
    </Section>
  );
}

export function AiSection({ api }: { api: SettingsApi }) {
  const settings = useSettings((s) => s.settings)!;
  const readOnly = useSettings((s) => s.readOnly);
  const change = useChange();
  const ai = settings.ai;
  const setAi = (patch: Partial<Settings["ai"]>) => change((s) => ({ ...s, ai: { ...s.ai, ...patch } }));
  const [stored, setStored] = useState<boolean | null>(null);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.apiKeyStored().then(setStored, (e) => setError(errorMessage(e)));
  }, [api]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      setStored(await api.apiKeyStored());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Section title="AI" intro="GuideRecorder sends step screenshots and their text to Anthropic to write guide text.">
      <Row label="Use AI">
        <Toggle label="Use AI" checked={ai.enabled} disabled={readOnly} onChange={(enabled) => setAi({ enabled })} />
      </Row>
      <Row label="Write the guide when recording stops">
        <Toggle
          label="Write the guide when recording stops"
          checked={ai.autoRun}
          disabled={readOnly || !ai.enabled}
          onChange={(autoRun) => setAi({ autoRun })}
        />
      </Row>
      <div className="flex flex-col gap-2 border-b border-border py-3">
        <span className="text-sm font-medium">Anthropic API key</span>
        <span className="text-xs text-muted-foreground">
          {stored === null
            ? "Checking the Keychain…"
            : stored
              ? "A key is stored in your Keychain. It is never shown again."
              : "No key stored. The key is kept in your Keychain."}
        </span>
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (key.trim()) void run(() => api.setApiKey(key).then(() => setKey("")));
          }}
        >
          <Input
            type="password"
            aria-label="API key"
            autoComplete="off"
            placeholder={stored ? "Replace the stored key" : "sk-ant-…"}
            value={key}
            disabled={busy}
            onChange={(e) => setKey(e.target.value)}
          />
          <Button type="submit" variant="outline" disabled={busy || !key.trim()}>
            Save key
          </Button>
          {stored && (
            <Button type="button" variant="ghost" disabled={busy} onClick={() => void run(api.deleteApiKey)}>
              Remove
            </Button>
          )}
        </form>
        <ErrorText>{error}</ErrorText>
      </div>
      <Row label="Step model" note="Writes each step.">
        <CommitInput label="Step model" value={ai.stepModel} valid={isModelId} disabled={readOnly} onCommit={(stepModel) => setAi({ stepModel })} />
      </Row>
      <Row label="Guide model" note="Writes the title, summary and sections.">
        <CommitInput label="Guide model" value={ai.guideModel} valid={isModelId} disabled={readOnly} onCommit={(guideModel) => setAi({ guideModel })} />
      </Row>
      <Row label="Language" note="A language tag such as en or de-CH. Empty: the guide's own language.">
        <CommitInput
          label="Language"
          className="w-28"
          placeholder="Guide's own"
          value={ai.language}
          valid={isLanguage}
          disabled={readOnly}
          onCommit={(language) => setAi({ language })}
        />
      </Row>
      <Row label="Audience">
        <Select
          label="Audience"
          value={ai.audience}
          options={[
            { value: "beginner", label: "Beginners" },
            { value: "expert", label: "Experts" },
          ]}
          disabled={readOnly}
          onChange={(audience) => setAi({ audience })}
        />
      </Row>
    </Section>
  );
}

export function ExportSection({ exportApi = tauriExportApi }: { exportApi?: ExportApi }) {
  const settings = useSettings((s) => s.settings)!;
  const readOnly = useSettings((s) => s.readOnly);
  const change = useChange();
  const e = settings.export;
  const setExport = (patch: Partial<Settings["export"]>) => change((s) => ({ ...s, export: { ...s.export, ...patch } }));
  const [error, setError] = useState<string | null>(null);
  const pick = async (choose: () => Promise<string | null>, apply: (path: string) => void) => {
    setError(null);
    try {
      const path = await choose();
      if (path) apply(path);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  return (
    <Section title="Export" intro="Defaults for the Export dialog.">
      <Row label="Format">
        <Select
          label="Default format"
          value={e.format}
          options={FORMATS.map((f) => ({ value: f.format, label: f.label }))}
          disabled={readOnly}
          onChange={(format) => setExport({ format })}
        />
      </Row>
      <Row label="Export folder" note="Markdown and HTML exports go into a new folder here; Word and PDF start here.">
        <PathText path={e.folder} empty="Not set" />
        <Button
          variant="outline"
          size="sm"
          disabled={readOnly}
          onClick={() => void pick(() => exportApi.chooseFolder(e.folder), (folder) => setExport({ folder }))}
        >
          Choose…
        </Button>
        {e.folder && (
          <Button variant="ghost" size="sm" disabled={readOnly} onClick={() => setExport({ folder: null })}>
            Clear
          </Button>
        )}
      </Row>
      <Row label="Logo" note="PNG or JPEG, up to 2 MB.">
        <PathText path={e.logo} empty="No logo" />
        <Button
          variant="outline"
          size="sm"
          disabled={readOnly}
          onClick={() => void pick(exportApi.chooseLogo, (logo) => setExport({ logo }))}
        >
          Choose…
        </Button>
        {e.logo && (
          <Button variant="ghost" size="sm" disabled={readOnly} onClick={() => setExport({ logo: null })}>
            Clear
          </Button>
        )}
      </Row>
      <Row label="Accent colour" note="#rgb or #rrggbb. Not used by Markdown.">
        <CommitInput
          label="Accent colour"
          className="w-32"
          placeholder="None"
          value={e.accent ?? ""}
          valid={isAccent}
          disabled={readOnly}
          onCommit={(accent) => setExport({ accent: accent.trim() || null })}
        />
      </Row>
      <ErrorText>{error}</ErrorText>
    </Section>
  );
}

const PERMISSIONS: Record<Permission, { label: string; why: string }> = {
  screen_recording: { label: "Screen Recording", why: "Takes the screenshots for each step." },
  accessibility: { label: "Accessibility", why: "Names the button or field you clicked." },
  input_monitoring: { label: "Input Monitoring", why: "Notices your clicks and typing while recording." },
};

const STATUS: Record<PermissionStatus, { label: string; color: string }> = {
  granted: { label: "Allowed", color: "var(--status-success)" },
  denied: { label: "Not allowed", color: "var(--status-danger)" },
  not_determined: { label: "Not asked yet", color: "var(--status-warning)" },
  unknown: { label: "Unknown", color: "var(--status-neutral)" },
};

type PermissionRow = { permission: Permission; status: PermissionStatus };

/**
 * The live permission checklist: status, an "Allow…" button that asks
 * macOS (only on a click), and a link to the System Settings pane. It
 * checks again when the window regains focus (coming back from System
 * Settings) and, with `poll`, every couple of seconds.
 */
export function PermissionList({
  api,
  poll = false,
  onRows,
}: {
  api: SettingsApi;
  poll?: boolean;
  onRows?(rows: PermissionRow[]): void;
}) {
  const [rows, setRows] = useState<PermissionRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(() => {
    api.permissions().then(
      (r) => {
        setRows(r);
        onRows?.(r);
        setError(null);
      },
      (e) => setError(errorMessage(e)),
    );
  }, [api, onRows]);
  useEffect(() => {
    refresh();
    window.addEventListener("focus", refresh);
    const timer = poll ? setInterval(refresh, 2000) : undefined;
    return () => {
      window.removeEventListener("focus", refresh);
      clearInterval(timer);
    };
  }, [refresh, poll]);

  const ask = async (permission: Permission) => {
    try {
      await api.requestPermission(permission);
    } catch (e) {
      setError(errorMessage(e));
    }
    refresh();
  };

  return (
    <div className="flex flex-col">
      {rows?.map(({ permission, status }) => (
        <Row key={permission} label={PERMISSIONS[permission].label} note={PERMISSIONS[permission].why}>
          <span className="text-xs font-medium" style={{ color: STATUS[status].color }}>
            {STATUS[status].label}
          </span>
          {status !== "granted" && (
            <>
              <Button variant="outline" size="sm" onClick={() => void ask(permission)}>
                Allow…
              </Button>
              <Button
                variant="ghost"
                size="sm"
                aria-label={`Open System Settings for ${PERMISSIONS[permission].label}`}
                onClick={() => api.openPermissionSettings(permission).catch((e) => setError(errorMessage(e)))}
              >
                <ExternalLink /> System Settings
              </Button>
            </>
          )}
        </Row>
      ))}
      <div className="pt-3">
        <Button variant="ghost" size="sm" onClick={refresh}>
          <RefreshCw /> Check again
        </Button>
      </div>
      <ErrorText>{error}</ErrorText>
    </div>
  );
}

export function PermissionsSection({ api }: { api: SettingsApi }) {
  return (
    <Section
      title="Permissions"
      intro="Recording needs all of these and stays off until they are allowed. Editing and exporting work without them. Some changes take effect only after GuideRecorder restarts."
    >
      <PermissionList api={api} />
    </Section>
  );
}

/** Copies text, falling back to a hidden textarea where the Clipboard API is refused. */
async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    if (!ok) throw new Error("copying is not allowed here");
  }
}

export function McpSection({ api }: { api: SettingsApi }) {
  const [setup, setSetup] = useState<McpSetup | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.mcpSetup().then(setSetup, (e) => setError(errorMessage(e)));
  }, [api]);

  return (
    <Section
      title="MCP"
      intro="Let Claude Code read and search your guides: run this command once in a terminal."
    >
      {setup && (
        <div className="mt-2 flex flex-col gap-2">
          <pre className="overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs whitespace-pre-wrap break-all select-all">
            {setup.command}
          </pre>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() =>
                copyText(setup.command).then(
                  () => {
                    setCopied(true);
                    setTimeout(() => setCopied(false), 1500);
                  },
                  (e) => setError(errorMessage(e)),
                )
              }
            >
              {copied ? <Check /> : <Copy />} {copied ? "Copied" : "Copy command"}
            </Button>
          </div>
          {!setup.found && (
            <p className="text-xs text-[var(--status-warning)]">
              guiderecorder-mcp was not found next to the app ({setup.mcpPath}). In a development build, run
              <code className="mx-1 font-mono">cargo build -p guiderecorder-mcp</code> first.
            </p>
          )}
        </div>
      )}
      <ErrorText>{error}</ErrorText>
    </Section>
  );
}
