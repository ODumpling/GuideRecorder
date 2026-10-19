// A guide item's content, shared by the desktop editor and the web editor:
// a step's image, title and description, a section's title, or a note.
// Each host brings its own image layer and text fields (the app's are
// store commands; the web's edit a draft).

import type { ReactNode } from "react";
import { Info, Lightbulb, TriangleAlert } from "lucide-react";
import type { Item, NoteKind } from "@guiderecorder/guide-schema";

export const NOTE_STYLES: Record<NoteKind, { icon: typeof Info; color: string }> = {
  tip: { icon: Lightbulb, color: "var(--status-success)" },
  warning: { icon: TriangleAlert, color: "var(--status-warning)" },
  note: { icon: Info, color: "var(--status-info)" },
};

/** The item fields holding Markdown. */
export type ItemTextField = "title" | "description" | "body";

/** One Markdown field of the item, for the host to render as an editor. */
export interface ItemTextProps {
  field: ItemTextField;
  value: string;
  label: string;
  placeholder: string;
  singleLine?: boolean;
  className?: string;
}

export interface ItemViewProps {
  item: Item;
  /** What a step is called: "Step", or "Page" in an annotated guide. */
  noun?: string;
  /** A step's image, with its annotations. */
  image?: ReactNode;
  text(props: ItemTextProps): ReactNode;
  /** Under a step's text. */
  children?: ReactNode;
}

/** The selected item's content. */
export function ItemView({ item, noun = "Step", image, text, children }: ItemViewProps) {
  if (item.type === "step") {
    return (
      <>
        {image}
        <div className="mt-3 flex flex-col gap-1">
          {text({
            field: "title",
            value: item.title,
            label: `${noun} title`,
            placeholder: `${noun} title`,
            singleLine: true,
            className: "font-serif text-[28px] leading-tight tracking-tight [&_strong]:font-normal [&_strong]:italic",
          })}
          {text({
            field: "description",
            value: item.description,
            label: `${noun} description`,
            placeholder: "Add a description",
            className: "text-[15px] leading-relaxed text-foreground/85",
          })}
        </div>
        {children}
      </>
    );
  }
  if (item.type === "section") {
    return text({
      field: "title",
      value: item.title,
      label: "Section title",
      placeholder: "Section title",
      singleLine: true,
      className: "font-serif text-4xl tracking-tight",
    });
  }
  const note = NOTE_STYLES[item.kind];
  return (
    <div
      className="flex gap-3 rounded-xl border p-4"
      style={{
        borderColor: `color-mix(in oklab, ${note.color} 30%, transparent)`,
        background: `color-mix(in oklab, ${note.color} 9%, transparent)`,
      }}
    >
      <note.icon className="mt-2 size-4 shrink-0" style={{ color: note.color }} />
      <div className="min-w-0 flex-1">
        {text({
          field: "body",
          value: item.body,
          label: `${item.kind} text`,
          placeholder: "Write the note",
          className: "text-[15px] leading-relaxed",
        })}
      </div>
    </div>
  );
}
