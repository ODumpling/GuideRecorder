import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Lock, Redo2, Undo2 } from "lucide-react";
import { Inspector } from "@/components/Inspector";
import { LeaveDialog } from "@/components/LeaveDialog";
import { LibrarySidebar } from "@/components/LibrarySidebar";
import { Outline } from "@/components/Outline";
import { SaveIndicator } from "@/components/SaveIndicator";
import { StepView } from "@/components/StepView";
import { Button } from "@/components/ui/button";
import { editorStore, useEditor } from "@/editor/editor";
import { shortcutFor } from "@/editor/shortcuts";
import { QUIT_REQUESTED_EVENT } from "@/lib/ipc";

function ReadOnlyBanner() {
  const doc = useEditor((s) => s.doc);
  if (!doc?.readOnly) return null;
  return (
    <div className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
      <Lock className="size-4 shrink-0 text-[var(--status-warning)]" />
      <span>
        {doc.migrationError
          ? `This guide was upgraded but could not be saved (${doc.migrationError}). It is read-only.`
          : "This guide was made with a newer version of GuidesYa. It is read-only."}
      </span>
    </div>
  );
}

function Toolbar() {
  const doc = useEditor((s) => s.doc);
  const canUndo = useEditor((s) => s.undoStack.length > 0 && !s.doc?.readOnly);
  const canRedo = useEditor((s) => s.redoStack.length > 0 && !s.doc?.readOnly);
  const { undo, redo } = editorStore.getState();
  return (
    <header className="flex h-11 shrink-0 items-center gap-2 border-b border-border px-3">
      <span className="truncate text-sm font-medium">{doc ? doc.guide.title || "Untitled" : "GuidesYa"}</span>
      {doc?.migratedFrom != null && !doc.migrationError && (
        <span className="text-xs text-muted-foreground">upgraded from schema v{doc.migratedFrom}</span>
      )}
      <div className="ml-auto flex items-center gap-1">
        {doc && <SaveIndicator />}
        <Button variant="ghost" size="icon" aria-label="Undo" title="Undo (⌘Z)" disabled={!canUndo} onClick={undo}>
          <Undo2 />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          aria-label="Redo"
          title="Redo (⇧⌘Z)"
          disabled={!canRedo}
          onClick={redo}
        >
          <Redo2 />
        </Button>
      </div>
    </header>
  );
}

export default function App() {
  const hasDoc = useEditor((s) => s.doc !== null);

  useEffect(() => {
    void editorStore.getState().loadGuides();
    const onKeyDown = (e: KeyboardEvent) => {
      const action = shortcutFor(e);
      if (!action || !editorStore.getState().doc) return;
      // Our history replaces the inputs' own undo, which would fight the store.
      e.preventDefault();
      editorStore.getState()[action]();
    };
    window.addEventListener("keydown", onKeyDown);

    // Closing the window or Cmd+Q: save first. `requestQuit` exits the app
    // once saved, or shows the leave dialog if the save failed.
    const quit = () => void editorStore.getState().requestQuit();
    const unlisten = [
      getCurrentWindow().onCloseRequested((e) => {
        e.preventDefault();
        quit();
      }),
      listen(QUIT_REQUESTED_EVENT, () => {
        void invoke("quit_ack");
        quit();
      }),
    ];
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      for (const p of unlisten) void p.then((stop) => stop());
    };
  }, []);

  return (
    <div className="flex h-screen bg-background text-foreground">
      <aside className="flex w-72 shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground">
        {hasDoc ? <Outline /> : <LibrarySidebar />}
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <Toolbar />
        <ReadOnlyBanner />
        <div className="flex min-h-0 flex-1">
          <main className="flex min-w-0 flex-1 overflow-y-auto">
            <StepView />
          </main>
          {hasDoc && (
            <aside className="w-72 shrink-0 overflow-y-auto border-l border-border">
              <Inspector />
            </aside>
          )}
        </div>
      </div>
      <LeaveDialog />
    </div>
  );
}
