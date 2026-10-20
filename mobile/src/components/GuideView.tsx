// A synced guide, read-only: the mobile counterpart of the web viewer
// (web/src/components/guides/GuideViewer.tsx): title, intro,
// prerequisites, then numbered steps with their images, sections and notes.

import type { Guide, NoteKind } from "@guiderecorder/guide-schema";
import type { ReviewState } from "@guiderecorder/sync";
import { View } from "react-native";
import { shownImage, stepCount, viewItems } from "@/lib/guides";
import { useTheme } from "@/theme/theme";
import { radius } from "@/theme/themes";
import { GuideImage, type ImageRef } from "./GuideImage";
import { Markdown } from "./Markdown";
import { Badge, Card, StepNumber, Text, type Tone } from "./ui";

const NOTE_TONE: Record<NoteKind, Tone> = { tip: "success", warning: "warning", note: "info" };
const NOTE_LABEL: Record<NoteKind, string> = { tip: "Tip", warning: "Warning", note: "Note" };

export const STATUS: Record<string, { label: string; tone: Tone }> = {
  draft: { label: "Draft", tone: "neutral" },
  inReview: { label: "In review", tone: "info" },
  published: { label: "Published", tone: "success" },
  needsUpdate: { label: "Needs update", tone: "warning" },
};

/** A guide, read-only. `image` says where each of its images is (on the phone or the web). */
export function GuideView({ guide, image, review }: { guide: Guide; image: (path: string) => ImageRef; review?: ReviewState }) {
  const theme = useTheme();
  const status = STATUS[review?.status ?? guide.status ?? ""];
  const steps = stepCount(guide);

  return (
    <View style={{ gap: 24, width: "100%", maxWidth: 820, alignSelf: "center" }}>
      <View style={{ gap: 10 }}>
        <Text variant="display">{guide.title || "Untitled guide"}</Text>
        {guide.summary ? <Text muted>{guide.summary}</Text> : null}
        <View style={{ flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 8 }}>
          {status ? <Badge tone={status.tone}>{status.label}</Badge> : null}
          <Text variant="small" muted>
            {steps === 1 ? "1 step" : `${steps} steps`}
          </Text>
          {guide.tags.map((tag) => (
            <Badge key={tag}>{tag}</Badge>
          ))}
        </View>
      </View>

      {guide.intro.trim() ? <Markdown text={guide.intro} small /> : null}
      {guide.prerequisites.trim() ? (
        <View style={{ gap: 4 }}>
          <Text variant="label">Before you start</Text>
          <Markdown text={guide.prerequisites} small />
        </View>
      ) : null}

      {viewItems(guide).map((item) => {
        if (item.type === "section") {
          return (
            <Text key={item.id} variant="heading" style={{ fontSize: 20, marginTop: 8 }}>
              {item.title}
            </Text>
          );
        }
        if (item.type === "note") {
          const tone = theme[NOTE_TONE[item.note.kind]];
          return (
            <View
              key={item.note.id}
              accessibilityRole="summary"
              style={{
                backgroundColor: theme.card,
                borderRadius: radius.xl,
                borderWidth: 1,
                borderColor: theme.border,
                borderLeftWidth: 4,
                borderLeftColor: tone,
                padding: 16,
                gap: 4,
              }}
            >
              <Text variant="caption" muted>
                {NOTE_LABEL[item.note.kind]}
              </Text>
              <Markdown text={item.note.body} small />
            </View>
          );
        }
        const { step, number } = item;
        const path = shownImage(step);
        return (
          <Card key={step.id} style={{ gap: 16 }}>
            <View style={{ flexDirection: "row", alignItems: "flex-start", gap: 12 }}>
              <StepNumber number={number} />
              <View style={{ flex: 1, paddingTop: 2 }}>
                {step.title.trim() ? <Markdown text={step.title} /> : <Text muted>Step {number}</Text>}
              </View>
            </View>
            {/* Portrait photos letterboxed, so a step stays on screen. */}
            {path ? <GuideImage source={image(path)} style={{ maxHeight: 480 }} /> : null}
            {step.description.trim() ? <Markdown text={step.description} small /> : null}
          </Card>
        );
      })}

      {steps === 0 ? (
        <Card style={{ alignItems: "center", paddingVertical: 32 }}>
          <Text variant="heading">No steps yet</Text>
          <Text variant="small" muted style={{ textAlign: "center" }}>
            Take a photo to add the first step.
          </Text>
        </Card>
      ) : null}
    </View>
  );
}
