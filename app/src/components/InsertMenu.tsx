import { useRef } from "react";
import { DropdownMenu } from "radix-ui";
import { FilePlus2, Heading, ImagePlus, Info, Lightbulb, Plus, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { insertImages, insertNote, insertPages, insertSection } from "@/editor/actions";
import { editorStore, useAnnotated } from "@/editor/editor";
import { IMAGE_EXTENSIONS } from "@/editor/images";

const ITEM =
  "flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none select-none data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground [&_svg]:size-4 [&_svg]:text-muted-foreground";

/**
 * Adds sections, notes and image steps around the selected item; in an
 * annotation guide, pages from images or a PDF instead of image steps.
 */
export default function InsertMenu({ disabled }: { disabled: boolean }) {
  const fileInput = useRef<HTMLInputElement>(null);
  const annotated = useAnnotated();
  return (
    <>
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild disabled={disabled}>
          <Button
            variant="outline"
            size="sm"
            aria-label="Insert"
            title={annotated ? "Add pages, a section or a note" : "Add a step from an image, a section or a note"}
          >
            <Plus /> Insert
          </Button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="end"
            sideOffset={4}
            className="z-50 min-w-48 rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-md"
          >
            {annotated ? (
              // The native dialog runs on the Rust side, which reads PDFs too.
              <DropdownMenu.Item className={ITEM} onSelect={() => void insertPages(editorStore)}>
                <FilePlus2 /> Add pages…
              </DropdownMenu.Item>
            ) : (
              <DropdownMenu.Item className={ITEM} onSelect={() => fileInput.current?.click()}>
                <ImagePlus /> Step from image…
              </DropdownMenu.Item>
            )}
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
          void insertImages(editorStore, files);
        }}
      />
    </>
  );
}
