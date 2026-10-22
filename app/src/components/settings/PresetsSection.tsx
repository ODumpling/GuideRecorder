import { useCallback, useEffect, useState, type ReactNode } from "react";
import { ImageIcon, Plus } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { Textarea } from "@guiderecorder/ui/textarea";
import { Row, Section, Select, Toggle } from "@/components/settings/controls";
import { blankPreset, isAccent, isFontFamily, LAYOUTS, presetProblem } from "@/export/export";
import { errorMessage, tauriExportApi, tauriPresetApi, type ExportApi, type PresetApi } from "@/lib/ipc";
import type { BrandFont, BrandPreset, FontKind, LogoChange, PresetList, PresetView, StepLayout } from "@/lib/types";
import { cn } from "@guiderecorder/ui/utils";

const FONT_KINDS: { value: FontKind; label: string }[] = [
  { value: "sans", label: "Sans serif" },
  { value: "serif", label: "Serif" },
  { value: "mono", label: "Monospace" },
];

const LAYOUT_OPTIONS = LAYOUTS.map((l) => ({ value: l.layout, label: l.label }));

/** A preset being edited: its id (null for a new one), settings and logo change. */
interface Draft {
  id: string | null;
  preset: BrandPreset;
  logo: LogoChange;
  /** What the logo looks like now, as a data URI. */
  logoSrc: string | null;
}

/**
 * The library's brand presets: named exports looks (logo, colours, fonts,
 * cover fields, header and footer, page numbers, layout, custom CSS),
 * stored in the library's `.presets` folder so a synced library shares
 * them. One can be the library's default, which the Export dialog and the
 * CLI start with.
 */
export function PresetsSection({
  presetApi = tauriPresetApi,
  exportApi = tauriExportApi,
}: {
  presetApi?: PresetApi;
  exportApi?: ExportApi;
}) {
  const [list, setList] = useState<PresetList | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The preset whose Delete was clicked once; a second click deletes it.
  const [deleting, setDeleting] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setList(await presetApi.listPresets());
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [presetApi]);
  useEffect(() => void reload(), [reload]);

  const run = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      await reload();
      return true;
    } catch (err) {
      setError(errorMessage(err));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!draft) return;
    if (await run(() => presetApi.savePreset(draft.id, draft.preset, draft.logo))) setDraft(null);
  };

  const edit = (view: PresetView) =>
    view.preset &&
    setDraft({ id: view.id, preset: view.preset, logo: { kind: "keep" }, logoSrc: view.logo });

  const presets = list?.presets ?? [];
  const defaultOptions = [
    { value: "", label: "None" },
    ...presets.filter((p) => p.preset && !p.error).map((p) => ({ value: p.id, label: p.preset!.name })),
  ];

  return (
    <Section
      title="Brand presets"
      intro="How exports of this library look: logo, colours, fonts, cover page, header and footer, and layout. Presets are files in the library's .presets folder, so a shared or synced library shares them."
    >
      {presets.length > 0 && (
        <Row label="Library default" note="The Export dialog and the command line start with it.">
          <Select
            label="Library default preset"
            value={list?.defaultId ?? ""}
            options={defaultOptions}
            disabled={busy}
            onChange={(id) => void run(() => presetApi.setDefaultPreset(id || null))}
          />
        </Row>
      )}
      {presets.map((view) => (
        <div key={view.id} className="flex items-center gap-3 border-b border-border py-2.5">
          {view.logo ? (
            <img src={view.logo} alt="" className="size-7 shrink-0 rounded-[3px] object-contain" />
          ) : (
            <ImageIcon className="size-5 shrink-0 text-muted-foreground" />
          )}
          <div className="flex min-w-0 flex-1 flex-col">
            <span className="truncate text-sm font-medium">
              {view.preset?.name ?? view.id}
              {view.id === list?.defaultId && <span className="font-normal text-muted-foreground"> · default</span>}
            </span>
            <span className={cn("truncate text-xs text-muted-foreground", view.error && "text-[var(--status-danger)]")}>
              {view.error ?? LAYOUTS.find((l) => l.layout === view.preset?.layout)?.label}
            </span>
          </div>
          {view.preset && (
            <Button variant="outline" size="sm" disabled={busy || !!draft} onClick={() => edit(view)}>
              Edit
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={() => {
              if (deleting !== view.id) return setDeleting(view.id);
              setDeleting(null);
              void run(() => presetApi.deletePreset(view.id));
            }}
            onBlur={() => setDeleting((d) => (d === view.id ? null : d))}
          >
            {deleting === view.id ? "Delete?" : "Delete"}
          </Button>
        </div>
      ))}
      {draft ? (
        <PresetEditor
          draft={draft}
          busy={busy}
          exportApi={exportApi}
          onChange={(update) => setDraft((d) => d && update(d))}
          onError={setError}
          onSave={() => void save()}
          onCancel={() => setDraft(null)}
        />
      ) : (
        <div className="pt-3">
          <Button
            variant="outline"
            size="sm"
            disabled={busy || !list}
            onClick={() =>
              setDraft({ id: null, preset: blankPreset(presets.length ? "" : "Default"), logo: { kind: "keep" }, logoSrc: null })
            }
          >
            <Plus /> New preset
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="mt-2 text-xs text-[var(--status-danger)]">
          {error}
        </p>
      )}
    </Section>
  );
}

function Field({ label, children, note }: { label: string; children: ReactNode; note?: string }) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      {children}
      {note && <span className="text-xs text-muted-foreground">{note}</span>}
    </div>
  );
}

/** `#rgb` as `#rrggbb`, for a colour well. */
function wellColor(value: string, fallback: string): string {
  const hex = value.trim();
  if (/^#[0-9a-f]{6}$/i.test(hex)) return hex.toLowerCase();
  if (/^#[0-9a-f]{3}$/i.test(hex)) return `#${[...hex.slice(1)].map((c) => c + c).join("")}`.toLowerCase();
  return fallback;
}

function ColourField({
  label,
  value,
  fallback,
  disabled,
  onChange,
}: {
  label: string;
  value: string | null;
  fallback: string;
  disabled: boolean;
  onChange(value: string | null): void;
}) {
  const text = value ?? "";
  const ok = isAccent(text);
  return (
    <Field label={label}>
      <div className="flex items-center gap-1.5">
        <input
          type="color"
          aria-label={`${label} well`}
          className="h-8 w-9 shrink-0 cursor-pointer rounded-md border border-input bg-transparent p-0.5"
          value={wellColor(text, fallback)}
          disabled={disabled}
          onChange={(e) => onChange(e.target.value)}
        />
        <Input
          aria-label={label}
          placeholder="Default"
          value={text}
          disabled={disabled}
          aria-invalid={!ok}
          className={cn(!ok && "border-[var(--status-danger)]")}
          onChange={(e) => onChange(e.target.value || null)}
        />
      </div>
    </Field>
  );
}

function FontField({
  label,
  font,
  disabled,
  onChange,
}: {
  label: string;
  font: BrandFont;
  disabled: boolean;
  onChange(font: BrandFont): void;
}) {
  const ok = isFontFamily(font.family ?? "");
  return (
    <Field label={label}>
      <div className="flex items-center gap-1.5">
        <Select
          label={`${label} kind`}
          value={font.kind}
          options={FONT_KINDS}
          disabled={disabled}
          onChange={(kind) => onChange({ ...font, kind })}
        />
        <Input
          aria-label={`${label} family`}
          placeholder="Brand font (optional)"
          value={font.family ?? ""}
          disabled={disabled}
          aria-invalid={!ok}
          className={cn(!ok && "border-[var(--status-danger)]")}
          onChange={(e) => onChange({ ...font, family: e.target.value || null })}
        />
      </div>
    </Field>
  );
}

function PresetEditor({
  draft,
  busy,
  exportApi,
  onChange,
  onError,
  onSave,
  onCancel,
}: {
  draft: Draft;
  busy: boolean;
  exportApi: ExportApi;
  onChange(update: (draft: Draft) => Draft): void;
  onError(error: string | null): void;
  onSave(): void;
  onCancel(): void;
}) {
  const p = draft.preset;
  const set = (patch: Partial<BrandPreset>) => onChange((d) => ({ ...d, preset: { ...d.preset, ...patch } }));
  const problem = presetProblem(p);

  const chooseLogo = async () => {
    onError(null);
    try {
      const path = await exportApi.chooseLogo();
      if (!path) return;
      const logoSrc = await exportApi.logoPreview(path);
      onChange((d) => ({ ...d, logo: { kind: "file", path }, logoSrc }));
    } catch (err) {
      onError(errorMessage(err));
    }
  };

  return (
    <div
      role="group"
      aria-label={draft.id ? `Edit ${p.name}` : "New preset"}
      className="mt-3 flex flex-col gap-3 rounded-xl border border-border p-4"
    >
      <Field label="Name">
        <Input aria-label="Preset name" value={p.name} disabled={busy} onChange={(e) => set({ name: e.target.value })} />
      </Field>

      <Field label="Logo" note="PNG or JPEG, up to 2 MB. It is copied into the preset.">
        <div className="flex items-center gap-2">
          {draft.logoSrc ? (
            <img src={draft.logoSrc} alt="" className="size-8 shrink-0 rounded-[3px] object-contain" />
          ) : (
            <ImageIcon className="size-5 shrink-0 text-muted-foreground" />
          )}
          <span className="flex-1 text-sm text-muted-foreground">{draft.logoSrc ? "" : "No logo"}</span>
          {draft.logoSrc && (
            <Button
              variant="ghost"
              size="sm"
              disabled={busy}
              onClick={() => onChange((d) => ({ ...d, logo: { kind: "remove" }, logoSrc: null }))}
            >
              Remove
            </Button>
          )}
          <Button variant="outline" size="sm" disabled={busy} onClick={() => void chooseLogo()}>
            Choose logo…
          </Button>
        </div>
      </Field>

      <div className="grid grid-cols-3 gap-2">
        <ColourField
          label="Accent"
          value={p.colors.accent}
          fallback="#2563eb"
          disabled={busy}
          onChange={(accent) => set({ colors: { ...p.colors, accent } })}
        />
        <ColourField
          label="Headings"
          value={p.colors.heading}
          fallback="#1f2328"
          disabled={busy}
          onChange={(heading) => set({ colors: { ...p.colors, heading } })}
        />
        <ColourField
          label="Text"
          value={p.colors.text}
          fallback="#1f2328"
          disabled={busy}
          onChange={(text) => set({ colors: { ...p.colors, text } })}
        />
      </div>

      <div className="grid grid-cols-2 gap-2">
        <FontField
          label="Text font"
          font={p.fonts.body}
          disabled={busy}
          onChange={(body) => set({ fonts: { ...p.fonts, body } })}
        />
        <FontField
          label="Heading font"
          font={p.fonts.heading}
          disabled={busy}
          onChange={(heading) => set({ fonts: { ...p.fonts, heading } })}
        />
      </div>
      <p className="-mt-1 text-xs text-muted-foreground">
        HTML and Word use a brand font where it is installed; PDFs use a built-in font of the kind chosen.
      </p>

      <div className="grid grid-cols-3 gap-2">
        <Field label="Company">
          <Input aria-label="Company" value={p.cover.company} disabled={busy} onChange={(e) => set({ cover: { ...p.cover, company: e.target.value } })} />
        </Field>
        <Field label="Author">
          <Input aria-label="Author" value={p.cover.author} disabled={busy} onChange={(e) => set({ cover: { ...p.cover, author: e.target.value } })} />
        </Field>
        <Field label="Version">
          <Input aria-label="Document version" value={p.cover.version} disabled={busy} onChange={(e) => set({ cover: { ...p.cover, version: e.target.value } })} />
        </Field>
      </div>
      <label className="flex items-center gap-2 text-sm">
        <Toggle
          label="Show the date on the cover"
          checked={p.cover.date}
          disabled={busy}
          onChange={(date) => set({ cover: { ...p.cover, date } })}
        />
        Show the date the guide last changed on the cover
      </label>

      <div className="grid grid-cols-2 gap-2">
        <Field label="Header">
          <Input aria-label="Header text" placeholder="The guide title" value={p.header} disabled={busy} onChange={(e) => set({ header: e.target.value })} />
        </Field>
        <Field label="Footer">
          <Input aria-label="Footer text" placeholder="None" value={p.footer} disabled={busy} onChange={(e) => set({ footer: e.target.value })} />
        </Field>
      </div>
      <label className="flex items-center gap-2 text-sm">
        <Toggle label="Page numbers" checked={p.pageNumbers} disabled={busy} onChange={(pageNumbers) => set({ pageNumbers })} />
        Number the pages of PDF and Word exports
      </label>

      <Field label="Layout">
        <Select<StepLayout>
          label="Preset layout"
          value={p.layout}
          options={LAYOUT_OPTIONS}
          disabled={busy}
          onChange={(layout) => set({ layout })}
        />
      </Field>

      <Field label="Custom CSS (HTML only)">
        <Textarea
          aria-label="Custom CSS"
          rows={4}
          value={p.css}
          disabled={busy}
          placeholder={".step { border-radius: 0; }"}
          className="font-mono text-xs"
          onChange={(e) => set({ css: e.target.value })}
        />
      </Field>

      {problem && p.name.trim() && <p className="text-xs text-[var(--status-danger)]">{problem}</p>}
      <div className="flex justify-end gap-2">
        <Button variant="outline" size="sm" disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" disabled={busy || !!problem} onClick={onSave}>
          {draft.id ? "Save preset" : "Add preset"}
        </Button>
      </div>
    </div>
  );
}
