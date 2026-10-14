import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { EditorContent, useEditor, useEditorState, type Editor } from "@tiptap/react";
import { Bold, Code, Italic, Link2, Link2Off, List, ListOrdered } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { textExtensions } from "@/text/extensions";
import { linkFromInput, parseMarkdown, serializeMarkdown } from "@/text/markdown";
import { cn } from "@/lib/utils";

export interface MarkdownEditorProps {
  /** Stored Markdown. */
  value: string;
  /**
   * Called with normalized Markdown after each edit. Returns false if the
   * edit was refused, which puts the editor back to `value`.
   */
  onChange(markdown: string): boolean;
  /** Undo and redo from the native Edit menu (⌘Z keys are handled globally). */
  onUndo(): void;
  onRedo(): void;
  readOnly: boolean;
  /** A one-line field (titles): no lists or line breaks. */
  singleLine?: boolean;
  placeholder?: string;
  label: string;
  className?: string;
}

function ToolbarButton({
  label,
  active,
  disabled,
  onClick,
  children,
}: {
  label: string;
  active?: boolean;
  disabled?: boolean;
  onClick(): void;
  children: ReactNode;
}) {
  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label={label}
      title={label}
      aria-pressed={active}
      disabled={disabled}
      className={cn(active && "bg-accent text-accent-foreground")}
      // Keep the focus (and selection) in the editor.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
    >
      {children}
    </Button>
  );
}

function LinkForm({ editor, onDone }: { editor: Editor; onDone(): void }) {
  const [input, setInput] = useState(() => String(editor.getAttributes("link").href ?? ""));
  const [error, setError] = useState<string | null>(null);

  const close = () => {
    onDone();
    editor.commands.focus();
  };
  const apply = () => {
    const href = linkFromInput(input);
    if (!href) {
      setError("Use an http, https or mailto link");
      return;
    }
    const chain = editor.chain().focus().extendMarkRange("link");
    if (editor.state.selection.empty && !editor.isActive("link")) {
      // Nothing selected: insert the address itself as the link text.
      chain.insertContent({ type: "text", text: input.trim(), marks: [{ type: "link", attrs: { href } }] }).run();
    } else {
      chain.setLink({ href }).run();
    }
    onDone();
  };

  return (
    <form
      className="flex flex-col gap-1"
      onSubmit={(e) => {
        e.preventDefault();
        apply();
      }}
    >
      <div className="flex items-center gap-1">
        <Input
          autoFocus
          aria-label="Link address"
          placeholder="https://example.com"
          value={input}
          aria-invalid={error !== null}
          onChange={(e) => {
            setInput(e.target.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              close();
            }
          }}
          onBlur={(e) => {
            // Leaving the form (not for one of its buttons) cancels it.
            if (!e.currentTarget.form?.contains(e.relatedTarget)) onDone();
          }}
          className="h-7"
        />
        <Button size="sm" type="submit">
          Apply
        </Button>
        <Button size="sm" variant="ghost" onClick={close}>
          Cancel
        </Button>
      </div>
      {error && <p className="text-xs text-destructive">{error}</p>}
    </form>
  );
}

function Toolbar({ editor, singleLine, onLink }: { editor: Editor; singleLine: boolean; onLink(): void }) {
  const state = useEditorState({
    editor,
    selector: ({ editor }) => ({
      bold: editor.isActive("bold"),
      italic: editor.isActive("italic"),
      code: editor.isActive("code"),
      link: editor.isActive("link"),
      bulletList: editor.isActive("bulletList"),
      orderedList: editor.isActive("orderedList"),
    }),
  });
  const run = (command: (chain: ReturnType<Editor["chain"]>) => ReturnType<Editor["chain"]>) => () =>
    command(editor.chain().focus()).run();
  return (
    <div className="flex items-center gap-0.5" role="toolbar" aria-label="Formatting">
      <ToolbarButton label="Bold (⌘B)" active={state.bold} onClick={run((c) => c.toggleBold())}>
        <Bold />
      </ToolbarButton>
      <ToolbarButton label="Italic (⌘I)" active={state.italic} onClick={run((c) => c.toggleItalic())}>
        <Italic />
      </ToolbarButton>
      <ToolbarButton label="Code (⌘E)" active={state.code} onClick={run((c) => c.toggleCode())}>
        <Code />
      </ToolbarButton>
      {state.link ? (
        <ToolbarButton label="Remove link" active onClick={run((c) => c.extendMarkRange("link").unsetLink())}>
          <Link2Off />
        </ToolbarButton>
      ) : (
        <ToolbarButton label="Link (⌘K)" onClick={onLink}>
          <Link2 />
        </ToolbarButton>
      )}
      {!singleLine && (
        <>
          <ToolbarButton label="Bulleted list" active={state.bulletList} onClick={run((c) => c.toggleBulletList())}>
            <List />
          </ToolbarButton>
          <ToolbarButton
            label="Numbered list"
            active={state.orderedList}
            onClick={run((c) => c.toggleOrderedList())}
          >
            <ListOrdered />
          </ToolbarButton>
        </>
      )}
    </div>
  );
}

/**
 * A TipTap editor for one Markdown text field. The editor has no history of
 * its own: every edit is reported through `onChange` (a store command), and
 * undo or redo come back as a new `value`.
 */
export default function MarkdownEditor({
  value,
  onChange,
  onUndo,
  onRedo,
  readOnly,
  singleLine = false,
  placeholder,
  label,
  className,
}: MarkdownEditorProps) {
  // The Markdown the editor content stands for, to tell our own edits from
  // outside changes (undo, redo, AI).
  const shown = useRef(value);
  const handlers = useRef({ onChange, onUndo, onRedo });
  handlers.current = { onChange, onUndo, onRedo };
  const [linking, setLinking] = useState(false);
  const extensions = useMemo(() => textExtensions({ singleLine, placeholder }), [singleLine, placeholder]);
  // Only the first render's content is used; later values arrive below.
  const [initialContent] = useState(() => parseMarkdown(value, { singleLine }));

  const editor = useEditor({
    extensions,
    content: initialContent,
    editable: !readOnly,
    editorProps: {
      attributes: {
        "aria-label": label,
        role: "textbox",
        ...(singleLine ? {} : { "aria-multiline": "true" }),
        class: cn(
          "markdown-text min-w-0 rounded-md px-2.5 py-1.5 outline-none",
          singleLine ? "whitespace-nowrap" : "min-h-16",
        ),
      },
      handleKeyDown: (_view, event) => {
        if ((event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "k") {
          event.preventDefault();
          setLinking(true);
          return true;
        }
        return false;
      },
      handleDOMEvents: {
        // Links are never followed, editable or not: a followed link would
        // replace the app in its window. (The Rust side also blocks it.)
        click: (_view, event) => {
          if (event.target instanceof Element && event.target.closest("a")) event.preventDefault();
          return false;
        },
        auxclick: (_view, event) => {
          if (event.target instanceof Element && event.target.closest("a")) event.preventDefault();
          return false;
        },
        // Edit > Undo in the native menu arrives here, not as a key press.
        beforeinput: (_view, event) => {
          if (event.inputType !== "historyUndo" && event.inputType !== "historyRedo") return false;
          event.preventDefault();
          if (event.inputType === "historyUndo") handlers.current.onUndo();
          else handlers.current.onRedo();
          return true;
        },
      },
    },
    onUpdate: ({ editor }) => {
      const markdown = serializeMarkdown(editor.getJSON());
      if (markdown === shown.current) return;
      const previous = shown.current;
      shown.current = markdown;
      if (!handlers.current.onChange(markdown)) {
        shown.current = previous;
        editor.commands.setContent(parseMarkdown(previous, { singleLine }), { emitUpdate: false });
      }
    },
  });

  useEffect(() => {
    editor.setEditable(!readOnly, false);
  }, [editor, readOnly]);

  // Outside changes replace the content, keeping the cursor where it was.
  useEffect(() => {
    if (value === shown.current) return;
    shown.current = value;
    const { from, to } = editor.state.selection;
    editor.commands.setContent(parseMarkdown(value, { singleLine }), { emitUpdate: false });
    if (editor.isFocused) {
      const end = editor.state.doc.content.size;
      editor.commands.setTextSelection({ from: Math.min(from, end), to: Math.min(to, end) });
    }
  }, [editor, value, singleLine]);

  return (
    <div
      className={cn(
        "group flex flex-col gap-1 rounded-md border border-input focus-within:ring-2 focus-within:ring-ring",
        readOnly && "opacity-60",
        className,
      )}
    >
      <EditorContent editor={editor} className={cn(singleLine && "overflow-x-auto")} />
      {!readOnly && (
        <div className={cn("border-t border-border px-1 py-1", !linking && "hidden group-focus-within:block")}>
          {linking ? (
            <LinkForm editor={editor} onDone={() => setLinking(false)} />
          ) : (
            <Toolbar editor={editor} singleLine={singleLine} onLink={() => setLinking(true)} />
          )}
        </div>
      )}
    </div>
  );
}
