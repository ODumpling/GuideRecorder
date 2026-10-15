import { useState } from "react";
import { aiStore } from "@/ai/ai";
import { useAiBlocked } from "@/ai/WriteWithAi";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/** The most a custom rewrite instruction may be, after trimming (as `ai_rewrite_step` checks). */
export const MAX_INSTRUCTION = 500;

/** Rewrite → Custom…: a one-line instruction for rewriting the step. */
export function CustomRewrite({ stepId, onClose }: { stepId: string; onClose(): void }) {
  const [instruction, setInstruction] = useState("");
  const { busy } = useAiBlocked();
  const trimmed = instruction.trim();
  const valid = trimmed.length >= 1 && trimmed.length <= MAX_INSTRUCTION;
  const submit = () => {
    if (!valid || busy) return;
    onClose();
    void aiStore.getState().rewriteStep(stepId, { kind: "custom", instruction: trimmed });
  };
  return (
    <form
      className="flex items-center gap-2"
      aria-label="Custom rewrite"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <Input
        autoFocus
        aria-label="Rewrite instruction"
        placeholder="How should AI rewrite this step?"
        maxLength={MAX_INSTRUCTION}
        value={instruction}
        onChange={(e) => setInstruction(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
        }}
      />
      <Button type="submit" size="sm" disabled={!valid || busy}>
        Rewrite
      </Button>
      <Button type="button" variant="ghost" size="sm" onClick={onClose}>
        Cancel
      </Button>
    </form>
  );
}
