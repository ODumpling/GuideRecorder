import { useEffect, useState } from "react";
import { tauriRecordingApi } from "@/lib/ipc";
import { makeThumbnail, THUMBNAIL_SIDE } from "@/recording/thumbnails";

/**
 * A thumbnail of the running recording's screenshot `id`, fetched and
 * scaled down once: its blob URL (revoked on unmount), and whether it
 * could not be had.
 */
export function useThumbnail(id: string): { url: string | null; failed: boolean } {
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let revoke: string | null = null;
    let gone = false;
    tauriRecordingApi
      .snapshotImage(id)
      .then((bytes) => makeThumbnail(bytes, THUMBNAIL_SIDE * Math.min(2, window.devicePixelRatio || 1)))
      .then((made) => {
        if (gone || !made) {
          if (made) URL.revokeObjectURL(made);
          else setFailed(true);
          return;
        }
        revoke = made;
        setUrl(made);
      })
      .catch(() => setFailed(true));
    return () => {
      gone = true;
      if (revoke) URL.revokeObjectURL(revoke);
    };
  }, [id]);
  return { url, failed };
}
