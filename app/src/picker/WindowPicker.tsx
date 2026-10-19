import { useEffect, useRef, useState } from "react";
import { Dialog } from "radix-ui";
import { AppWindow, LoaderCircle, Monitor, RotateCw, TriangleAlert, X } from "lucide-react";
import { useStore } from "zustand";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { cn } from "@guiderecorder/ui/utils";
import { filterWindows, windowSize, windowTitle } from "@/picker/windows";
import type { StartFlowStore } from "@/recording/startFlow";
import type { RecordingStore } from "@/recording/store";

/**
 * Asks which window to record, when the capture area is Window. Type to
 * filter; ↑/↓ move, Return records the highlighted window.
 */
export function WindowPicker({ flow, recording }: { flow: StartFlowStore; recording: RecordingStore }) {
  const picker = useStore(flow, (s) => s.windowPicker);
  const starting = useStore(recording, (s) => s.pending === "start");
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const list = useRef<HTMLUListElement>(null);
  const filter = useRef<HTMLInputElement>(null);
  const open = picker !== null;

  // A fresh filter each time it opens.
  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
    }
  }, [open]);

  const windows = picker?.windows ?? null;
  const shown = windows ? filterWindows(windows, query) : [];
  const current = shown[Math.min(active, shown.length - 1)] ?? null;

  // Back to the filter after a failed start, so the keys work again.
  const startError = picker?.startError ?? null;
  useEffect(() => {
    if (startError) filter.current?.focus();
  }, [startError]);

  useEffect(() => {
    list.current?.querySelector("[aria-selected=true]")?.scrollIntoView({ block: "nearest" });
  }, [current?.windowId]);

  const { chooseWindow, recordDisplay, reloadWindows, closeWindowPicker } = flow.getState();
  const choose = (windowId: number) => void chooseWindow(windowId);

  return (
    <Dialog.Root open={open} onOpenChange={(next) => !next && !starting && closeWindowPicker()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content
          onOpenAutoFocus={(e) => {
            // Typing filters at once.
            e.preventDefault();
            filter.current?.focus();
          }}
          className="fixed top-1/2 left-1/2 z-50 flex max-h-[80vh] w-[32rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 rounded-2xl border border-border bg-card p-5 text-card-foreground shadow-2xl">
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-base font-semibold">Choose a window to record</Dialog.Title>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close" disabled={starting}>
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">
            Type to filter the windows, use the arrow keys to move, and Return to start recording.
          </Dialog.Description>

          <div className="flex gap-2">
            <Input
              ref={filter}
              aria-label="Filter windows"
              placeholder="Filter by app or title"
              value={query}
              // Not `disabled`, which would drop the focus while a start runs.
              readOnly={starting}
              aria-disabled={starting}
              role="combobox"
              aria-expanded={shown.length > 0}
              aria-autocomplete="list"
              aria-controls="capture-windows"
              aria-activedescendant={current ? `capture-window-${current.windowId}` : undefined}
              onChange={(e) => {
                setQuery(e.target.value);
                setActive(0);
              }}
              onKeyDown={(e) => {
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  if (shown.length === 0) return;
                  const at = Math.min(active, shown.length - 1);
                  setActive(e.key === "ArrowDown" ? Math.min(at + 1, shown.length - 1) : Math.max(at - 1, 0));
                } else if (e.key === "Enter" && current && !starting) {
                  e.preventDefault();
                  choose(current.windowId);
                }
              }}
            />
            <Button
              variant="ghost"
              size="icon"
              aria-label="Reload windows"
              title="Reload windows"
              disabled={starting || windows === null}
              onClick={() => void reloadWindows()}
            >
              <RotateCw />
            </Button>
          </div>

          {picker?.startError && (
            <p role="alert" className="flex items-center gap-2 text-sm">
              <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
              {picker.startError} Choose another window.
            </p>
          )}

          {windows === null ? (
            <p role="status" className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
              <LoaderCircle className="size-4 animate-spin" /> Finding windows…
            </p>
          ) : picker?.loadError ? (
            <p role="alert" className="flex items-center gap-2 py-2 text-sm">
              <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
              Couldn't list the windows: {picker.loadError}
            </p>
          ) : windows.length === 0 ? (
            <p className="py-2 text-sm text-muted-foreground">No windows available.</p>
          ) : shown.length === 0 ? (
            <p className="py-2 text-sm text-muted-foreground">No windows match “{query}”.</p>
          ) : (
            <ul
              ref={list}
              id="capture-windows"
              role="listbox"
              aria-label="Windows"
              className="-mx-1 flex min-h-0 flex-col gap-0.5 overflow-y-auto"
            >
              {shown.map((w) => {
                const selected = w.windowId === current?.windowId;
                return (
                  <li
                    key={w.windowId}
                    id={`capture-window-${w.windowId}`}
                    role="option"
                    aria-selected={selected}
                    aria-disabled={starting}
                    className={cn(
                      "flex cursor-default items-center gap-3 rounded-md px-2 py-1.5 text-sm",
                      selected ? "bg-accent text-accent-foreground" : "hover:bg-muted",
                    )}
                    onMouseMove={() => setActive(shown.indexOf(w))}
                    // The filter keeps the focus.
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => !starting && choose(w.windowId)}
                  >
                    <AppWindow className="size-4 shrink-0 text-muted-foreground" />
                    <span className="flex min-w-0 flex-1 flex-col">
                      <span className="truncate font-medium">{w.appName}</span>
                      <span className="truncate text-xs text-muted-foreground">{windowTitle(w)}</span>
                    </span>
                    <span className="shrink-0 font-mono text-xs text-muted-foreground tabular-nums">{windowSize(w)}</span>
                  </li>
                );
              })}
            </ul>
          )}

          <div className="mt-1 flex items-center justify-between gap-2">
            {starting ? (
              <span role="status" className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <LoaderCircle className="size-3.5 animate-spin" /> Starting…
              </span>
            ) : (
              <span />
            )}
            <div className="flex gap-2">
              {windows !== null && (windows.length === 0 || picker?.loadError) && (
                <Button variant="outline" onClick={() => void recordDisplay()}>
                  <Monitor /> Record whole display instead
                </Button>
              )}
              <Dialog.Close asChild>
                <Button variant="outline" disabled={starting}>
                  Cancel
                </Button>
              </Dialog.Close>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
