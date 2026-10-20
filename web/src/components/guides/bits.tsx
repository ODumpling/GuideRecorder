import type { GuideStatus } from "@guiderecorder/guide-schema";
import { cn } from "@guiderecorder/ui/utils";
import { STATUS_LABEL } from "@/lib/library-search";

// Small pieces the desktop app's library shows too (app
// components/library/bits.tsx), so guides look the same in both.

/** The colour of each status, as a CSS colour. */
export const STATUS_COLORS: Record<GuideStatus, string> = {
  draft: "var(--status-neutral)",
  inReview: "var(--status-warning)",
  published: "var(--status-success)",
  needsUpdate: "var(--status-danger)",
};

/** A guide's status: a coloured dot and its name. */
export function StatusPill({ status, className }: { status: GuideStatus; className?: string }) {
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-full bg-foreground/[0.06] py-0.5 pr-2 pl-1.5 text-[11px] font-medium text-foreground/80",
        className,
      )}
    >
      <span className="size-1.5 rounded-full" style={{ background: STATUS_COLORS[status] }} />
      {STATUS_LABEL[status]}
    </span>
  );
}

/** Up to two initials of a name, for an avatar: "Dana Ortiz" → "DO". */
export function initials(name: string): string {
  const words = name.trim().split(/[\s._@-]+/).filter(Boolean);
  return words
    .slice(0, 2)
    .map((w) => [...w][0]!.toUpperCase())
    .join("");
}

/** A hue from a name, so the same person always gets the same colour. */
function hue(name: string): number {
  let h = 0;
  for (const c of name.toLowerCase()) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

/** Someone's initials in a coloured circle; the name on hover. */
export function Avatar({ name, label = name, className }: { name: string; label?: string; className?: string }) {
  return (
    <span
      title={label}
      aria-label={label}
      className={cn("flex size-5 shrink-0 items-center justify-center rounded-full text-[9px] font-semibold text-white", className)}
      style={{ background: `hsl(${hue(name)} 42% 48%)` }}
    >
      {initials(name)}
    </span>
  );
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["week", 7 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
];

/** How long ago `iso` was, e.g. "2 hours ago" or "yesterday"; "just now" under a minute. */
export function timeAgo(iso: string, now: Date = new Date()): string {
  const seconds = (now.getTime() - (Date.parse(iso) || 0)) / 1000;
  const format = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  for (const [unit, size] of UNITS) {
    if (Math.abs(seconds) >= size) return format.format(-Math.round(seconds / size), unit);
  }
  return "just now";
}
