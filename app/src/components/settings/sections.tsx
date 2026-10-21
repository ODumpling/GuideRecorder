import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Copy, ExternalLink, RotateCw, X } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { CommitInput, HotkeyField, PathText, Row, Section, Select, Toggle } from "@/components/settings/controls";
import { useEditor } from "@/editor/editor";
import { allGranted, setupStore } from "@/onboarding/onboarding";
import { switchLibrary } from "@/settings/library";
import { FORMATS } from "@/export/export";
import { errorMessage, tauriExportApi, tauriUpdaterApi, type ExportApi, type SettingsApi } from "@/lib/ipc";
import type { AiEndpoint, AiProviderKind, CaptureArea, Density, HotkeyErrors, LibraryInfo, McpSetup, Permission, PermissionRow, PermissionStatus, RedactionMode, Settings, Theme, UpdateStatus } from "@/lib/types";
import { useSettings, settingsStore } from "@/settings/settings";
import { adjustAccent, normalHex, systemIsDark, themeBackground, themeFor, THEMES } from "@/settings/theme";
import { cn } from "@guiderecorder/ui/utils";
import {
  AFTER_FRAME_DELAY_MS,
  bundleIdError,
  inRange,
  isBaseUrl,
  isBundleId,
  isHotkey,
  isLanguage,
  isModelId,
  KEEP_DAYS,
  KEEP_VERSIONS,
  MAX_EXCLUDED_APPS,
  MAX_REDACTION_PATTERNS,
  MAX_REDACTION_WORDS,
  TYPING_IDLE_MS,
} from "@/settings/validate";
import { isAccent } from "@/export/export";
import { appName, DEFAULT_EXCLUDED_IDS } from "@/recording/apps";
import { isWindows, keyStore, thisComputer } from "@/lib/platform";
import { progressText } from "@/updater/UpdateBanner";
import { updaterStore, useUpdater } from "@/updater/updater";

type Change = (update: (s: Settings) => Settings) => void;

function useChange(): Change {
  return useCallback((update) => void settingsStore.getState().change(update), []);
}

function ErrorText({ children }: { children: string | null }) {
  if (!children) return null;
  return <p className="mt-2 text-xs text-[var(--status-danger)]">{children}</p>;
}

/** The updater's state in a few words. */
function updateText(status: UpdateStatus): string | null {
  switch (status.state) {
    case "checking":
      return "Checking…";
    case "upToDate":
      return "Up to date.";
    case "available":
      return `Version ${status.version} is available.`;
    case "downloading":
      return `Downloading ${status.version}… ${progressText(status.downloaded, status.total)}`;
    case "ready":
      return `Version ${status.version} is installed; restart to use it.`;
    case "failed":
      return status.message;
    default:
      return null;
  }
}

function UpdatesRow() {
  const settings = useSettings((s) => s.settings)!;
  const readOnly = useSettings((s) => s.readOnly);
  const change = useChange();
  const status = useUpdater((s) => s.status);
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    tauriUpdaterApi.version().then(setVersion, () => setVersion(null));
  }, []);
  const { check, install, restart } = updaterStore.getState();
  const text = updateText(status);
  return (
    <Row
      label="Updates"
      note={
        <>
          {version && `You have version ${version}. `}
          {text ?? "GuideRecorder checks for a new version at startup and every few hours."}
        </>
      }
    >
      <Toggle
        label="Check for updates automatically"
        checked={settings.general.checkForUpdates}
        disabled={readOnly}
        onChange={(on) => change((s) => ({ ...s, general: { ...s.general, checkForUpdates: on } }))}
      />
      {status.state === "available" ? (
        <Button variant="outline" size="sm" onClick={() => void install()}>
          Install update
        </Button>
      ) : status.state === "ready" ? (
        <Button variant="outline" size="sm" onClick={() => void restart()}>
          Restart now
        </Button>
      ) : (
        <Button
          variant="outline"
          size="sm"
          disabled={status.state === "checking" || status.state === "downloading"}
          onClick={() => void check()}
        >
          Check now
        </Button>
      )}
    </Row>
  );
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
      <Row
        label="First-run setup"
        note={isWindows() ? "The library folder and the API key, step by step." : "Permissions, the library folder and the API key, step by step."}
      >
        <Button variant="outline" size="sm" onClick={() => setupStore.getState().open()}>
          Run setup again
        </Button>
      </Row>
      <UpdatesRow />
      <Row
        label="Version history"
        note="Versions each guide keeps (0: no limit). Older ones are deleted when a guide closes; named versions and the newest are always kept."
      >
        <CommitInput
          label="Versions to keep"
          type="number"
          className="w-20"
          value={String(settings.history.keepVersions)}
          valid={(v) => v !== "" && inRange(Number(v), KEEP_VERSIONS)}
          disabled={readOnly}
          onCommit={(v) => change((s) => ({ ...s, history: { ...s.history, keepVersions: Number(v) } }))}
        />
        <span className="text-xs text-muted-foreground">versions, up to</span>
        <CommitInput
          label="Days to keep versions"
          type="number"
          className="w-20"
          value={String(settings.history.keepDays)}
          valid={(v) => v !== "" && inRange(Number(v), KEEP_DAYS)}
          disabled={readOnly}
          onCommit={(v) => change((s) => ({ ...s, history: { ...s.history, keepDays: Number(v) } }))}
        />
        <span className="text-xs text-muted-foreground">days</span>
      </Row>
      <Row label={isWindows() ? "Tray icon" : "Menu bar icon"} note="Coming with recording; stored for now.">
        <Toggle
          label={isWindows() ? "Tray icon" : "Menu bar icon"}
          checked={settings.general.menuBarIcon}
          disabled={readOnly}
          onChange={(on) => change((s) => ({ ...s, general: { ...s.general, menuBarIcon: on } }))}
        />
      </Row>
      <ErrorText>{error}</ErrorText>
    </Section>
  );
}

const DENSITIES: { value: Density; label: string }[] = [
  { value: "comfortable", label: "Comfortable" },
  { value: "compact", label: "Compact" },
];

/** The colour theme, Match system, the accent, density and effects. */
export function AppearanceSection() {
  const general = useSettings((s) => s.settings!.general);
  const readOnly = useSettings((s) => s.readOnly);
  const change = useChange();
  const set = (patch: Partial<Settings["general"]>) => change((s) => ({ ...s, general: { ...s.general, ...patch } }));
  // The accent is checked against each background it can show on.
  const shownThemes = general.matchSystem ? [general.lightTheme, general.darkTheme] : [general.theme];
  const adjusted = general.accent
    ? shownThemes.flatMap((theme) => {
        const { color, adjusted } = adjustAccent(general.accent!, themeBackground(theme));
        return adjusted ? [{ theme, color }] : [];
      })
    : [];
  return (
    <Section title="Appearance" intro="The colours of GuideRecorder's windows. Exported guides keep their own look.">
      <Row label="Match system" note="Switch between a light and a dark theme with the system's appearance.">
        <Toggle label="Match system" checked={general.matchSystem} disabled={readOnly} onChange={(on) => set({ matchSystem: on })} />
      </Row>
      {general.matchSystem ? (
        <>
          <h3 className="mt-3 text-sm font-medium">Light appearance</h3>
          <ThemeCards
            label="Light theme"
            themes={THEMES.filter((t) => !t.dark)}
            value={general.lightTheme}
            disabled={readOnly}
            onChange={(lightTheme) => set({ lightTheme })}
          />
          <h3 className="mt-4 text-sm font-medium">Dark appearance</h3>
          <ThemeCards
            label="Dark theme"
            themes={THEMES.filter((t) => t.dark)}
            value={general.darkTheme}
            disabled={readOnly}
            onChange={(darkTheme) => set({ darkTheme })}
          />
        </>
      ) : (
        <ThemeCards label="Theme" themes={THEMES} value={general.theme} disabled={readOnly} onChange={(theme) => set({ theme })} />
      )}
      <div className="mt-4 flex flex-col">
        <Row
          label="Accent colour"
          note={
            adjusted.length > 0 ? (
              <span className="text-[var(--status-warning)]">
                Too close to the background to read well; shown as{" "}
                {adjusted.map(({ theme, color }) => `${color} in ${THEMES.find((t) => t.value === theme)!.label}`).join(" and ")}.
              </span>
            ) : (
              "Main buttons, the outline selection and annotation handles. Empty: the theme's own."
            )
          }
        >
          <ColourPicker
            label="Pick an accent colour"
            value={(general.accent && normalHex(general.accent)) || themeAccent(themeFor(general, systemIsDark()))}
            disabled={readOnly}
            onChange={(accent) => set({ accent })}
          />
          <CommitInput
            label="Accent colour"
            className="w-28"
            placeholder="Theme's"
            value={general.accent ?? ""}
            valid={isAccent}
            disabled={readOnly}
            onCommit={(accent) => set({ accent: accent.trim() || null })}
          />
          {general.accent && (
            <Button variant="ghost" size="sm" disabled={readOnly} onClick={() => set({ accent: null })}>
              Clear
            </Button>
          )}
        </Row>
        <Row label="Density" note="Outline row height and the size of the library's cards.">
          <Select label="Density" value={general.density} options={DENSITIES} disabled={readOnly} onChange={(density) => set({ density })} />
        </Row>
        <Row
          label="Reduce transparency and motion"
          note="No glow, backdrop blur or card hover animation. Also on when the system asks for less motion or transparency."
        >
          <Toggle
            label="Reduce transparency and motion"
            checked={general.reduceEffects}
            disabled={readOnly}
            onChange={(on) => set({ reduceEffects: on })}
          />
        </Row>
      </div>
    </Section>
  );
}

/**
 * The system colour picker. Changes once the picker closes (the native
 * `change` event), not on every colour passed while dragging.
 */
function ColourPicker({
  label,
  value,
  disabled,
  onChange,
}: {
  label: string;
  value: string;
  disabled: boolean;
  onChange(value: string): void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const latest = useRef(onChange);
  latest.current = onChange;
  useEffect(() => {
    const el = input.current;
    if (!el) return;
    const changed = () => latest.current(el.value);
    el.addEventListener("change", changed);
    return () => el.removeEventListener("change", changed);
  }, []);
  useEffect(() => {
    if (input.current) input.current.value = value;
  }, [value]);
  return (
    <input
      ref={input}
      type="color"
      aria-label={label}
      disabled={disabled}
      defaultValue={value}
      className="h-8 w-10 cursor-pointer rounded-md border border-input bg-transparent p-0.5 disabled:opacity-50"
    />
  );
}

/** The theme's own accent, for the colour picker's starting point. */
function themeAccent(theme: Theme): string {
  return (THEMES.find((t) => t.value === theme) ?? THEMES[0]).swatch[2];
}

/** A card per theme, with a small picture of it. */
function ThemeCards({
  label,
  themes,
  value: theme,
  disabled,
  onChange,
}: {
  label: string;
  themes: typeof THEMES;
  value: Theme;
  disabled: boolean;
  onChange(theme: Theme): void;
}) {
  return (
    <div role="radiogroup" aria-label={label} className="mt-2 grid grid-cols-3 gap-3">
      {themes.map(({ value, label, swatch: [background, sidebar, accent] }) => (
        <button
          key={value}
          type="button"
          role="radio"
          aria-checked={theme === value}
          disabled={disabled}
          onClick={() => onChange(value)}
          className={cn(
            "flex flex-col gap-2 rounded-lg border p-2 text-left text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50",
            theme === value ? "border-[var(--status-info)] ring-1 ring-[var(--status-info)]" : "border-border hover:bg-accent/60",
          )}
        >
          <span aria-hidden className="flex h-16 overflow-hidden rounded-md border border-black/10" style={{ background }}>
            <span className="w-1/4" style={{ background: sidebar }} />
            <span className="flex flex-1 flex-col justify-end gap-1.5 p-2">
              <span className="h-1.5 w-3/4 rounded-full opacity-25" style={{ background: accent }} />
              <span className="h-1.5 w-1/2 rounded-full opacity-25" style={{ background: accent }} />
              <span className="h-3 w-8 rounded" style={{ background: accent }} />
            </span>
          </span>
          <span className="flex items-center justify-between gap-1 px-0.5">
            <span className="font-medium">{label}</span>
            {theme === value && <Check className="size-4 text-[var(--status-info)]" />}
          </span>
        </button>
      ))}
    </div>
  );
}

const CAPTURE_AREAS: { value: CaptureArea; label: string }[] = [
  { value: "display", label: "All displays" },
  { value: "oneDisplay", label: "One display" },
  { value: "window", label: "Application" },
  { value: "region", label: "A region" },
];

/** A shortcut that could not be registered, in a row's note. */
function HotkeyError({ children }: { children: string }) {
  return <span className="text-[var(--status-danger)]">{children}</span>;
}

export function RecordingSection({ api }: { api: SettingsApi }) {
  const settings = useSettings((s) => s.settings)!;
  const readOnly = useSettings((s) => s.readOnly);
  const change = useChange();
  const r = settings.recording;
  const [app, setApp] = useState("");
  const [hotkeyErrors, setHotkeyErrors] = useState<HotkeyErrors>({ record: null, pause: null });
  const refreshHotkeyErrors = useCallback(() => {
    api.hotkeyErrors().then(setHotkeyErrors, (e) => setHotkeyErrors({ record: errorMessage(e), pause: null }));
  }, [api]);
  useEffect(refreshHotkeyErrors, [refreshHotkeyErrors]);
  const setRecording = (patch: Partial<Settings["recording"]>) =>
    change((s) => ({ ...s, recording: { ...s.recording, ...patch } }));
  const setNarration = (patch: Partial<Settings["recording"]["narration"]>) =>
    change((s) => ({ ...s, recording: { ...s.recording, narration: { ...s.recording.narration, ...patch } } }));
  // Record and pause are registered as the change is saved: check them after.
  const setHotkey = (name: keyof Settings["recording"]["hotkeys"], value: string) => {
    if (!isHotkey(value)) return;
    const hotkeys = (s: Settings) => ({ ...s.recording.hotkeys, [name]: value });
    void settingsStore
      .getState()
      .change((s) => ({ ...s, recording: { ...s.recording, hotkeys: hotkeys(s) } }))
      .then(refreshHotkeyErrors);
  };
  const addApp = () => {
    // Executable names are kept in lower case on Windows, as the backend reports them.
    const id = isWindows() ? app.trim().toLowerCase() : app.trim();
    if (!isBundleId(id) || r.excludedApps.includes(id) || r.excludedApps.length >= MAX_EXCLUDED_APPS) return;
    setRecording({ excludedApps: [...r.excludedApps, id] });
    setApp("");
  };
  // Adds back the suggested apps that were removed; keeps the user's own.
  const missingDefaults = DEFAULT_EXCLUDED_IDS.filter((id) => !r.excludedApps.includes(id));
  const full = r.excludedApps.length >= MAX_EXCLUDED_APPS;
  const appError = bundleIdError(app.trim());

  return (
    <Section title="Recording" intro="The next recording uses these. With One display, Application or Region as the capture area, you pick the display or window, or draw the region, each time you record. The record and pause shortcuts work from any app and change right away.">
      <Row label="Start or stop recording" note={hotkeyErrors.record && <HotkeyError>{hotkeyErrors.record}</HotkeyError>}>
        <HotkeyField label="Record shortcut" value={r.hotkeys.record} disabled={readOnly} onChange={(v) => setHotkey("record", v)} />
      </Row>
      <Row label="Pause recording" note={hotkeyErrors.pause && <HotkeyError>{hotkeyErrors.pause}</HotkeyError>}>
        <HotkeyField label="Pause shortcut" value={r.hotkeys.pause} disabled={readOnly} onChange={(v) => setHotkey("pause", v)} />
      </Row>
      <Row label="Add a manual step" note="Works only while recording; the next recording uses a change.">
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
      <Row label="Click ring" note="Briefly show a ring on screen where each click was captured.">
        <Toggle
          label="Click ring"
          checked={r.clickRing}
          disabled={readOnly}
          onChange={(clickRing) => setRecording({ clickRing })}
        />
      </Row>
      {narrationSupported() && (
        <>
          <Row
            label="Narrate"
            note={`Start recording with the microphone on, and say what you're doing. It is transcribed on ${thisComputer()} and becomes the steps' descriptions. The toolbar's microphone turns it on and off.`}
          >
            <Toggle
              label="Narrate"
              checked={r.narration.enabled}
              disabled={readOnly}
              onChange={(enabled) => setNarration({ enabled })}
            />
          </Row>
          <Row
            label="Keep narration audio"
            note={`Save the audio in the guide's folder too (narration.m4a). It stays on ${thisComputer()} and is never synced.`}
          >
            <Toggle
              label="Keep narration audio"
              checked={r.narration.keepAudio}
              disabled={readOnly}
              onChange={(keepAudio) => setNarration({ keepAudio })}
            />
          </Row>
        </>
      )}
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
          Nothing is recorded while one of these apps is in front.{" "}
          {isWindows()
            ? "Use app executables, such as 1password.exe."
            : "Use bundle ids, such as com.1password.1password."}
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
            aria-label={isWindows() ? "App executable" : "App bundle id"}
            placeholder={isWindows() ? "example.exe" : "com.example.app"}
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

const REDACTION_MODES: { value: RedactionMode; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "suggest", label: "Suggest blurs" },
  { value: "apply", label: "Blur automatically" },
];

const REDACTION_NOTES: Record<RedactionMode, string> = {
  off: "Screenshots are not scanned.",
  suggest: "Each finding becomes a suggested blur to accept or reject in the editor.",
  apply: "Each finding is blurred at once; delete a blur in the editor to show the text again.",
};

/**
 * A list of strings to add to and remove from, saved as each change is
 * made. Rust checks each entry; a refused one stays in the field and the
 * reason shows at the top of Settings.
 */
function RuleList({
  title,
  note,
  label,
  placeholder,
  items,
  max,
  mono,
  disabled,
  onChange,
}: {
  title: string;
  note: string;
  label: string;
  placeholder: string;
  items: string[];
  max: number;
  mono?: boolean;
  disabled: boolean;
  onChange(items: string[]): Promise<boolean>;
}) {
  const [draft, setDraft] = useState("");
  const value = draft.trim();
  const full = items.length >= max;
  const canAdd = !disabled && !full && value !== "" && !items.includes(value);
  const add = async () => {
    if (canAdd && (await onChange([...items, value]))) setDraft("");
  };
  return (
    <div className="flex flex-col gap-2 border-b border-border py-3 last:border-b-0">
      <span className="text-sm font-medium">{title}</span>
      <span className="text-xs text-muted-foreground">{note}</span>
      {items.length > 0 && (
        <ul className="flex flex-col gap-1">
          {items.map((item) => (
            <li key={item} className="flex items-center justify-between rounded-md bg-muted px-2 py-1 text-sm">
              <span className={cn("min-w-0 truncate", mono && "font-mono text-xs")}>{item}</span>
              <Button
                variant="ghost"
                size="icon"
                aria-label={`Remove ${item}`}
                disabled={disabled}
                onClick={() => void onChange(items.filter((i) => i !== item))}
              >
                <X />
              </Button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex gap-2">
        <Input
          aria-label={label}
          placeholder={placeholder}
          value={draft}
          disabled={disabled}
          className={cn(mono && "font-mono")}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void add()}
        />
        <Button variant="outline" disabled={!canAdd} onClick={() => void add()}>
          Add
        </Button>
      </div>
      {full && <span className="text-xs text-muted-foreground">The list is full ({max}); remove one first.</span>}
    </div>
  );
}

export function PrivacySection() {
  const settings = useSettings((s) => s.settings)!;
  const readOnly = useSettings((s) => s.readOnly);
  const r = settings.redaction;
  const setRedaction = (patch: Partial<Settings["redaction"]>) =>
    settingsStore.getState().change((s) => ({ ...s, redaction: { ...s.redaction, ...patch } }));
  return (
    <Section
      title="Privacy"
      intro={`When a recording stops, GuideRecorder reads the text in its screenshots on ${thisComputer()} and looks for email addresses, phone numbers, card numbers, IBANs, IP addresses, API keys and tokens, and long numbers. Nothing is uploaded and no API key is needed. Before you export, you are asked to review any suggestions left.`}
    >
      <Row label="Sensitive text" note={REDACTION_NOTES[r.mode]}>
        <Select
          label="Sensitive text"
          value={r.mode}
          options={REDACTION_MODES}
          disabled={readOnly}
          onChange={(mode) => void setRedaction({ mode })}
        />
      </Row>
      <RuleList
        title="Words to blur"
        note="Names, customers, internal host names: blurred wherever they appear as whole words, ignoring case."
        label="Word to blur"
        placeholder="Acme Corp"
        items={r.words}
        max={MAX_REDACTION_WORDS}
        disabled={readOnly || r.mode === "off"}
        onChange={(words) => setRedaction({ words })}
      />
      <RuleList
        title="Patterns to blur"
        note="Regular expressions, such as INC-\d{6} or (?i)\w+\.corp\.example\.com; (?i) ignores case."
        label="Pattern to blur"
        placeholder="INC-\d{6}"
        items={r.patterns}
        max={MAX_REDACTION_PATTERNS}
        mono
        disabled={readOnly || r.mode === "off"}
        onChange={(patterns) => setRedaction({ patterns })}
      />
    </Section>
  );
}

const PROVIDERS: { value: AiProviderKind; label: string }[] = [
  { value: "anthropic", label: "Anthropic" },
  { value: "openAiCompatible", label: "OpenAI-compatible API" },
  { value: "ollama", label: `Ollama (on ${thisComputer()})` },
  { value: "lmStudio", label: `LM Studio (on ${thisComputer()})` },
];

/** The local presets' servers, used when the base URL is empty (see `OpenAiConfig` in core). */
const LOCAL_BASE_URLS: Partial<Record<AiProviderKind, string>> = {
  ollama: "http://localhost:11434/v1",
  lmStudio: "http://localhost:1234/v1",
};

type EndpointProvider = "openAiCompatible" | "ollama" | "lmStudio";

function providerIntro(provider: AiProviderKind): string {
  switch (provider) {
    case "openAiCompatible":
      return "GuideRecorder sends step text, and screenshots if the model takes images, to the API below to write guide text.";
    case "ollama":
    case "lmStudio":
      return `GuideRecorder writes guide text with ${provider === "ollama" ? "Ollama" : "LM Studio"} on ${thisComputer()}. Nothing leaves your ${isWindows() ? "PC" : "Mac"}.`;
    default:
      return "GuideRecorder sends step screenshots and their text to Anthropic to write guide text.";
  }
}

/** A provider's API key: stored in the Keychain (Credential Manager on Windows), never shown back. */
function ApiKeyField({
  api,
  provider,
  title,
  placeholder,
  optional,
}: {
  api: SettingsApi;
  provider: AiProviderKind;
  title: string;
  placeholder: string;
  optional?: boolean;
}) {
  const [stored, setStored] = useState<boolean | null>(null);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.apiKeyStored(provider).then(setStored, (e) => setError(errorMessage(e)));
  }, [api, provider]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      setStored(await api.apiKeyStored(provider));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-2 border-b border-border py-3">
      <span className="text-sm font-medium">{title}</span>
      <span className="text-xs text-muted-foreground">
        {stored === null
          ? isWindows()
            ? "Checking for a stored key…"
            : "Checking the Keychain…"
          : stored
            ? `A key is stored in ${keyStore()}. It is never shown again.`
            : optional
              ? `No key stored; only needed if the API asks for one. The key is kept in ${keyStore()}.`
              : `No key stored. The key is kept in ${keyStore()}.`}
      </span>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (key.trim()) void run(() => api.setApiKey(key, provider).then(() => setKey("")));
        }}
      >
        <Input
          type="password"
          aria-label="API key"
          autoComplete="off"
          placeholder={stored ? "Replace the stored key" : placeholder}
          value={key}
          disabled={busy}
          onChange={(e) => setKey(e.target.value)}
        />
        <Button type="submit" variant="outline" disabled={busy || !key.trim()}>
          Save key
        </Button>
        {stored && (
          <Button type="button" variant="ghost" disabled={busy} onClick={() => void run(() => api.deleteApiKey(provider))}>
            Remove
          </Button>
        )}
      </form>
      <ErrorText>{error}</ErrorText>
    </div>
  );
}

const isOptionalModelId = (value: string) => value === "" || isModelId(value);

export function AiSection({ api }: { api: SettingsApi }) {
  const settings = useSettings((s) => s.settings)!;
  const readOnly = useSettings((s) => s.readOnly);
  const change = useChange();
  const ai = settings.ai;
  const setAi = (patch: Partial<Settings["ai"]>) => change((s) => ({ ...s, ai: { ...s.ai, ...patch } }));
  const provider = ai.provider;
  const endpoint = provider === "anthropic" ? null : ai[provider];
  const setEndpoint = (name: EndpointProvider, patch: Partial<AiEndpoint>) =>
    change((s) => ({ ...s, ai: { ...s.ai, [name]: { ...s.ai[name], ...patch } } }));

  return (
    <Section title="AI" intro={providerIntro(provider)}>
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
      <Row label="Provider" note={`Ollama and LM Studio run models on ${thisComputer()}.`}>
        <Select
          label="Provider"
          value={provider}
          options={PROVIDERS}
          disabled={readOnly}
          onChange={(provider) => setAi({ provider })}
        />
      </Row>
      {provider === "anthropic" && (
        <>
          <ApiKeyField key={provider} api={api} provider={provider} title="Anthropic API key" placeholder="sk-ant-…" />
          <Row label="Step model" note="Writes each step.">
            <CommitInput label="Step model" value={ai.stepModel} valid={isModelId} disabled={readOnly} onCommit={(stepModel) => setAi({ stepModel })} />
          </Row>
          <Row label="Guide model" note="Writes the title, summary and sections.">
            <CommitInput label="Guide model" value={ai.guideModel} valid={isModelId} disabled={readOnly} onCommit={(guideModel) => setAi({ guideModel })} />
          </Row>
        </>
      )}
      {endpoint && provider !== "anthropic" && (
        <>
          <Row
            label="Base URL"
            note={
              provider === "openAiCompatible"
                ? "Up to the version, such as https://api.openai.com/v1."
                : `Empty: ${LOCAL_BASE_URLS[provider]}.`
            }
          >
            <CommitInput
              label="Base URL"
              className="w-64"
              placeholder={LOCAL_BASE_URLS[provider] ?? "https://api.example.com/v1"}
              value={endpoint.baseUrl}
              valid={isBaseUrl}
              disabled={readOnly}
              onCommit={(baseUrl) => setEndpoint(provider, { baseUrl })}
            />
          </Row>
          {provider === "openAiCompatible" && (
            <ApiKeyField key={provider} api={api} provider={provider} title="API key" placeholder="sk-…" optional />
          )}
          <Row label="Step model" note="Writes each step. The model must support tool calls.">
            <CommitInput
              label="Step model"
              placeholder="Model id"
              value={endpoint.stepModel}
              valid={isOptionalModelId}
              disabled={readOnly}
              onCommit={(stepModel) => setEndpoint(provider, { stepModel })}
            />
          </Row>
          <Row label="Guide model" note="Writes the title, summary and sections. Can be the same model.">
            <CommitInput
              label="Guide model"
              placeholder="Model id"
              value={endpoint.guideModel}
              valid={isOptionalModelId}
              disabled={readOnly}
              onCommit={(guideModel) => setEndpoint(provider, { guideModel })}
            />
          </Row>
          <Row
            label="Send screenshots"
            note="Turn off for a model without image input: steps are then written from the clicked element, keystrokes and step kind, with no redaction suggestions."
          >
            <Toggle
              label="Send screenshots"
              checked={endpoint.images}
              disabled={readOnly}
              onChange={(images) => setEndpoint(provider, { images })}
            />
          </Row>
        </>
      )}
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
  microphone: { label: "Microphone", why: "Hears you narrate, only while the toolbar's microphone is on." },
  speech_recognition: {
    label: "Speech Recognition",
    why: "Turns what you say into text, on this Mac: the audio is never sent anywhere.",
  },
};

/** Narration works on macOS only (the Speech framework, on the device). */
export function narrationSupported(): boolean {
  return !isWindows();
}

const STATUS: Record<PermissionStatus, { label: string; color: string }> = {
  granted: { label: "Allowed", color: "var(--status-success)" },
  denied: { label: "Not allowed", color: "var(--status-danger)" },
  not_determined: { label: "Not asked yet", color: "var(--status-warning)" },
  unknown: { label: "Unknown", color: "var(--status-neutral)" },
};

/** How often the checklist checks again while it is on screen. */
const PERMISSION_POLL_MS = 1000;

/** Permissions "Turn on…" has asked macOS for (on this Mac), so the next click opens System Settings. */
const ASKED_KEY = "guiderecorder.permissionsAsked";

function readAsked(): Permission[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(ASKED_KEY) ?? "[]");
    return Array.isArray(value) ? (value.filter((p) => typeof p === "string") as Permission[]) : [];
  } catch {
    return [];
  }
}

/** Where to turn a permission on, once macOS has been asked. */
function howTo(permission: Permission): string {
  return `In System Settings → Privacy & Security → ${PERMISSIONS[permission].label}, turn on GuideRecorder. Not in the list? Click + and choose GuideRecorder from Applications.`;
}

/**
 * The live permission checklist. It checks every second while on screen
 * (and at once when the window comes back to the front), so a switch
 * flipped in System Settings shows up right away. Each missing permission
 * has one button: the first time it asks macOS (which adds GuideRecorder
 * to the list in System Settings, and may show its own prompt); after
 * that it opens the right System Settings pane. A Screen Recording grant
 * that macOS applies only at launch offers a restart. With `narration`, it
 * lists the optional ones narration needs instead.
 */
export function PermissionList({
  api,
  onRows,
  narration = false,
}: {
  api: SettingsApi;
  onRows?(rows: PermissionRow[]): void;
  narration?: boolean;
}) {
  const [rows, setRows] = useState<PermissionRow[] | null>(null);
  const [asked, setAsked] = useState<Permission[]>(readAsked);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(() => {
    (narration ? api.narrationPermissions() : api.permissions()).then(
      (r) => {
        setRows(r);
        onRows?.(r);
        setError(null);
      },
      (e) => setError(errorMessage(e)),
    );
  }, [api, onRows, narration]);
  useEffect(() => {
    refresh();
    const onVisible = () => document.visibilityState === "visible" && refresh();
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", onVisible);
    const timer = setInterval(() => document.visibilityState === "visible" && refresh(), PERMISSION_POLL_MS);
    return () => {
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", onVisible);
      clearInterval(timer);
    };
  }, [refresh]);

  const openSettings = (permission: Permission) =>
    api.openPermissionSettings(permission).catch((e) => setError(errorMessage(e)));

  const turnOn = async (permission: Permission) => {
    setError(null);
    if (asked.includes(permission)) {
      await openSettings(permission);
      return;
    }
    const next = [...asked, permission];
    setAsked(next);
    localStorage.setItem(ASKED_KEY, JSON.stringify(next));
    try {
      await api.requestPermission(permission);
    } catch (e) {
      setError(errorMessage(e));
    }
    refresh();
  };

  return (
    <div className="flex flex-col">
      {rows?.map(({ permission, status, restartNeeded }) => {
        const { label, why } = PERMISSIONS[permission];
        const granted = status === "granted";
        const wasAsked = asked.includes(permission);
        return (
          <Row key={permission} label={label} note={granted || restartNeeded || !wasAsked ? why : howTo(permission)}>
            {restartNeeded ? (
              <>
                <span className="text-xs font-medium text-[var(--status-success)]">Allowed after a restart</span>
                <Button variant="outline" size="sm" onClick={() => api.restartApp().catch((e) => setError(errorMessage(e)))}>
                  <RotateCw /> Restart GuideRecorder
                </Button>
              </>
            ) : (
              <>
                <span className="flex items-center gap-1 text-xs font-medium" style={{ color: STATUS[status].color }}>
                  {granted && <Check className="size-3.5" />}
                  {STATUS[status].label}
                </span>
                {!granted && (
                  <Button variant={wasAsked ? "outline" : "default"} size="sm" onClick={() => void turnOn(permission)}>
                    {wasAsked ? (
                      <>
                        <ExternalLink /> Open System Settings
                      </>
                    ) : (
                      "Turn on…"
                    )}
                  </Button>
                )}
              </>
            )}
          </Row>
        );
      })}
      <ErrorText>{error}</ErrorText>
    </div>
  );
}

export function PermissionsSection({ api }: { api: SettingsApi }) {
  const [rows, setRows] = useState<PermissionRow[] | null>(null);
  const restartNeeded = rows?.some((r) => r.restartNeeded) ?? false;
  return (
    <Section
      title="Permissions"
      intro="Recording needs all of these and stays off until they are allowed. Editing and exporting work without them. This list updates as soon as you change a switch in System Settings."
    >
      <PermissionList api={api} onRows={setRows} />
      {rows && allGranted(rows) && (
        <p className="mt-3 flex items-center gap-2 text-xs text-[var(--status-success)]">
          <Check className="size-3.5" /> All set: recording is available.
        </p>
      )}
      {restartNeeded && (
        <p className="mt-3 text-xs text-muted-foreground">
          macOS applies Screen Recording when GuideRecorder starts. Restart saves the open guide first.
        </p>
      )}
      {narrationSupported() && <NarrationPermissions api={api} />}
    </Section>
  );
}

/** The optional permissions narration needs, under their own heading. */
export function NarrationPermissions({ api }: { api: SettingsApi }) {
  return (
    <div className="mt-4 flex flex-col gap-1">
      <span className="text-sm font-medium">Narration (optional)</span>
      <span className="text-xs text-muted-foreground">
        To narrate while you record, allow these too. Speech is transcribed on this Mac; the audio never leaves it.
        Recording works without them.
      </span>
      <PermissionList api={api} narration />
    </div>
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
  const writable = useSettings((s) => s.settings!.mcp.writable);
  const readOnly = useSettings((s) => s.readOnly);
  const refresh = useCallback(() => {
    api.mcpSetup().then(setSetup, (e) => setError(errorMessage(e)));
  }, [api]);
  useEffect(refresh, [refresh]);

  /** The command comes from the saved setting, so it is fetched again once stored. */
  const allowEdits = async (on: boolean) => {
    if (await settingsStore.getState().change((s) => ({ ...s, mcp: { ...s.mcp, writable: on } }))) refresh();
  };

  return (
    <Section
      title="MCP"
      intro="Let Claude Code read and search your guides: run this command once in a terminal."
    >
      <Row
        label="Allow MCP to edit guides"
        note="Adds --writable: Claude Code can then change guide text, tags and order, and create annotation guides from images. Guides open in the editor are never changed. If you added the server before, run claude mcp remove guiderecorder first."
      >
        <Toggle
          label="Allow MCP to edit guides"
          checked={writable}
          disabled={readOnly}
          onChange={(on) => void allowEdits(on)}
        />
      </Row>
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
