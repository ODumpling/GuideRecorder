// The command palette (⌘K): guides, actions, editor commands, settings and
// tags in one list, searched as you type, from the library and the editor.
// Each command calls what its button or menu item calls.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  AppWindow,
  ArrowLeft,
  CheckCheck,
  ChevronRight,
  Copy,
  FileOutput,
  FilePlus2,
  FileText,
  FolderInput,
  GitBranch,
  Hash,
  Heading,
  Info,
  Lightbulb,
  ListOrdered,
  Monitor,
  Palette,
  Redo2,
  RefreshCw,
  ScanLine,
  Search,
  Settings as SettingsIcon,
  Sparkles,
  Square,
  Target,
  TriangleAlert,
  Undo2,
} from "lucide-react";
import { create } from "zustand";
import { aiStore, useAi } from "@/ai/ai";
import { openExport } from "@/components/ExportDialog";
import { guidesAt, selectionStore } from "@/components/library/selection";
import { itemLabel } from "@/components/Outline";
import { PAGES, type SettingsPage } from "@/components/settings/SettingsDialog";
import { REWRITES } from "@/components/StepView";
import { insertDecision, insertNote, insertPages, insertSection } from "@/editor/actions";
import { editorStore, stepNoun, stepNumbers, useEditor } from "@/editor/editor";
import { folderName, folderPaths, libraryTags, localToday } from "@/editor/library";
import { FORMATS } from "@/export/export";
import { tauriApi } from "@/lib/ipc";
import { shortcutLabel } from "@/lib/platform";
import type { CaptureArea, GuideListItem } from "@/lib/types";
import { parseQuery, rankEntries, recentFirst, recentGuideIds, rememberOpened, type PaletteEntry } from "@/palette/palette";
import { startFlow, useRecording } from "@/recording/recording";
import { settingsStore, useSettings } from "@/settings/settings";
import { THEMES } from "@/settings/theme";
import { formatHotkey } from "@/settings/validate";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@guiderecorder/ui/command";
import type { Step } from "@guiderecorder/guide-schema";
import { AI_ENABLED } from "@/lib/features";

/** How long typing pauses before the library search looks in step text. */
const STEP_SEARCH_DELAY_MS = 200;
/** The most results shown; typing more narrows them. */
const MAX_RESULTS = 60;

/**
 * Whether the palette is open; ⌘K toggles it. Each opening counts, so it
 * starts afresh (an empty search) even when reopened while still closing.
 */
const paletteStore = create<{ open: boolean; opened: number }>(() => ({ open: false, opened: 0 }));

function setOpen(open: boolean) {
  paletteStore.setState((s) => ({ open, opened: open && !s.open ? s.opened + 1 : s.opened }));
}

type Group = "Guides" | "Actions" | "Editor" | "AI" | "Settings" | "Tags";

interface Entry extends PaletteEntry {
  group: Group;
  icon: ReactNode;
  /** Dimmer text after the title: a guide's folder, what a command works on. */
  detail?: string;
  shortcut?: string;
  run?(): void;
  /** A list to pick from instead of running (folders, themes, formats, steps). */
  page?: { title: string; entries: Entry[] };
}

/** Commands that work on guides: the open guide in the editor, the selected ones in the library. */
function guideEntries(targets: GuideListItem[], inLibrary: boolean, folders: string[]): Entry[] {
  if (targets.length === 0) return [];
  const one = targets.length === 1 ? targets[0]! : null;
  const detail = inLibrary ? (one ? one.title || "Untitled" : `${targets.length} selected guides`) : undefined;
  const editor = () => editorStore.getState();
  const moveTo = (folder: string, label: string): Entry => ({
    id: `move:${folder}`,
    kind: "command",
    group: "Actions",
    icon: <FolderInput />,
    title: `Move to ${label}`,
    detail,
    searchOnly: true,
    run: () =>
      void (one
        ? editor().moveGuide(one, folder)
        : editor().moveGuides(
            targets.filter((g) => g.folder !== folder),
            folder,
          )),
  });
  const moves = [
    moveTo("", "Library (top level)"),
    ...folders.map((folder) => moveTo(folder, folder.split("/").join(" › "))),
  ].filter((e) => targets.some((g) => `move:${g.folder}` !== e.id));
  const entries: Entry[] = [];
  if (targets.some((g) => g.lastReviewed !== localToday())) {
    entries.push({
      id: "mark-reviewed",
      kind: "command",
      group: "Actions",
      icon: <CheckCheck />,
      title: "Mark reviewed",
      detail,
      run: () => void (one ? editor().markReviewed(one) : editor().markGuidesReviewed(targets)),
    });
  }
  entries.push(
    {
      id: "duplicate",
      kind: "command",
      group: "Actions",
      icon: <Copy />,
      title: "Duplicate",
      keywords: ["copy"],
      detail,
      run: () => void (one ? editor().duplicateGuide(one) : editor().duplicateGuides(targets)),
    },
    {
      id: "move",
      kind: "command",
      group: "Actions",
      icon: <FolderInput />,
      title: "Move to folder…",
      detail,
      page: { title: "Move to folder", entries: moves },
    },
    ...moves,
  );
  return entries;
}

/** The editor's commands: steps to jump to, inserts, AI, undo and redo. */
function useEditorEntries(): Entry[] {
  const doc = useEditor((s) => s.doc);
  const selectedId = useEditor((s) => s.selectedId);
  const canUndo = useEditor((s) => s.undoStack.length > 0 && !s.doc?.readOnly);
  const canRedo = useEditor((s) => s.redoStack.length > 0 && !s.doc?.readOnly);
  const aiIdle = useAi((s) => s.phase === "idle" && s.explicit === null);
  const aiRunning = useAi((s) => s.phase !== "idle" && s.explicit === null);
  if (!doc) return [];
  const annotated = doc.guide.kind === "annotated";
  const noun = stepNoun(annotated);
  const Noun = annotated ? "Page" : "Step";
  const numbers = stepNumbers(doc.guide.items);
  const steps = doc.guide.items.filter((item): item is Step => item.type === "step");
  const editor = editorStore;
  const entry = (id: string, title: string, icon: ReactNode, run: () => void, fields: Partial<Entry> = {}): Entry => ({
    id,
    kind: "command",
    group: "Editor",
    title,
    icon,
    run,
    ...fields,
  });

  const jumps = steps.map((step) => {
    const n = numbers.get(step.id)!;
    return entry(`step:${step.id}`, `${Noun} ${n}: ${itemLabel(step, annotated)}`, <ListOrdered />, () => editor.getState().select(step.id), {
      keywords: [`${noun} ${n}`, String(n)],
      searchOnly: true,
    });
  });
  const entries: Entry[] = [];
  if (jumps.length > 0) {
    entries.push(entry("go-to-step", `Go to ${noun}…`, <ListOrdered />, () => {}, { page: { title: `Go to ${noun}`, entries: jumps } }));
    entries.push(...jumps);
  }
  if (!doc.readOnly) {
    if (annotated) entries.push(entry("add-pages", "Add pages…", <FilePlus2 />, () => void insertPages(editor)));
    entries.push(
      entry("insert-section", "Insert section above", <Heading />, () => insertSection(editor)),
      entry("insert-tip", "Insert tip below", <Lightbulb />, () => insertNote(editor, "tip")),
      entry("insert-warning", "Insert warning below", <TriangleAlert />, () => insertNote(editor, "warning")),
      entry("insert-note", "Insert note below", <Info />, () => insertNote(editor, "note")),
      entry("insert-decision", "Insert decision below", <GitBranch />, () => insertDecision(editor)),
    );
  }
  if (canUndo) entries.push(entry("undo", "Undo", <Undo2 />, () => editor.getState().undo(), { shortcut: shortcutLabel("⌘Z") }));
  if (canRedo) entries.push(entry("redo", "Redo", <Redo2 />, () => editor.getState().redo(), { shortcut: shortcutLabel("⇧⌘Z") }));
  const formats = FORMATS.map(({ format, label, detail }) =>
    entry(`export:${format}`, `Export as ${label}`, <FileOutput />, () => openExport(format), {
      group: "Actions",
      detail,
      searchOnly: true,
    }),
  );
  entries.push(
    entry("export", "Export as…", <FileOutput />, () => {}, { group: "Actions", page: { title: "Export as", entries: formats } }),
    ...formats,
  );
  entries.push(
    entry(
      "library",
      "Back to library",
      <ArrowLeft />,
      async () => {
        if (await editor.getState().closeGuide()) await editor.getState().loadGuides();
      },
      { keywords: ["close guide"] },
    ),
  );

  // AI doesn't write annotation guides.
  if (AI_ENABLED && !annotated && !doc.readOnly) {
    if (aiIdle) {
      entries.push(entry("write-with-ai", "Write with AI", <Sparkles />, () => void aiStore.getState().start(), { group: "AI" }));
    } else if (aiRunning) {
      entries.push(entry("stop-ai", "Stop writing with AI", <Square />, () => void aiStore.getState().stop(), { group: "AI" }));
    }
    const step = steps.find((s) => s.id === selectedId);
    if (step && aiIdle) {
      const n = numbers.get(step.id)!;
      entries.push(
        entry("regenerate", `Regenerate step ${n}`, <RefreshCw />, () => void aiStore.getState().regenerateStep(step.id), {
          group: "AI",
          keywords: ["rewrite"],
        }),
        ...REWRITES.map(({ style, label, icon: Icon }) =>
          entry(`rewrite:${style.kind}`, `Rewrite step ${n} ${label.toLowerCase()}`, <Icon />, () => void aiStore.getState().rewriteStep(step.id, style), {
            group: "AI",
          }),
        ),
      );
    }
  }
  return entries;
}

/** Everything the palette can list, in the order shown before anything is typed. */
function useEntries(all: GuideListItem[], onSettings: (page?: SettingsPage) => void): Entry[] {
  const doc = useEditor((s) => s.doc);
  const folders = useEditor((s) => s.folders);
  const canCreate = useEditor((s) => !s.creating && !s.busy && !s.opening);
  const selected = selectionStore((s) => s.paths);
  const idle = useRecording((s) => s.status.state === "idle" && s.pending === null);
  const hotkey = useSettings((s) => s.settings?.recording.hotkeys.record);
  const theme = useSettings((s) => s.settings?.general.theme);
  const editorEntries = useEditorEntries();

  const recent = useMemo(() => recentFirst(all, recentGuideIds()), [all]);
  const guides: Entry[] = recent.map((g, i) => ({
    id: `guide:${g.path}`,
    kind: "guide",
    group: "Guides",
    icon: g.kind === "annotated" ? <Target /> : <FileText />,
    title: g.title || "Untitled",
    keywords: [...g.tags, g.summary, g.folder].filter(Boolean),
    detail: g.folder ? folderName(g.folder) : undefined,
    recency: i,
    run: () => void editorStore.getState().openGuide(g.id),
  }));

  const record = (id: string, title: string, icon: ReactNode, area: CaptureArea, shortcut?: string): Entry => ({
    id,
    kind: "command",
    group: "Actions",
    icon,
    title,
    keywords: ["new recording", "capture"],
    shortcut,
    run: () => void startFlow.getState().begin(area),
  });
  const actions: Entry[] = [];
  if (idle) {
    actions.push(
      record("record-screen", "Record screen", <Monitor />, "display", hotkey ? formatHotkey(hotkey) : undefined),
      record("record-window", "Record a window", <AppWindow />, "window"),
      record("record-region", "Record a region", <ScanLine />, "region"),
    );
  }
  if (canCreate) {
    actions.push({
      id: "new-annotation-guide",
      kind: "command",
      group: "Actions",
      icon: <Target />,
      title: "New annotation guide",
      keywords: ["images", "pdf"],
      run: () => void editorStore.getState().createAnnotatedGuide(),
    });
  }
  const open = doc ? all.find((g) => g.id === doc.guide.id && g.path === doc.path) : undefined;
  const targets = doc ? (open ? [open] : []) : guidesAt(all, selected);

  const themes: Entry[] = THEMES.map(({ value, label }) => ({
    id: `theme:${value}`,
    kind: "command",
    group: "Settings",
    icon: <Palette />,
    title: `Theme: ${label}`,
    detail: value === theme ? "Current" : undefined,
    searchOnly: true,
    run: () => void settingsStore.getState().change((s) => ({ ...s, general: { ...s.general, theme: value } })),
  }));
  const settings: Entry[] = [
    {
      id: "settings",
      kind: "command",
      group: "Settings",
      icon: <SettingsIcon />,
      title: "Settings",
      keywords: ["preferences"],
      shortcut: shortcutLabel("⌘,"),
      run: () => onSettings(),
    },
    ...PAGES.map(({ page, label, icon: Icon }): Entry => ({
      id: `settings:${page}`,
      kind: "command",
      group: "Settings",
      icon: <Icon />,
      title: `Settings: ${label}`,
      searchOnly: true,
      run: () => onSettings(page),
    })),
    ...(theme
      ? [
          {
            id: "theme",
            kind: "command",
            group: "Settings",
            icon: <Palette />,
            title: "Switch theme…",
            keywords: ["appearance", "dark mode"],
            page: { title: "Switch theme", entries: themes },
          } satisfies Entry,
          ...themes,
        ]
      : []),
  ];

  const tags: Entry[] = libraryTags(all).map((tag) => ({
    id: `tag:${tag}`,
    kind: "tag",
    group: "Tags",
    icon: <Hash />,
    title: tag,
    run: async () => {
      const editor = editorStore.getState();
      if (editor.doc && !(await editor.closeGuide())) return;
      editorStore.getState().setTag(tag);
    },
  }));

  return [
    ...guides,
    ...actions,
    ...guideEntries(targets, !doc, folderPaths(folders, all)),
    ...editorEntries,
    ...settings,
    ...tags,
  ];
}

/** Ids of the guides the library search finds for `text` (it also looks in step text). */
function useStepMatches(text: string, enabled: boolean): ReadonlySet<string> | undefined {
  const [found, setFound] = useState<{ text: string; ids: Set<string> } | null>(null);
  useEffect(() => {
    if (!enabled || text.length < 2) return;
    let gone = false;
    const timer = setTimeout(() => {
      tauriApi.listGuides(text).then(
        (guides) => !gone && setFound({ text, ids: new Set(guides.map((g) => g.id)) }),
        (error: unknown) => console.error("palette search failed:", error),
      );
    }, STEP_SEARCH_DELAY_MS);
    return () => {
      gone = true;
      clearTimeout(timer);
    };
  }, [text, enabled]);
  return found?.text === text ? found.ids : undefined;
}

/** Search entries carry the guide id in `id` as `guide:<path>`; step matches are by guide id. */
function stepMatchIds(all: GuideListItem[], ids: ReadonlySet<string> | undefined): ReadonlySet<string> | undefined {
  return ids && new Set(all.filter((g) => ids.has(g.id)).map((g) => `guide:${g.path}`));
}

/** Groups in the order their first result is shown. */
function grouped(entries: Entry[]): [Group, Entry[]][] {
  const groups = new Map<Group, Entry[]>();
  for (const e of entries) {
    const list = groups.get(e.group);
    if (list) list.push(e);
    else groups.set(e.group, [e]);
  }
  return [...groups];
}

function PaletteResults({ onSettings, onClose }: { onSettings(page?: SettingsPage): void; onClose(): void }) {
  const guides = useEditor((s) => s.guides);
  const libraryQuery = useEditor((s) => s.query);
  // The library list is narrowed while its search is on; the palette searches every guide.
  const [library, setLibrary] = useState<GuideListItem[] | null>(null);
  useEffect(() => {
    if (!libraryQuery) return;
    let gone = false;
    tauriApi.listGuides().then(
      (all) => !gone && setLibrary(all),
      (error: unknown) => console.error("palette list failed:", error),
    );
    return () => {
      gone = true;
    };
  }, [libraryQuery]);
  const all = (libraryQuery && library) || guides;

  const [search, setSearch] = useState("");
  const [pages, setPages] = useState<{ title: string; entries: Entry[] }[]>([]);
  const page = pages.at(-1);
  const entries = useEntries(all, onSettings);
  const { mode, text } = parseQuery(search);
  const found = useStepMatches(text, !page && mode === "all");
  const stepMatches = useMemo(() => stepMatchIds(all, found), [all, found]);
  const results = rankEntries(page ? page.entries.map((e) => ({ ...e, searchOnly: false })) : entries, page ? text : search, {
    stepMatches,
  }).slice(0, MAX_RESULTS);

  const choose = (entry: Entry) => {
    if (entry.page) {
      setPages([...pages, entry.page]);
      setSearch("");
      return;
    }
    onClose();
    entry.run?.();
  };
  const searchLibrary = async () => {
    onClose();
    const editor = editorStore.getState();
    if (editor.doc && !(await editor.closeGuide())) return;
    await editorStore.getState().setQuery(text);
  };

  return (
    <>
      <CommandInput
        autoFocus
        placeholder={page ? `${page.title}…` : "Search guides and commands (> commands, # tags)"}
        value={search}
        onValueChange={setSearch}
        onKeyDown={(e) => {
          if (e.key === "Backspace" && !search && page) {
            e.preventDefault();
            setPages(pages.slice(0, -1));
          }
        }}
      />
      <CommandList className="max-h-[min(26rem,60vh)]">
        <CommandEmpty>No results</CommandEmpty>
        {grouped(results).map(([group, list]) => (
          <CommandGroup key={group} heading={page ? page.title : group}>
            {list.map((entry) => (
              <CommandItem key={entry.id} value={entry.id} onSelect={() => choose(entry)}>
                {entry.icon}
                <span className="truncate">{entry.title}</span>
                {entry.detail && <span className="min-w-0 truncate text-xs text-muted-foreground">{entry.detail}</span>}
                {entry.shortcut && <CommandShortcut>{entry.shortcut}</CommandShortcut>}
                {entry.page && <ChevronRight className="ml-auto" />}
              </CommandItem>
            ))}
          </CommandGroup>
        ))}
        {!page && mode === "all" && text && (
          <CommandGroup heading="Library">
            <CommandItem value="search-library" onSelect={() => void searchLibrary()}>
              <Search />
              <span className="truncate">Search the library for “{text}”</span>
              <CommandShortcut>{shortcutLabel("⌘F")}</CommandShortcut>
            </CommandItem>
          </CommandGroup>
        )}
      </CommandList>
    </>
  );
}

/**
 * The command palette, over the library or the editor. ⌘K (Ctrl+K) opens
 * and closes it; `>` narrows it to commands, `#` to tags, and Backspace
 * in an empty search goes back from a list such as the folders.
 */
export function CommandPalette({ onSettings }: { onSettings(page?: SettingsPage): void }) {
  const open = paletteStore((s) => s.open);
  const opened = paletteStore((s) => s.opened);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== "k" || !(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
      const open = paletteStore.getState().open;
      // Not over another dialog (Settings, export, a rename).
      if (!open && document.querySelector("[role=dialog]:not([data-state=closed])")) return;
      e.preventDefault();
      setOpen(!open);
    };
    window.addEventListener("keydown", onKeyDown);
    // The guides opened lately come first.
    const unsubscribe = editorStore.subscribe((state, prev) => {
      const id = state.doc?.guide.id;
      if (id && id !== prev.doc?.guide.id) rememberOpened(id);
    });
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      unsubscribe();
    };
  }, []);

  return (
    <CommandDialog
      open={open}
      onOpenChange={setOpen}
      title="Command palette"
      description="Open a guide, or run a command"
      className="sm:max-w-xl"
      commandProps={{ shouldFilter: false, loop: true, label: "Search guides and commands" }}
    >
      <PaletteResults key={opened} onSettings={onSettings} onClose={() => setOpen(false)} />
    </CommandDialog>
  );
}
