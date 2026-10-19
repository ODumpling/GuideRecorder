import { useEffect, useState, type ReactNode } from "react";
import { Switch } from "radix-ui";
import { Input } from "@guiderecorder/ui/input";
import { formatHotkey, hotkeyFromEvent } from "@/settings/validate";
import { cn } from "@guiderecorder/ui/utils";

/** One setting: a label and note on the left, the control on the right. */
export function Row({ label, note, children }: { label: string; note?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-6 border-b border-border py-3 last:border-b-0">
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="text-sm font-medium">{label}</span>
        {note && <span className="text-xs text-muted-foreground">{note}</span>}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}

export function Section({ title, intro, children }: { title: string; intro?: ReactNode; children: ReactNode }) {
  return (
    <section className="flex flex-col">
      <h2 className="text-base font-semibold">{title}</h2>
      {intro && <p className="mt-1 text-xs text-muted-foreground">{intro}</p>}
      <div className="mt-2 flex flex-col">{children}</div>
    </section>
  );
}

export function Toggle({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange(checked: boolean): void;
}) {
  return (
    <Switch.Root
      aria-label={label}
      checked={checked}
      disabled={disabled}
      onCheckedChange={onChange}
      className="relative h-5 w-9 shrink-0 rounded-full bg-input outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50 data-[state=checked]:bg-[var(--status-info)]"
    >
      <Switch.Thumb className="block size-4 translate-x-0.5 rounded-full bg-background shadow transition-transform data-[state=checked]:translate-x-[18px]" />
    </Switch.Root>
  );
}

/**
 * A text field that commits on Enter or blur, when `valid`. Shows the
 * stored value again after a refused or cancelled edit (Escape).
 */
export function CommitInput({
  label,
  value,
  valid,
  disabled,
  placeholder,
  className,
  type = "text",
  onCommit,
}: {
  label: string;
  value: string;
  valid(value: string): boolean;
  disabled?: boolean;
  placeholder?: string;
  className?: string;
  type?: "text" | "number";
  onCommit(value: string): void;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const ok = valid(draft);
  const commit = () => {
    if (draft === value) return;
    if (ok) onCommit(draft);
    else setDraft(value);
  };
  return (
    <Input
      aria-label={label}
      type={type}
      value={draft}
      disabled={disabled}
      placeholder={placeholder}
      aria-invalid={!ok}
      className={cn("w-56", !ok && "border-[var(--status-danger)]", className)}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") setDraft(value);
      }}
    />
  );
}

/**
 * Records a shortcut: focus it and press the keys. Backspace clears it,
 * Escape keeps the old one.
 */
export function HotkeyField({
  label,
  value,
  disabled,
  onChange,
}: {
  label: string;
  value: string;
  disabled?: boolean;
  onChange(value: string): void;
}) {
  const [recording, setRecording] = useState(false);
  return (
    <button
      type="button"
      aria-label={label}
      disabled={disabled}
      onFocus={() => setRecording(true)}
      onBlur={() => setRecording(false)}
      onKeyDown={(e) => {
        if (e.key === "Tab") return;
        e.preventDefault();
        e.stopPropagation();
        if (e.key === "Escape" && !e.metaKey && !e.ctrlKey && !e.altKey) {
          e.currentTarget.blur();
          return;
        }
        if (e.key === "Backspace" && !e.metaKey && !e.ctrlKey && !e.altKey) {
          onChange("");
          e.currentTarget.blur();
          return;
        }
        const hotkey = hotkeyFromEvent(e);
        if (hotkey) {
          onChange(hotkey);
          e.currentTarget.blur();
        }
      }}
      className={cn(
        "h-8 w-40 rounded-md border border-input px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50",
        recording && "text-muted-foreground",
      )}
    >
      {recording ? "Type a shortcut…" : formatHotkey(value)}
    </button>
  );
}

export function Select<T extends string>({
  label,
  value,
  options,
  disabled,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  disabled?: boolean;
  onChange(value: T): void;
}) {
  return (
    <select
      aria-label={label}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value as T)}
      className="h-8 rounded-md border border-input bg-transparent px-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

/** A path shown by its last part, with the whole path on hover. */
export function PathText({ path, empty }: { path: string | null; empty: string }) {
  const name = path ? path.replace(/\/+$/, "").split("/").pop() || path : empty;
  return (
    <span className={cn("max-w-52 truncate text-sm", !path && "text-muted-foreground")} title={path ?? undefined}>
      {name}
    </span>
  );
}
