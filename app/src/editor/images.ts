// Manual steps from image files: the webview decodes the file (so anything
// it can show works: PNG, JPEG, HEIC, GIF, WebP…) and re-encodes it as PNG,
// Rust stores that as the step's WebP screenshot, and the step is inserted
// as an undoable command.

import { insertItemAfter } from "@/editor/commands";
import type { EditorStore } from "@/editor/store";
import { addImageStep, readImageFile } from "@/lib/ipc";
import type { Step } from "@/lib/types";

/** Core's `MAX_WEBP_DIMENSION`. */
export const MAX_SIDE = 16383;
/** Larger images are scaled down (Rust refuses more): canvases stay reasonable. */
export const MAX_PIXELS = 40_000_000;
/** Larger files are refused before decoding: their pixels alone would be gigabytes. */
export const MAX_SOURCE_PIXELS = 200_000_000;

/** File extensions offered for and accepted from dropped files. */
export const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "gif", "heic", "heif", "tif", "tiff", "bmp"];

export function isImagePath(path: string): boolean {
  const extension = /\.([^./\\]+)$/.exec(path)?.[1].toLowerCase();
  return extension !== undefined && IMAGE_EXTENSIONS.includes(extension);
}

/** `width`×`height` scaled down (never up) to fit the limits, keeping the shape. */
export function fitSize(width: number, height: number): { width: number; height: number } {
  const scale = Math.min(1, MAX_SIDE / width, MAX_SIDE / height, Math.sqrt(MAX_PIXELS / (width * height)));
  return {
    width: Math.max(1, Math.min(MAX_SIDE, Math.floor(width * scale))),
    height: Math.max(1, Math.min(MAX_SIDE, Math.floor(height * scale))),
  };
}

/**
 * An image's size, read without decoding its pixels: WebKit decodes an
 * `<img>` lazily, when it is drawn, but knows the size once it has loaded.
 */
function imageSize(blob: Blob): Promise<{ width: number; height: number }> {
  const url = URL.createObjectURL(blob);
  return new Promise<{ width: number; height: number }>((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve({ width: image.naturalWidth, height: image.naturalHeight });
    image.onerror = () => reject(new Error("This file is not an image the app can read"));
    image.src = url;
  }).finally(() => URL.revokeObjectURL(url));
}

/**
 * Re-encodes an image as PNG with the webview's decoders, scaled down to
 * {@link fitSize}. Images too large to decode safely are refused first.
 */
export async function toPng(blob: Blob): Promise<Uint8Array> {
  const source = await imageSize(blob);
  if (!source.width || !source.height) throw new Error("This file is not an image the app can read");
  if (source.width * source.height > MAX_SOURCE_PIXELS) {
    throw new Error(`The image is too large (${source.width}×${source.height})`);
  }
  const { width, height } = fitSize(source.width, source.height);
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(blob, { resizeWidth: width, resizeHeight: height, resizeQuality: "high" });
  } catch {
    throw new Error("This file is not an image the app can read");
  }
  try {
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext("2d");
    if (!context) throw new Error("Cannot convert images here");
    // Scales too, where resizeWidth/resizeHeight are not supported.
    context.drawImage(bitmap, 0, 0, width, height);
    const png = await canvas.convertToBlob({ type: "image/png" });
    return new Uint8Array(await png.arrayBuffer());
  } finally {
    bitmap.close();
  }
}

/** The bytes of a dropped file (Tauri hands the webview paths). */
export async function readDroppedImage(path: string): Promise<Blob> {
  return new Blob([await readImageFile(path)]);
}

/**
 * Turns images into manual steps inserted after `afterId` (or at the end),
 * in order, each as its own undoable command. Stops at the first failure,
 * leaving the steps added so far.
 */
export async function insertImageSteps(
  store: EditorStore,
  images: (Blob | (() => Promise<Blob>))[],
  afterId: string | null,
): Promise<Step[]> {
  const guideId = store.getState().doc?.guide.id;
  if (!guideId) return [];
  const added: Step[] = [];
  let after = afterId;
  for (const image of images) {
    const blob = typeof image === "function" ? await image() : image;
    const step = { ...(await addImageStep(guideId, await toPng(blob))), type: "step" } as Step;
    const { doc, dispatch } = store.getState();
    // The guide changed (closed or another opened) meanwhile.
    if (doc?.guide.id !== guideId) break;
    if (after !== null && !doc.guide.items.some((item) => item.id === after)) after = null;
    if (!dispatch(insertItemAfter(doc.guide, step, after))) break;
    added.push(step);
    after = step.id;
  }
  return added;
}
