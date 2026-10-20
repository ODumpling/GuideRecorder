// The guides kept on this phone: one folder per guide under Documents/guides,
// holding guide.json and its images at their bundle paths (steps/…,
// assets/…), like the desktop app's bundles. Guides here work offline; the
// ones the user syncs follow the web through sync.ts.

import type { Guide } from "@guiderecorder/guide-schema";
import { Directory, File, Paths } from "expo-file-system";
import { create } from "zustand";
import { ulid } from "./ulid";

const ROOT = new Directory(Paths.document, "guides");

function root(): Directory {
  if (!ROOT.exists) ROOT.create({ intermediates: true, idempotent: true });
  return ROOT;
}

const folder = (id: string) => new Directory(root(), id);

/** Images live in `steps/` or `assets/`, as the apps write them. */
export function checkImagePath(path: string): string[] {
  const parts = path.split("/");
  const ok = (parts[0] === "steps" || parts[0] === "assets") && parts.length >= 2 && parts.every((p) => p && p !== "." && p !== "..");
  if (!ok) throw new Error(`Unexpected image path ${path}`);
  return parts;
}

/** A guide's image on the phone (it may not be there yet). */
export function imageFile(guideId: string, path: string): File {
  return new File(folder(guideId), ...checkImagePath(path));
}

export function readGuide(id: string): Guide | null {
  const file = new File(folder(id), "guide.json");
  if (!file.exists) return null;
  try {
    return JSON.parse(file.textSync()) as Guide;
  } catch {
    return null;
  }
}

export function guideIds(): string[] {
  return root()
    .list()
    .filter((entry): entry is Directory => entry instanceof Directory && new File(entry, "guide.json").exists)
    .map((d) => d.name);
}

export function writeGuide(guide: Guide): void {
  const dir = folder(guide.id);
  if (!dir.exists) dir.create({ intermediates: true, idempotent: true });
  const file = new File(dir, "guide.json");
  if (!file.exists) file.create();
  file.write(JSON.stringify(guide, null, 2));
}

/** Copies a file on the phone into the guide at `path`. */
export async function putImage(guideId: string, path: string, uri: string): Promise<void> {
  const target = imageFile(guideId, path);
  const parent = target.parentDirectory;
  if (!parent.exists) parent.create({ intermediates: true, idempotent: true });
  if (target.exists) target.delete();
  await new File(uri).copy(target);
}

export function deleteGuide(id: string): void {
  const dir = folder(id);
  if (dir.exists) dir.delete();
}

/** Copies a guide, images and all, under a new id. */
export async function duplicateGuide(id: string, title: string): Promise<string> {
  const guide = readGuide(id);
  if (!guide) throw new Error("The guide isn't on this phone.");
  const copyId = ulid();
  await folder(id).copy(folder(copyId));
  const now = new Date().toISOString();
  writeGuide({ ...guide, id: copyId, title, createdAt: now, updatedAt: now });
  return copyId;
}

export interface LocalImage {
  /** Bundle path, e.g. `steps/<id>.jpg`. */
  path: string;
  /** A file on the device, e.g. a photo just taken. */
  uri: string;
}

interface DeviceStore {
  guides: Record<string, Guide>;
  reload(): void;
  save(guide: Guide, images?: LocalImage[]): Promise<void>;
  remove(id: string): void;
}

export const deviceStore = create<DeviceStore>((set) => ({
  guides: {},
  reload() {
    const guides: Record<string, Guide> = {};
    for (const id of guideIds()) {
      const guide = readGuide(id);
      if (guide) guides[id] = guide;
    }
    set({ guides });
  },
  async save(guide, images = []) {
    for (const image of images) await putImage(guide.id, image.path, image.uri);
    writeGuide(guide);
    set((s) => ({ guides: { ...s.guides, [guide.id]: guide } }));
  },
  remove(id) {
    deleteGuide(id);
    set((s) => {
      const { [id]: _, ...rest } = s.guides;
      return { guides: rest };
    });
  },
}));
