import { useRef } from "react";
import { DropdownMenu } from "radix-ui";
import { ChevronDown, GraduationCap, Minimize2, PenLine, RefreshCw, Rocket, Sparkles } from "lucide-react";
import { aiStore } from "@/ai/ai";
import { BUSY_HINT } from "@/ai/controller";
import { useAiBlocked } from "@/ai/WriteWithAi";
import { Button } from "@/components/ui/button";
import type { RewriteStyleArg } from "@/lib/types";

const ITEM =
  "flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground [&_svg]:size-4 [&_svg]:text-muted-foreground";
const LABEL = "px-2 py-1 text-xs text-muted-foreground";

const STYLES: { style: RewriteStyleArg; label: string; icon: typeof Minimize2 }[] = [
  { style: { kind: "shorter" }, label: "Shorter", icon: Minimize2 },
  { style: { kind: "beginner" }, label: "For beginners", icon: GraduationCap },
  { style: { kind: "expert" }, label: "For experts", icon: Rocket },
];

/**
 * The step's AI actions: Regenerate, and Rewrite in a style. Each replaces
 * the step's text (edited or not) as one undoable change.
 */
export default function StepAiMenu({ stepId, onCustom }: { stepId: string; onCustom(): void }) {
  const { busy, background } = useAiBlocked();
  // Custom… opens a form whose input takes the focus from the trigger.
  const custom = useRef(false);
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <Button variant="ghost" size="sm" aria-label="AI actions" title="Regenerate or rewrite this step with AI">
          <Sparkles /> AI <ChevronDown />
        </Button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={4}
          onCloseAutoFocus={(e) => {
            if (!custom.current) return;
            // Opened once the menu has closed, so its input keeps the focus.
            e.preventDefault();
            custom.current = false;
            onCustom();
          }}
          className="z-50 max-w-64 min-w-48 rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-md"
        >
          {background && <DropdownMenu.Label className={LABEL}>{BUSY_HINT}</DropdownMenu.Label>}
          <DropdownMenu.Item
            className={ITEM}
            disabled={busy}
            onSelect={() => void aiStore.getState().regenerateStep(stepId)}
          >
            <RefreshCw /> Regenerate
          </DropdownMenu.Item>
          <DropdownMenu.Separator className="my-1 h-px bg-border" />
          <DropdownMenu.Label className={LABEL}>Rewrite</DropdownMenu.Label>
          {STYLES.map(({ style, label, icon: Icon }) => (
            <DropdownMenu.Item
              key={style.kind}
              className={ITEM}
              disabled={busy}
              onSelect={() => void aiStore.getState().rewriteStep(stepId, style)}
            >
              <Icon /> {label}
            </DropdownMenu.Item>
          ))}
          <DropdownMenu.Item
            className={ITEM}
            disabled={busy}
            onSelect={() => {
              custom.current = true;
            }}
          >
            <PenLine /> Custom…
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
