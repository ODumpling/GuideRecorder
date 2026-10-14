import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { insertImages } from "@/editor/actions";
import { editorStore } from "@/editor/editor";
import { isImagePath, readDroppedImage } from "@/editor/images";

/** True inside a text field or editor, where paste means text. */
function isEditable(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (target.isContentEditable || !!target.closest("input, textarea, [contenteditable]"));
}

/**
 * Image files dropped on the window, or pasted outside text fields, become
 * manual steps below the selected item. Returns whether files are being
 * dragged over the window (for a drop hint).
 */
export function useImageDrop(): boolean {
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    const canInsert = () => {
      const doc = editorStore.getState().doc;
      return !!doc && !doc.readOnly;
    };
    // Tauri delivers file drops as paths, not as DOM drop events.
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const { payload } = event;
      if (payload.type === "enter" || payload.type === "over") {
        setDragging(canInsert());
        return;
      }
      setDragging(false);
      if (payload.type !== "drop" || !canInsert()) return;
      const paths = payload.paths.filter(isImagePath);
      if (paths.length < payload.paths.length) {
        editorStore.getState().notify("Only image files can be added as steps");
      }
      void insertImages(
        editorStore,
        paths.map((path) => () => readDroppedImage(path)),
      );
    });

    const onPaste = (e: ClipboardEvent) => {
      if (isEditable(e.target) || !canInsert()) return;
      const files = [...(e.clipboardData?.files ?? [])].filter((file) => file.type.startsWith("image/"));
      if (!files.length) return;
      e.preventDefault();
      void insertImages(editorStore, files);
    };
    window.addEventListener("paste", onPaste);

    return () => {
      void unlisten.then((stop) => stop());
      window.removeEventListener("paste", onPaste);
    };
  }, []);

  return dragging;
}
