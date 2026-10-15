import { Sparkles, Square } from "lucide-react";
import { aiStore, useAi } from "@/ai/ai";
import { Button } from "@/components/ui/button";
import { useEditor } from "@/editor/editor";

/**
 * The editor header's AI control: "Write with AI" starts a run on the open
 * guide, "Stop" ends it. Why it can't run (AI off, no key) comes back from
 * the backend and shows as the editor's notice.
 */
export function WriteWithAi() {
  const readOnly = useEditor((s) => s.doc?.readOnly ?? true);
  const phase = useAi((s) => s.phase);
  if (phase === "idle") {
    return (
      <Button
        variant="outline"
        size="sm"
        className="ml-1"
        disabled={readOnly}
        onClick={() => void aiStore.getState().start()}
      >
        <Sparkles /> Write with AI
      </Button>
    );
  }
  return (
    <Button
      variant="outline"
      size="sm"
      className="ml-1"
      disabled={phase === "stopping"}
      onClick={() => void aiStore.getState().stop()}
    >
      <Square /> {phase === "stopping" ? "Stopping…" : "Stop"}
    </Button>
  );
}

/** "Writing…" beside a step the running AI pass is still writing. */
export function WritingIndicator({ stepId }: { stepId: string }) {
  const writing = useAi((s) => s.generating.has(stepId));
  if (!writing) return null;
  return (
    <span className="ml-auto shrink-0 animate-pulse text-xs text-muted-foreground" aria-label="Writing">
      Writing…
    </span>
  );
}
