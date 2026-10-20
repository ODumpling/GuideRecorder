// Images for steps: JPEG, at most MAX_SIDE pixels on the long side.

import { ImageManipulator, SaveFormat } from "expo-image-manipulator";

/** Photos and screenshots are scaled down to this many pixels on their long side before upload. */
export const MAX_SIDE = 2560;

export interface Photo {
  uri: string;
  width: number;
  height: number;
}

/** Saves an image (a file or a decoded image) as a step's JPEG. */
export async function saveStepImage(source: Parameters<typeof ImageManipulator.manipulate>[0], width: number, height: number): Promise<Photo> {
  const context = ImageManipulator.manipulate(source);
  if (Math.max(width, height) > MAX_SIDE) context.resize(width >= height ? { width: MAX_SIDE, height: null } : { width: null, height: MAX_SIDE });
  const rendered = await context.renderAsync();
  const saved = await rendered.saveAsync({ format: SaveFormat.JPEG, compress: 0.82 });
  return { uri: saved.uri, width: saved.width, height: saved.height };
}
