import { Label } from "@guiderecorder/ui/label";
import { cn } from "@guiderecorder/ui/utils";
import type { ComponentProps } from "react";

/** A label wrapping its caption and control, stacked. */
export function Field({ className, ...props }: ComponentProps<typeof Label>) {
  return <Label className={cn("grid gap-1.5 leading-normal", className)} {...props} />;
}

export function ErrorText({ children }: { children?: string | null }) {
  return children ? <p role="alert" className="text-sm text-destructive">{children}</p> : null;
}
