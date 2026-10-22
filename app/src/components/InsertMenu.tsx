import { useRef } from "react";
import { DropdownMenu } from "radix-ui";
import { FilePlus2, GitBranch, Heading, ImagePlus, Info, Lightbulb, ListPlus, Plus, TriangleAlert } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { insertDecision, insertImages, insertNote, insertPages, insertSection } from "@/editor/actions";
import { editorStore, useAnnotated } from "@/editor/editor";
import { IMAGE_EXTENSIONS } from "@/editor/images";
import { startInsert } from "@/editor/recapture";
import { startFlow, useRecording } from "@/recording/recording";

const ITEM =
  "flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none select-none data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground [&_svg]:size-4 [&_svg]:text-muted-foreground";

/**
 * Adds sections, notes, image steps and recorded steps around the selected
 * item; in an annotation guide, pages from images or a PDF instead of
 * image and recorded steps. `atEnd`: the Add step button after the last
 * item, which adds only steps (or pages), there.
 */
export default function InsertMenu({ disabled, atEnd = false }: { disabled: boolean; atEnd?: boolean }) {
  const fileInput = useRef<HTMLInputElement>(null);
  const annotated = useAnnotated();
  const recording = useRecording((s) => s.status.state !== "idle" || s.pending !== null);
  return (
    <>
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild disabled={disabled}>
          {atEnd ? (
            <Button
              variant="ghost"
              size="sm"
              className="w-full justify-start text-muted-foreground"
              title={annotated ? "Add pages at the end" : "Add a step from an image, or recorded steps, at the end"}
            >
              <Plus /> {annotated ? "Add pages" : "Add step"}
            </Button>
          ) : (
            <Button
              variant="outline"
              size="sm"
              aria-label="Insert"
              title={annotated ? "Add pages, a section or a note" : "Add a step from an image, recorded steps, a section or a note"}
            >
              <Plus /> Insert
            </Button>
          )}
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align={atEnd ? "start" : "end"}
            sideOffset={4}
            className="z-50 min-w-48 rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-md"
          >
            {annotated ? (
              // The native dialog runs on the Rust side, which reads PDFs too.
              <DropdownMenu.Item className={ITEM} onSelect={() => void insertPages(editorStore, atEnd)}>
                <FilePlus2 /> Add pages…
              </DropdownMenu.Item>
            ) : (
              <>
                <DropdownMenu.Item className={ITEM} onSelect={() => fileInput.current?.click()}>
                  <ImagePlus /> Step from image…
                </DropdownMenu.Item>
                <DropdownMenu.Item
                  className={`${ITEM} data-[disabled]:opacity-50`}
                  disabled={recording}
                  onSelect={() => void startInsert(editorStore, startFlow, atEnd ? null : editorStore.getState().selectedId)}
                >
                  <ListPlus /> Recorded steps…
                </DropdownMenu.Item>
              </>
            )}
            {!atEnd && (
              <>
                <DropdownMenu.Separator className="my-1 h-px bg-border" />
                <DropdownMenu.Item className={ITEM} onSelect={() => insertSection(editorStore)}>
                  <Heading /> Section above
                </DropdownMenu.Item>
                <DropdownMenu.Item className={ITEM} onSelect={() => insertNote(editorStore, "tip")}>
                  <Lightbulb /> Tip below
                </DropdownMenu.Item>
                <DropdownMenu.Item className={ITEM} onSelect={() => insertNote(editorStore, "warning")}>
                  <TriangleAlert /> Warning below
                </DropdownMenu.Item>
                <DropdownMenu.Item className={ITEM} onSelect={() => insertNote(editorStore, "note")}>
                  <Info /> Note below
                </DropdownMenu.Item>
                <DropdownMenu.Item className={ITEM} onSelect={() => insertDecision(editorStore)}>
                  <GitBranch /> Decision below
                </DropdownMenu.Item>
              </>
            )}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
      <input
        ref={fileInput}
        type="file"
        multiple
        hidden
        accept={["image/*", ...IMAGE_EXTENSIONS.map((e) => `.${e}`)].join(",")}
        onChange={(e) => {
          const files = [...(e.target.files ?? [])];
          e.target.value = "";
          void insertImages(editorStore, files, atEnd);
        }}
      />
    </>
  );
}
