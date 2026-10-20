// Steps from a screen recording: samples the video a few times a second as
// tiny frames, picks the screens worth a step (frames.ts), then grabs those
// moments at full size.

import { File } from "expo-file-system";
import { ImageManipulator, SaveFormat } from "expo-image-manipulator";
import { createVideoPlayer, type VideoPlayer } from "expo-video";
import { decode } from "jpeg-js";
import { pickSteps, sampleTimes, signature, type Signature } from "./frames";
import { saveStepImage, type Photo } from "./photos";

/** Width of the frames compared; enough to see a screen change, cheap to decode. */
const SAMPLE_WIDTH = 40;
const BATCH = 24;

export interface FoundStep extends Photo {
  /** Seconds into the recording. */
  time: number;
}

function waitUntilReady(player: VideoPlayer): Promise<void> {
  if (player.status === "readyToPlay") return Promise.resolve();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => done(new Error("The recording didn't open.")), 20_000);
    const sub = player.addListener("statusChange", ({ status, error }) => {
      if (status === "readyToPlay") done();
      else if (status === "error") done(new Error(error?.message ?? "The recording didn't open."));
    });
    function done(error?: Error) {
      clearTimeout(timer);
      sub.remove();
      if (error) reject(error);
      else resolve();
    }
  });
}

function bytesOf(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/** A frame's signature: the decoded image saved as a small JPEG and read back as pixels. */
async function frameSignature(image: Parameters<typeof ImageManipulator.manipulate>[0]): Promise<Signature> {
  const rendered = await ImageManipulator.manipulate(image).renderAsync();
  const saved = await rendered.saveAsync({ format: SaveFormat.JPEG, compress: 1, base64: true });
  new File(saved.uri).delete();
  const { width, height, data } = decode(bytesOf(saved.base64!), { useTArray: true, formatAsRGBA: true });
  return signature(data, width, height);
}

/**
 * Finds the steps in the recording at `uri`. `onProgress` gets 0–1 while
 * it looks; `cancelled` stops it early.
 */
export async function findSteps(
  uri: string,
  durationSeconds: number | null,
  onProgress: (fraction: number) => void,
  cancelled: () => boolean = () => false,
): Promise<FoundStep[]> {
  const player = createVideoPlayer({ uri });
  try {
    await waitUntilReady(player);
    const times = sampleTimes(player.duration || durationSeconds || 0);
    const frames: Signature[] = [];
    for (let i = 0; i < times.length; i += BATCH) {
      if (cancelled()) return [];
      const thumbs = await player.generateThumbnailsAsync(times.slice(i, i + BATCH), { maxWidth: SAMPLE_WIDTH });
      for (const thumb of thumbs) frames.push(await frameSignature(thumb));
      // Looking is most of the work; full-size frames are the last 10%.
      onProgress((0.9 * Math.min(i + BATCH, times.length)) / times.length);
    }
    const picked = pickSteps(frames).map((i) => times[i]!);
    const steps: FoundStep[] = [];
    for (let i = 0; i < picked.length; i += BATCH) {
      if (cancelled()) return [];
      const thumbs = await player.generateThumbnailsAsync(picked.slice(i, i + BATCH));
      for (const [j, thumb] of thumbs.entries()) {
        steps.push({ time: picked[i + j]!, ...(await saveStepImage(thumb, thumb.width, thumb.height)) });
      }
      onProgress(0.9 + (0.1 * Math.min(i + BATCH, picked.length)) / Math.max(picked.length, 1));
    }
    return steps;
  } finally {
    player.release();
  }
}
