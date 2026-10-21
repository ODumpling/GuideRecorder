// Recording into the open guide (#12): Re-capture a step, or Insert
// recorded steps here. The recording runs as any other (see
// recording/startFlow's `recordInto`); its steps come back in
// `RecordingSaved.recorded` and are added here, as one undoable command.

import { createStore, type StoreApi } from "zustand/vanilla";
import type { Step } from "@guiderecorder/guide-schema";
import { clickOffset, insertItemsAfter, recaptureStep, type AnnotationChoice } from "@/editor/commands";
import type { EditorStore } from "@/editor/store";
import type { RecordedSteps, RecordInto } from "@/lib/types";
import type { StartFlowStore } from "@/recording/startFlow";

/** A re-captured step, waiting for the user to say what happens to its annotations. */
export interface PendingRecapture {
  guideId: string;
  stepId: string;
  captured: Omit<Step, "type">;
  /** How far the click moved, in screenshot pixels; null: the annotations can't follow it. */
  offset: { dx: number; dy: number } | null;
  /** How many annotations the step has. */
  annotations: number;
}

export type RecaptureStore = StoreApi<{ pending: PendingRecapture | null }>;

export function createRecaptureStore(): RecaptureStore {
  return createStore(() => ({ pending: null as PendingRecapture | null }));
}

/** Whether steps can be recorded into the open guide: a writable step-by-step guide. */
export function canRecordInto(editor: EditorStore): boolean {
  const { doc, busy } = editor.getState();
  return !!doc && !doc.readOnly && !busy && doc.guide.kind !== "annotated";
}

/** Re-captures step `stepId` of the open guide, on the area the guide was recorded with. */
export async function startRecapture(editor: EditorStore, flow: StartFlowStore, stepId: string): Promise<void> {
  const doc = editor.getState().doc;
  if (!doc || !canRecordInto(editor)) return;
  await flow.getState().recordInto({ mode: "replace", guideId: doc.guide.id, stepId }, doc.guide.recording?.captureArea);
}

/**
 * Records steps into the open guide, after the item `afterId` (null: at the
 * end), on the area the guide was recorded with.
 */
export async function startInsert(editor: EditorStore, flow: StartFlowStore, afterId: string | null): Promise<void> {
  const doc = editor.getState().doc;
  if (!doc || !canRecordInto(editor)) return;
  await flow.getState().recordInto({ mode: "insert", guideId: doc.guide.id, afterId }, doc.guide.recording?.captureArea);
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/**
 * Adds what a recording into a guide made to the open guide: the steps
 * inserted after their item (at the end if it is gone), or the step
 * re-captured. A re-capture of a step with annotations waits in `pending`
 * for `resolveRecapture`. False, with a notice, when it no longer applies
 * (another guide is open, the step was deleted, nothing was recorded).
 */
export function applyRecorded(editor: EditorStore, pending: RecaptureStore, recorded: RecordedSteps): boolean {
  const { doc, notify } = editor.getState();
  const { into } = recorded;
  if (!doc || doc.guide.id !== into.guideId) {
    notify("The recording was for a guide that is no longer open, so it was not added.");
    return false;
  }
  if (recorded.steps.length === 0) {
    notify(into.mode === "replace" ? "Nothing was captured, so the step is unchanged." : "No steps were recorded.");
    return false;
  }
  if (into.mode === "insert") return insertRecorded(editor, recorded.steps, into);
  const step = doc.guide.items.find((item) => item.id === into.stepId);
  if (step?.type !== "step") {
    notify("The step was deleted while it was re-captured.");
    return false;
  }
  const captured = recorded.steps[0];
  if (step.annotations.length === 0) return editor.getState().dispatch(recaptureStep(doc.guide, step.id, captured));
  pending.setState({
    pending: {
      guideId: doc.guide.id,
      stepId: step.id,
      captured,
      offset: clickOffset(step, captured),
      annotations: step.annotations.length,
    },
  });
  return true;
}

function insertRecorded(editor: EditorStore, steps: Omit<Step, "type">[], into: Extract<RecordInto, { mode: "insert" }>) {
  const { doc, dispatch } = editor.getState();
  if (!doc) return false;
  const items = steps.map((step): Step => ({ ...step, type: "step" }));
  const after = into.afterId && doc.guide.items.some((item) => item.id === into.afterId) ? into.afterId : null;
  const label = `Insert ${plural(items.length, "recorded step", "recorded steps")}`;
  return dispatch(insertItemsAfter(doc.guide, items, after, label));
}

/** Re-captures the pending step, its annotations as `choice` says. False if it no longer applies. */
export function resolveRecapture(editor: EditorStore, pending: RecaptureStore, choice: AnnotationChoice): boolean {
  const waiting = pending.getState().pending;
  pending.setState({ pending: null });
  const { doc, dispatch, notify } = editor.getState();
  if (!waiting || !doc || doc.guide.id !== waiting.guideId) return false;
  const step = doc.guide.items.find((item) => item.id === waiting.stepId);
  if (step?.type !== "step") {
    notify("The step was deleted while it was re-captured.");
    return false;
  }
  return dispatch(recaptureStep(doc.guide, step.id, waiting.captured, choice));
}
