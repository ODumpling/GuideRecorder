import { useEffect, useState } from "react";
import { errorMessage, tauriApi } from "@/lib/ipc";

type ImageState = { url: string | null; error: string | null };

/**
 * Loads a step image (a bundle-relative path from the editor's copy of the
 * guide) as a blob URL, revoked when it changes or unmounts.
 */
export function useStepImage(guideId: string, path: string): ImageState {
  // Keyed by what was loaded, so a changed path never returns the previous
  // image's URL (e.g. the full screenshot's, sized against a crop's rect).
  const key = `${guideId}|${path}`;
  const [state, setState] = useState<ImageState & { key: string | null }>({ key: null, url: null, error: null });

  useEffect(() => {
    let url: string | null = null;
    let cancelled = false;
    tauriApi
      .stepImage(guideId, path)
      .then((bytes) => {
        if (cancelled) return;
        url = URL.createObjectURL(new Blob([bytes], { type: "image/webp" }));
        setState({ key, url, error: null });
      })
      .catch((error) => {
        if (!cancelled) setState({ key, url: null, error: errorMessage(error) });
      });
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [guideId, path, key]);

  return state.key === key ? { url: state.url, error: state.error } : { url: null, error: null };
}
