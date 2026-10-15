import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { FileOutput, ImagePlus, Lock, Redo2, Settings as SettingsIcon, TriangleAlert, Undo2, X } from "lucide-react";
import { ExportDialog } from "@/components/ExportDialog";
import { Inspector } from "@/components/Inspector";
import { LeaveDialog } from "@/components/LeaveDialog";
import { Onboarding } from "@/components/Onboarding";
import { LibrarySidebar } from "@/components/LibrarySidebar";
import { Outline } from "@/components/Outline";
import { SaveIndicator } from "@/components/SaveIndicator";
import { SettingsDialog } from "@/components/settings/SettingsDialog";
import { StepView } from "@/components/StepView";
import { useImageDrop } from "@/components/useImageDrop";
import { Button } from "@/components/ui/button";
import { editorStore, useEditor } from "@/editor/editor";
import { shortcutFor } from "@/editor/shortcuts";
import { QUIT_REQUESTED_EVENT, tauriSettingsApi } from "@/lib/ipc";
import { setupAtStartup, setupStore, useSetupStep } from "@/onboarding/onboarding";
import { settingsStore } from "@/settings/settings";

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

function Notice() {
  const notice = useEditor((s) => s.notice);
  if (!notice) return null;
  return (
    <div role="alert" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
      <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
      <span className="flex-1">{notice}</span>
      <Button variant="ghost" size="icon" aria-label="Dismiss" onClick={() => editorStore.getState().notify(null)}>
        <X />
      </Button>
    </div>
  );
}

function Toolbar({ onSettings }: { onSettings(): void }) {
  const doc = useEditor((s) => s.doc);
  const canUndo = useEditor((s) => s.undoStack.length > 0 && !s.doc?.readOnly);
  const canRedo = useEditor((s) => s.redoStack.length > 0 && !s.doc?.readOnly);
  const { undo, redo } = editorStore.getState();
  const [exporting, setExporting] = useState(false);
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
        {doc && (
          <>
            <Button variant="outline" size="sm" className="ml-1" onClick={() => setExporting(true)}>
              <FileOutput /> Export…
            </Button>
            <ExportDialog open={exporting} onOpenChange={setExporting} />
          </>
        )}
        <Button variant="ghost" size="icon" aria-label="Settings" title="Settings (⌘,)" onClick={onSettings}>
          <SettingsIcon />
        </Button>
      </div>
    </header>
  );
}

export default function App() {
  const hasDoc = useEditor((s) => s.doc !== null);
  const draggingImages = useImageDrop();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const setupStep = useSetupStep();
  // Setup replaces Settings when Settings reopens it.
  useEffect(() => {
    if (setupStep) setSettingsOpen(false);
  }, [setupStep]);

  useEffect(() => {
    void editorStore.getState().loadGuides();
    void settingsStore
      .getState()
      .load()
      .then(() => setupAtStartup(tauriSettingsApi.libraryInfo, () => settingsStore.getState().settings))
      .then((step) => step && setupStore.getState().open(step));
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "," && (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey) {
        e.preventDefault();
        setSettingsOpen(true);
        return;
      }
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
        <Toolbar onSettings={() => setSettingsOpen(true)} />
        <ReadOnlyBanner />
        <Notice />
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
      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
      <Onboarding />
      {draggingImages && (
        <div className="pointer-events-none fixed inset-3 flex items-center justify-center rounded-xl border-2 border-dashed border-[var(--status-info)] bg-background/70">
          <span className="flex items-center gap-2 text-sm font-medium">
            <ImagePlus className="size-5" /> Drop images to add them as steps
          </span>
        </div>
      )}
    </div>
  );
}
