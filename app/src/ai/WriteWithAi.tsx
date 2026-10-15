import { Sparkles, Square } from "lucide-react";
import { aiStore, useAi } from "@/ai/ai";
import { Button } from "@/components/ui/button";
import { useEditor } from "@/editor/editor";
import type { AiState } from "@/lib/types";
import { cn } from "@/lib/utils";

/**
 * The editor header's AI control: "Write with AI" starts a run on the open
 * guide, "Stop" ends it. Why it can't run (AI off, no key) comes back from
 * the backend and shows as the editor's notice. While an explicit action
 * (Regenerate, Rewrite) waits, it is disabled: one AI action at a time.
 */
export function WriteWithAi() {
  const readOnly = useEditor((s) => s.doc?.readOnly ?? true);
  const phase = useAi((s) => s.phase);
  const explicit = useAi((s) => s.explicit !== null);
  if (phase === "idle" || explicit) {
    return (
      <Button
        variant="outline"
        size="sm"
        className="ml-1"
        disabled={readOnly || explicit}
        title={explicit ? "Wait for the AI action to finish" : undefined}
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

/** Whether explicit AI actions are blocked by a background run (the conflict rule). */
export function useAiBlocked(): { busy: boolean; background: boolean } {
  const phase = useAi((s) => s.phase);
  const explicit = useAi((s) => s.explicit !== null);
  return { busy: phase !== "idle" || explicit, background: phase !== "idle" && !explicit };
}

const STATUS_LABELS: Record<AiState["status"], string> = {
  template: "Template",
  // Only the running controller knows a step is being written; a saved
  // `generating` is a pass that never finished.
  generating: "Template",
  ai: "AI",
  edited: "Edited",
};

/**
 * A step's text status beside it in the outline: "Writing…" while AI writes
 * it, else template / AI / edited, and "Failed" when the last pass failed.
 */
export function StepStatusBadge({ stepId, aiState }: { stepId: string; aiState: AiState }) {
  const writing = useAi((s) => s.generating.has(stepId));
  const failed = !writing && !!aiState.error;
  const label = writing ? "Writing…" : STATUS_LABELS[aiState.status];
  return (
    <span className="ml-auto flex shrink-0 items-center gap-1" data-testid="step-status">
      {failed && (
        <span className="text-[10px] text-[var(--status-warning)]" title={`AI could not write this step: ${aiState.error}`}>
          Failed
        </span>
      )}
      <span
        title={writing ? "AI is writing this step" : `Text: ${label}`}
        className={cn(
          "rounded px-1 text-[10px] leading-4 text-muted-foreground",
          writing && "animate-pulse",
          aiState.status === "ai" && !writing && "bg-muted",
        )}
      >
        {!writing && <span className="sr-only">Text: </span>}
        {label}
      </span>
    </span>
  );
}
