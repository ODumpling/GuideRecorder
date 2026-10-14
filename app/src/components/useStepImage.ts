import { useEffect, useState } from "react";
import { errorMessage, tauriApi } from "@/lib/ipc";

type ImageState = { url: string | null; error: string | null };

/**
 * Loads a step image (a bundle-relative path from the editor's copy of the
 * guide) as a blob URL, revoked when it changes or unmounts.
 */
export function useStepImage(guideId: string, path: string): ImageState {
  const [state, setState] = useState<ImageState>({ url: null, error: null });

  useEffect(() => {
    let url: string | null = null;
    let cancelled = false;
    setState({ url: null, error: null });
    tauriApi
      .stepImage(guideId, path)
      .then((bytes) => {
        if (cancelled) return;
        url = URL.createObjectURL(new Blob([bytes], { type: "image/webp" }));
        setState({ url, error: null });
      })
      .catch((error) => {
        if (!cancelled) setState({ url: null, error: errorMessage(error) });
      });
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [guideId, path]);

  return state;
}
