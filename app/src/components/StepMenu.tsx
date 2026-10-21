import { DropdownMenu } from "radix-ui";
import { Camera, Check, Circle, Ellipsis, ImageIcon, ImageOff, ListPlus } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { changeCover, toggleCover } from "@/editor/actions";
import { coverOptions, coverStepId } from "@/editor/commands";
import { editorStore, stepNoun, useAnnotated, useEditor } from "@/editor/editor";
import { startInsert, startRecapture } from "@/editor/recapture";
import { startFlow, useRecording } from "@/recording/recording";
import type { ImageShow, Step } from "@guiderecorder/guide-schema";

const ITEM =
  "flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none select-none data-[disabled]:opacity-50 data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground [&_svg]:size-4 [&_svg]:text-muted-foreground";
const CHECKABLE = `${ITEM} relative pl-8`;
const INDICATOR = "absolute left-2 flex size-4 items-center justify-center";
const LABEL = "px-2 pt-1.5 pb-1 text-xs font-medium text-muted-foreground";

/**
 * The step's ⋯ menu: Re-capture step and Insert recorded steps here (in a
 * step-by-step guide), Set as cover (or Remove as cover), and, on the
 * cover, how it is drawn. Each is one undoable change.
 */
export default function StepMenu({ step }: { step: Step }) {
  const annotated = useAnnotated();
  const isCover = useEditor((s) => !!s.doc && coverStepId(s.doc.guide) === step.id);
  const image = useEditor((s) => (s.doc ? coverOptions(s.doc.guide).image : "full"));
  const annotations = useEditor((s) => !!s.doc && coverOptions(s.doc.guide).annotations);
  const recording = useRecording((s) => s.status.state !== "idle" || s.pending !== null);
  const noun = stepNoun(annotated);
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <Button variant="ghost" size="icon" aria-label={`More ${noun} actions`} title={`More ${noun} actions`}>
          <Ellipsis />
        </Button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={4}
          className="z-50 min-w-56 rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-md"
        >
          {!annotated && (
            <>
              <DropdownMenu.Item
                className={ITEM}
                disabled={recording}
                title="Record this step again: the next click, shortcut or manual step replaces its screenshot"
                onSelect={() => void startRecapture(editorStore, startFlow, step.id)}
              >
                <Camera /> Re-capture step
              </DropdownMenu.Item>
              <DropdownMenu.Item
                className={ITEM}
                disabled={recording}
                title="Record new steps and insert them after this one"
                onSelect={() => void startInsert(editorStore, startFlow, step.id)}
              >
                <ListPlus /> Insert recorded steps here
              </DropdownMenu.Item>
              <DropdownMenu.Separator className="my-1 h-px bg-border" />
            </>
          )}
          <DropdownMenu.Item
            className={ITEM}
            disabled={!step.image.full}
            onSelect={() => toggleCover(editorStore, step.id)}
          >
            {isCover ? (
              <>
                <ImageOff /> Remove as cover
              </>
            ) : (
              <>
                <ImageIcon /> Set as cover
              </>
            )}
          </DropdownMenu.Item>
          {isCover && (
            <>
              <DropdownMenu.Separator className="my-1 h-px bg-border" />
              <DropdownMenu.Label className={LABEL}>Cover shows</DropdownMenu.Label>
              <DropdownMenu.RadioGroup
                value={image}
                onValueChange={(image) => changeCover(editorStore, { image: image as ImageShow })}
              >
                <DropdownMenu.RadioItem className={CHECKABLE} value="crop" disabled={!step.image.crop}>
                  <DropdownMenu.ItemIndicator className={INDICATOR}>
                    <Circle className="size-2 fill-current" />
                  </DropdownMenu.ItemIndicator>
                  The close-up
                </DropdownMenu.RadioItem>
                <DropdownMenu.RadioItem className={CHECKABLE} value="full">
                  <DropdownMenu.ItemIndicator className={INDICATOR}>
                    <Circle className="size-2 fill-current" />
                  </DropdownMenu.ItemIndicator>
                  The full screenshot
                </DropdownMenu.RadioItem>
              </DropdownMenu.RadioGroup>
              <DropdownMenu.CheckboxItem
                className={CHECKABLE}
                checked={annotations}
                onCheckedChange={(annotations) => changeCover(editorStore, { annotations })}
              >
                <DropdownMenu.ItemIndicator className={INDICATOR}>
                  <Check />
                </DropdownMenu.ItemIndicator>
                Draw annotations
              </DropdownMenu.CheckboxItem>
            </>
          )}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
