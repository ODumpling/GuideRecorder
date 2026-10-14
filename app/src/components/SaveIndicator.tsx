import { Check, CircleAlert, LoaderCircle } from "lucide-react";
import { useEditor } from "@/editor/editor";

export function SaveIndicator() {
  const status = useEditor((s) => s.saveStatus);
  const error = useEditor((s) => s.saveError);
  const readOnly = useEditor((s) => s.doc?.readOnly);
  if (readOnly) return null;
  switch (status) {
    case "saved":
      return (
        <span className="flex items-center gap-1 text-xs text-muted-foreground">
          <Check className="size-3.5" /> Saved
        </span>
      );
    case "pending":
    case "saving":
      return (
        <span className="flex items-center gap-1 text-xs text-muted-foreground">
          <LoaderCircle className="size-3.5 animate-spin" /> Saving…
        </span>
      );
    case "error":
      return (
        <span className="flex items-center gap-1 text-xs text-[var(--status-danger)]" title={error ?? ""}>
          <CircleAlert className="size-3.5" /> Not saved
        </span>
      );
  }
}
