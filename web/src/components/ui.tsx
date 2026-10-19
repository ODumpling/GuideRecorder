import type { ComponentProps } from "react";

const cx = (...classes: (string | false | undefined)[]) => classes.filter(Boolean).join(" ");

export function Button({ variant = "primary", className, ...props }: ComponentProps<"button"> & { variant?: "primary" | "secondary" | "danger" }) {
  return (
    <button
      className={cx(
        "inline-flex h-9 items-center justify-center gap-2 rounded-lg px-4 text-sm font-medium transition-colors focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50",
        variant === "primary" && "bg-brand text-brand-foreground hover:bg-brand/90",
        variant === "secondary" && "border border-border bg-card hover:bg-muted",
        variant === "danger" && "border border-border bg-card text-destructive hover:bg-muted",
        className,
      )}
      {...props}
    />
  );
}

export function Input({ className, ...props }: ComponentProps<"input">) {
  return (
    <input
      className={cx(
        "h-9 w-full rounded-lg border border-input bg-card px-3 text-sm outline-none focus:border-ring focus:ring-2 focus:ring-ring/20",
        className,
      )}
      {...props}
    />
  );
}

export function Card({ className, ...props }: ComponentProps<"section">) {
  return <section className={cx("rounded-xl border border-border bg-card p-6 shadow-sm", className)} {...props} />;
}

export function Label({ className, ...props }: ComponentProps<"label">) {
  return <label className={cx("grid gap-1.5 text-sm font-medium", className)} {...props} />;
}

export function ErrorText({ children }: { children?: string | null }) {
  return children ? <p role="alert" className="text-sm text-destructive">{children}</p> : null;
}
