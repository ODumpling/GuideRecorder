// Editors for conditional content (#22): a decision's options and where
// each leads, and the audiences a section is for. Every change is one
// undoable command; typing into a label or the audiences coalesces.

import { useState } from "react";
import { Plus, X } from "lucide-react";
import type { BranchTarget, Decision, Guide, Section } from "@guiderecorder/guide-schema";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { NativeSelect, NativeSelectOption } from "@guiderecorder/ui/native-select";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";
import {
  addDecisionOption,
  removeDecisionOption,
  setOptionLabel,
  setOptionTarget,
  setSectionAudiences,
  type Command,
} from "@/editor/commands";
import { editorStore, stepNoun, useAnnotated, useEditor } from "@/editor/editor";
import { ulid } from "@/lib/ulid";

function dispatch(make: (guide: Guide) => Command): boolean {
  const { doc, dispatch } = editorStore.getState();
  return !!doc && !doc.readOnly && dispatch(make(doc.guide));
}

/** A target as a `<select>` value: `next`, `step:<id>`, `section:<id>` or `guide:<id>`. */
export function targetValue(target: BranchTarget): string {
  return target.kind === "next" ? "next" : `${target.kind}:${target.id}`;
}

function plain(markdown: string): string {
  return markdownToPlainText(markdown).replace(/\n/g, " ").trim();
}

/** The decision's options: a label, where it goes, and a remove button each. */
export function DecisionOptions({ decision, readOnly }: { decision: Decision; readOnly: boolean }) {
  const annotated = useAnnotated();
  const items = useEditor((s) => s.doc?.guide.items ?? []);
  const guides = useEditor((s) => s.guides);
  const guideId = useEditor((s) => s.doc?.guide.id);
  const noun = stepNoun(annotated);
  let number = 0;
  const places = items.flatMap((item) => {
    if (item.type === "step") {
      number += 1;
      const title = plain(item.title);
      return [{ value: `step:${item.id}`, label: `${noun} ${number}${title ? `: ${title}` : ""}` }];
    }
    if (item.type === "section") return [{ value: `section:${item.id}`, label: `Section: ${plain(item.title) || "Untitled"}` }];
    return [];
  });
  const others = guides.filter((g) => g.id !== guideId);

  const choose = (optionId: string, value: string) => {
    const [kind, id] = value.split(":");
    let target: BranchTarget;
    if (kind === "step" || kind === "section") target = { kind, id };
    else if (kind === "guide") {
      const current = decision.options.find((o) => o.id === optionId)?.target;
      const title = others.find((g) => g.id === id)?.title ?? (current?.kind === "guide" && current.id === id ? current.title : "");
      target = { kind, id, title };
    } else target = { kind: "next" };
    dispatch((g) => setOptionTarget(g, decision.id, optionId, target));
  };

  return (
    <div className="flex flex-col gap-2" data-testid="decision-options">
      {decision.options.map((option, i) => {
        const value = targetValue(option.target);
        const known = value === "next" || places.some((p) => p.value === value) || others.some((g) => `guide:${g.id}` === value);
        return (
          <div key={option.id} className="flex items-center gap-2">
            <Input
              aria-label={`Option ${i + 1} label`}
              className="h-8 max-w-48"
              value={option.label}
              placeholder="Answer, e.g. Yes"
              disabled={readOnly}
              onChange={(e) => dispatch((g) => setOptionLabel(g, decision.id, option.id, e.target.value))}
            />
            <span className="text-sm text-muted-foreground">goes to</span>
            <NativeSelect
              aria-label={`Option ${i + 1} goes to`}
              className="max-w-72"
              value={value}
              disabled={readOnly}
              onChange={(e) => choose(option.id, e.target.value)}
            >
              <NativeSelectOption value="next">The next item</NativeSelectOption>
              {/* A step or section that is gone reads as the next item in exports. */}
              {!known && (
                <NativeSelectOption value={value}>
                  {option.target.kind === "guide" ? `Guide: ${option.target.title || "Untitled"}` : "A removed item (the next item)"}
                </NativeSelectOption>
              )}
              {places.map((p) => (
                <NativeSelectOption key={p.value} value={p.value}>
                  {p.label}
                </NativeSelectOption>
              ))}
              {others.length > 0 && (
                <optgroup label="Another guide">
                  {others.map((g) => (
                    <NativeSelectOption key={g.id} value={`guide:${g.id}`}>
                      {g.title || "Untitled guide"}
                    </NativeSelectOption>
                  ))}
                </optgroup>
              )}
            </NativeSelect>
            {!readOnly && (
              <Button
                variant="ghost"
                size="icon"
                aria-label={`Remove option ${i + 1}`}
                title="Remove this option"
                onClick={() => dispatch((g) => removeDecisionOption(g, decision.id, option.id))}
              >
                <X />
              </Button>
            )}
          </div>
        );
      })}
      {!readOnly && (
        <div>
          <Button variant="outline" size="sm" onClick={() => dispatch((g) => addDecisionOption(g, decision.id, ulid()))}>
            <Plus /> Add option
          </Button>
        </div>
      )}
    </div>
  );
}

/**
 * Who a section is for, as comma-separated names. Applied when the field
 * loses focus or Enter is pressed, so typing a comma doesn't get trimmed.
 */
export function SectionAudiences({ section, readOnly }: { section: Section; readOnly: boolean }) {
  const stored = (section.audiences ?? []).join(", ");
  const [text, setText] = useState(stored);
  const [editing, setEditing] = useState(false);
  const commit = () => {
    setEditing(false);
    if (text.trim() === stored) return;
    dispatch((g) => setSectionAudiences(g, section.id, text.split(",")));
  };
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2">
        <span className="text-sm text-muted-foreground">Audience</span>
        <Input
          aria-label="Section audiences"
          className="h-8 max-w-80"
          value={editing ? text : stored}
          placeholder="Everyone"
          disabled={readOnly}
          onFocus={() => {
            setText(stored);
            setEditing(true);
          }}
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
          }}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        Who this section is for, e.g. Admin, User. Leave empty for everyone. An export can leave out sections for other audiences.
      </p>
    </div>
  );
}
