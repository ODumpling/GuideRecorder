import type { GuideStatus } from "@guiderecorder/guide-schema";
import { GUIDE_STATUSES, initials } from "@/editor/library";
import { cn } from "@guiderecorder/ui/utils";

/** The colour of each status, as a CSS colour. */
export const STATUS_COLORS: Record<GuideStatus, string> = {
  draft: "var(--status-neutral)",
  inReview: "var(--status-warning)",
  published: "var(--status-success)",
  needsUpdate: "var(--status-danger)",
};

/** A guide's status: a coloured dot and its name. */
export function StatusPill({ status, className }: { status: GuideStatus; className?: string }) {
  const label = GUIDE_STATUSES.find((s) => s.value === status)?.label ?? status;
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-full bg-foreground/[0.06] py-0.5 pr-2 pl-1.5 text-[11px] font-medium text-foreground/80",
        className,
      )}
    >
      <span className="size-1.5 rounded-full" style={{ background: STATUS_COLORS[status] }} />
      {label}
    </span>
  );
}

/** A hue from a name, so the same owner always gets the same colour. */
function hue(name: string): number {
  let h = 0;
  for (const c of name.toLowerCase()) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

/** An owner's initials in a coloured circle; the name on hover. */
export function Avatar({ name, className }: { name: string; className?: string }) {
  return (
    <span
      title={name}
      aria-label={`Owner ${name}`}
      className={cn(
        "flex size-5 shrink-0 items-center justify-center rounded-full text-[9px] font-semibold text-white",
        className,
      )}
      style={{ background: `hsl(${hue(name)} 42% 48%)` }}
    >
      {initials(name)}
    </span>
  );
}

/** A small uppercase heading over a group of sidebar items. */
export function NavHeading({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <h2 className="flex h-6 items-center justify-between px-2 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">
      {children}
      {action}
    </h2>
  );
}
