import { useEffect, useState } from "react";
import { Dialog } from "radix-ui";
import { Bot, FileOutput, Keyboard, Lock, Plug, ShieldCheck, SlidersHorizontal, TriangleAlert, X } from "lucide-react";
import {
  AiSection,
  ExportSection,
  GeneralSection,
  McpSection,
  PermissionsSection,
  RecordingSection,
} from "@/components/settings/sections";
import { Button } from "@/components/ui/button";
import { tauriExportApi, tauriSettingsApi, type ExportApi, type SettingsApi } from "@/lib/ipc";
import { settingsStore, useSettings } from "@/settings/settings";
import { cn } from "@/lib/utils";

export type SettingsPage = "general" | "recording" | "ai" | "export" | "permissions" | "mcp";

const PAGES: { page: SettingsPage; label: string; icon: typeof Bot }[] = [
  { page: "general", label: "General", icon: SlidersHorizontal },
  { page: "recording", label: "Recording", icon: Keyboard },
  { page: "ai", label: "AI", icon: Bot },
  { page: "export", label: "Export", icon: FileOutput },
  { page: "permissions", label: "Permissions", icon: ShieldCheck },
  { page: "mcp", label: "MCP", icon: Plug },
];

/** The Settings window: a sheet over the editor with one page per section. */
export function SettingsDialog({
  open,
  onOpenChange,
  initialPage = "general",
  api = tauriSettingsApi,
  exportApi = tauriExportApi,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
  initialPage?: SettingsPage;
  api?: SettingsApi;
  exportApi?: ExportApi;
}) {
  const [page, setPage] = useState<SettingsPage>(initialPage);
  const settings = useSettings((s) => s.settings);
  const readOnly = useSettings((s) => s.readOnly);
  const loadError = useSettings((s) => s.loadError);
  const error = useSettings((s) => s.error);

  useEffect(() => {
    if (!open) return;
    setPage(initialPage);
    // Pick up changes made elsewhere (another window, the login item).
    void settingsStore.getState().load();
  }, [open, initialPage]);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex h-[min(40rem,90vh)] w-[min(52rem,95vw)] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-lg border border-border bg-background text-foreground shadow-lg">
          <nav aria-label="Settings sections" className="flex w-44 shrink-0 flex-col gap-0.5 border-r border-sidebar-border bg-sidebar p-2">
            <Dialog.Title className="px-2 py-2 text-sm font-semibold">Settings</Dialog.Title>
            {PAGES.map(({ page: p, label, icon: Icon }) => (
              <button
                key={p}
                type="button"
                aria-current={page === p ? "page" : undefined}
                onClick={() => setPage(p)}
                className={cn(
                  "flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring",
                  page === p ? "bg-accent font-medium text-accent-foreground" : "hover:bg-accent/60",
                )}
              >
                <Icon className="size-4 shrink-0" /> {label}
              </button>
            ))}
          </nav>
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex items-center justify-end p-2">
              <Dialog.Close asChild>
                <Button variant="ghost" size="icon" aria-label="Close settings">
                  <X />
                </Button>
              </Dialog.Close>
            </div>
            <Dialog.Description className="sr-only">GuidesYa settings</Dialog.Description>
            <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-6">
              {readOnly && (
                <p className="mb-3 flex items-center gap-2 rounded-md bg-muted px-3 py-2 text-xs">
                  <Lock className="size-3.5 shrink-0" />
                  {loadError ??
                    "These settings were saved by a newer GuidesYa. They are shown but can't be changed here."}
                </p>
              )}
              {((loadError && !readOnly) || error) && (
                <p role="alert" className="mb-3 flex items-start gap-2 rounded-md bg-muted px-3 py-2 text-xs">
                  <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-[var(--status-danger)]" />
                  <span className="flex-1">{error ?? loadError}</span>
                  {error && (
                    <button type="button" aria-label="Dismiss" onClick={() => settingsStore.getState().clearError()}>
                      <X className="size-3.5" />
                    </button>
                  )}
                </p>
              )}
              {!settings ? (
                <p className="text-sm text-muted-foreground">{loadError ? "Settings could not be loaded." : "Loading…"}</p>
              ) : page === "general" ? (
                <GeneralSection api={api} />
              ) : page === "recording" ? (
                <RecordingSection />
              ) : page === "ai" ? (
                <AiSection api={api} />
              ) : page === "export" ? (
                <ExportSection exportApi={exportApi} />
              ) : page === "permissions" ? (
                <PermissionsSection api={api} />
              ) : (
                <McpSection api={api} />
              )}
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
