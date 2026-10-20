import { Button } from "@guiderecorder/ui/button";

export type Frame = "before" | "after";

/**
 * Shows a step's screenshot (Before) or its after frame (After). View-only:
 * the choice is not part of the guide.
 */
export function BeforeAfterSwitch({ value, onChange }: { value: Frame; onChange(value: Frame): void }) {
  return (
    <div className="flex items-center gap-1" role="group" aria-label="Screenshot">
      {(["before", "after"] as const).map((frame) => (
        <Button
          key={frame}
          size="sm"
          variant={value === frame ? "default" : "outline"}
          aria-pressed={value === frame}
          title={frame === "before" ? "Show the screenshot taken before the action" : "Show the screen after the action"}
          onClick={() => onChange(frame)}
        >
          {frame === "before" ? "Before" : "After"}
        </Button>
      ))}
    </div>
  );
}
