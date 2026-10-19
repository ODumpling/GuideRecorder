import { useEffect, useState } from "react";
import { guideCover } from "@/lib/ipc";
import { makeThumbnail } from "@/recording/thumbnails";

/** Longest side of a card's cover, in CSS pixels (times the pixel ratio). */
const COVER_SIDE = 480;

// One scaled-down cover per guide version, kept while the app runs: the
// full screenshot is several MB, and the library re-renders often.
const covers = new Map<string, Promise<string | null>>();

function loadCover(guideId: string, updatedAt: string): Promise<string | null> {
  const key = `${guideId}@${updatedAt}`;
  let cover = covers.get(key);
  if (!cover) {
    cover = guideCover(guideId)
      .then((bytes) =>
        bytes.byteLength ? makeThumbnail(bytes, COVER_SIDE * Math.min(2, window.devicePixelRatio || 1)) : null,
      )
      .catch(() => null);
    // An older version's cover is no longer shown.
    for (const [old, url] of covers) {
      if (old.startsWith(`${guideId}@`)) {
        covers.delete(old);
        void url.then((u) => u && URL.revokeObjectURL(u));
      }
    }
    covers.set(key, cover);
  }
  return cover;
}

/** A blob URL of the guide's first screenshot, scaled down; null while loading or if it has none. */
export function useGuideCover(guideId: string, updatedAt: string): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let gone = false;
    void loadCover(guideId, updatedAt).then((u) => !gone && setUrl(u));
    return () => {
      gone = true;
    };
  }, [guideId, updatedAt]);
  return url;
}
