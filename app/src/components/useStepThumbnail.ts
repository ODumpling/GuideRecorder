import { useEffect, useState } from "react";
import { tauriApi } from "@/lib/ipc";
import { makeThumbnail } from "@/recording/thumbnails";

/** Longest side of an outline thumbnail, in CSS pixels (times the pixel ratio). */
const SIDE = 120;
/** Thumbnails kept; the oldest go first. */
const MAX = 300;

// Scaled-down step images by guide and path: the outline shows many at
// once, and re-renders on every edit.
const thumbnails = new Map<string, Promise<string | null>>();

function load(guideId: string, path: string): Promise<string | null> {
  const key = `${guideId}/${path}`;
  let thumbnail = thumbnails.get(key);
  if (!thumbnail) {
    thumbnail = tauriApi
      .stepImage(guideId, path)
      .then((bytes) => makeThumbnail(bytes, SIDE * Math.min(2, window.devicePixelRatio || 1)))
      .catch(() => null);
    thumbnails.set(key, thumbnail);
    if (thumbnails.size > MAX) {
      const [oldest, url] = thumbnails.entries().next().value!;
      thumbnails.delete(oldest);
      void url.then((u) => u && URL.revokeObjectURL(u));
    }
  }
  return thumbnail;
}

/** A small blob URL of a step image of the open guide; null while loading or if it fails. */
export function useStepThumbnail(guideId: string, path: string | null): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    setUrl(null);
    if (!path) return;
    let gone = false;
    void load(guideId, path).then((u) => !gone && setUrl(u));
    return () => {
      gone = true;
    };
  }, [guideId, path]);
  return url;
}
