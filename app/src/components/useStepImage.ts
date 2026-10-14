import { useEffect, useState } from "react";
import { errorMessage, tauriApi } from "@/lib/ipc";
import type { ImageVariant } from "@/lib/types";

type ImageState = { url: string | null; error: string | null };

/** Loads a step image as a blob URL, revoked when it changes or unmounts. */
export function useStepImage(guideId: string, stepId: string, variant: ImageVariant): ImageState {
  const [state, setState] = useState<ImageState>({ url: null, error: null });

  useEffect(() => {
    let url: string | null = null;
    let cancelled = false;
    setState({ url: null, error: null });
    tauriApi
      .stepImage(guideId, stepId, variant)
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
  }, [guideId, stepId, variant]);

  return state;
}
