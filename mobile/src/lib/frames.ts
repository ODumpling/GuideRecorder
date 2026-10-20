// Finding the steps in a screen recording. iOS shows an app nothing of other
// apps, not even where you tapped, so the steps come from the picture alone:
// the recording is sampled a few times a second, and each screen that holds
// still for a moment and differs from the last step becomes a step, captured
// just before it changes (the moment the desktop app captures on a click).
// Pure: the frames come in as small grayscale pictures.

/** A sampled frame: a small grayscale picture, `width` × `height`, one byte a pixel. */
export interface Signature {
  width: number;
  height: number;
  data: Uint8Array;
}

/** Leaves out the status bar (its clock) and the home indicator, as fractions of the height. */
export interface Crop {
  top: number;
  bottom: number;
}

export const PHONE_CROP: Crop = { top: 0.07, bottom: 0.04 };

/** A frame's signature from RGBA pixels, without the cropped bands. */
export function signature(rgba: Uint8Array, width: number, height: number, crop: Crop = PHONE_CROP): Signature {
  const top = Math.floor(height * crop.top);
  const bottom = height - Math.floor(height * crop.bottom);
  const rows = Math.max(bottom - top, 1);
  const data = new Uint8Array(width * rows);
  for (let y = 0; y < rows; y++) {
    for (let x = 0; x < width; x++) {
      const i = ((y + top) * width + x) * 4;
      // Rec. 601 luma.
      data[y * width + x] = (rgba[i]! * 299 + rgba[i + 1]! * 587 + rgba[i + 2]! * 114) / 1000;
    }
  }
  return { width, height: rows, data };
}

/** How different two frames look: the mean absolute difference of their pixels, 0–255. */
export function difference(a: Signature, b: Signature): number {
  if (a.width !== b.width || a.height !== b.height) return 255;
  let total = 0;
  for (let i = 0; i < a.data.length; i++) total += Math.abs(a.data[i]! - b.data[i]!);
  return total / Math.max(a.data.length, 1);
}

export interface StepOptions {
  /** Frames closer than this are the same screen holding still. */
  stable: number;
  /** A step must differ from the previous step by more than this. */
  change: number;
  /** Samples a screen must hold for to count (animations and scrolling don't). */
  minRun: number;
}

export const DEFAULT_STEP_OPTIONS: StepOptions = { stable: 1.5, change: 5, minRun: 2 };

/**
 * The samples to make steps of: for each screen that held still for at
 * least `minRun` samples and differs enough from the previous step, its
 * last sample. The final screen always counts, however briefly it showed.
 */
export function pickSteps(frames: Signature[], options: StepOptions = DEFAULT_STEP_OPTIONS): number[] {
  if (!frames.length) return [];
  // Runs of near-identical consecutive frames: [first, last].
  const runs: [number, number][] = [[0, 0]];
  for (let i = 1; i < frames.length; i++) {
    const run = runs[runs.length - 1]!;
    if (difference(frames[i]!, frames[i - 1]!) <= options.stable) run[1] = i;
    else runs.push([i, i]);
  }
  const steps: number[] = [];
  runs.forEach(([first, last], n) => {
    const settled = last - first + 1 >= options.minRun || n === runs.length - 1;
    if (!settled) return;
    const previous = steps[steps.length - 1];
    if (previous === undefined || difference(frames[last]!, frames[previous]!) > options.change) steps.push(last);
  });
  return steps;
}

/** When to sample a recording `duration` seconds long: every `interval` seconds, at most `max` samples. */
export function sampleTimes(duration: number, interval = 0.5, max = 600): number[] {
  if (!(duration > 0)) return [0];
  const step = Math.max(interval, duration / (max - 1));
  const round = (t: number) => Math.round(t * 1000) / 1000;
  const times: number[] = [];
  for (let i = 0; i * step < duration - step / 2 && times.length < max - 1; i++) times.push(round(i * step));
  // The last frame, a little before the end so it decodes.
  times.push(round(Math.max(duration - 0.05, 0)));
  return times;
}
