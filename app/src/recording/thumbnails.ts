// Thumbnails of the screenshots a running recording has taken. The session
// encodes full-size lossless WebP (a Retina display is several MB and tens
// of millions of pixels decoded), so each is scaled down once, here, and
// only the small copy is kept.

/** Longest side of a thumbnail, in CSS pixels times the device pixel ratio. */
export const THUMBNAIL_SIDE = 160;

/**
 * `width`×`height` scaled down (never up) so the longer side is `side`.
 */
export function thumbnailSize(width: number, height: number, side = THUMBNAIL_SIDE): { width: number; height: number } {
  const scale = Math.min(1, side / Math.max(width, height, 1));
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

/**
 * A blob URL of a thumbnail of `bytes` (an image the webview can decode),
 * or null if it cannot be decoded. The caller revokes the URL.
 */
export async function makeThumbnail(bytes: ArrayBuffer, side = THUMBNAIL_SIDE): Promise<string | null> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(new Blob([bytes], { type: "image/webp" }));
  } catch {
    return null;
  }
  try {
    const { width, height } = thumbnailSize(bitmap.width, bitmap.height, side);
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext("2d");
    if (!context) return null;
    context.drawImage(bitmap, 0, 0, width, height);
    const png = await canvas.convertToBlob({ type: "image/png" });
    return URL.createObjectURL(png);
  } catch {
    return null;
  } finally {
    bitmap.close();
  }
}
