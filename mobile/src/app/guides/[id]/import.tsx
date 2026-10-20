// Adds steps from a screen recording: the user records the task with iOS's
// Screen Recording, picks the video here, and reviews the screens found in
// it (recording.ts) before they're added.

import * as Haptics from "expo-haptics";
import { Image } from "expo-image";
import * as ImagePicker from "expo-image-picker";
import { Stack, useLocalSearchParams, useRouter } from "expo-router";
import { useRef, useState } from "react";
import { KeyboardAvoidingView, Pressable, ScrollView, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { Button, Card, ErrorText, Icon, Input, Screen, StepNumber, Text } from "@/components/ui";
import { appendItem, photoPath, photoStep } from "@/lib/guides";
import { deviceStore } from "@/lib/device";
import { saveGuide } from "@/lib/sync";
import { findSteps, type FoundStep } from "@/lib/recording";
import { ulid } from "@/lib/ulid";
import { useTheme } from "@/theme/theme";
import { radius } from "@/theme/themes";

type Phase = "intro" | "scanning" | "review" | "saving";

interface Candidate extends FoundStep {
  keep: boolean;
  title: string;
}

const HOW = [
  "Open Control Center and tap Screen Recording. (No button? Add it in Settings › Control Center.)",
  "Do the task in any app. Pause for a moment on each screen, so it's caught.",
  "Stop the recording, come back here and choose it. You can trim the start and end.",
];

function clock(seconds: number): string {
  const s = Math.floor(seconds);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export default function ImportRecording() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const router = useRouter();
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const { width } = useWindowDimensions();
  const [phase, setPhase] = useState<Phase>("intro");
  const [progress, setProgress] = useState(0);
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [error, setError] = useState<string | null>(null);
  const cancelled = useRef(false);
  const guideTitle = deviceStore((s) => s.guides[id]?.title);

  const kept = candidates.filter((c) => c.keep);
  const columns = width >= 1000 ? 3 : width >= 700 ? 2 : 1;

  async function choose() {
    setError(null);
    // The original file needs Photos access; asking first avoids a second prompt after picking.
    const permission = await ImagePicker.requestMediaLibraryPermissionsAsync();
    if (!permission.granted) return setError("Allow GuideRecorder to access your Photos to choose a recording.");
    const picked = await ImagePicker.launchImageLibraryAsync({ mediaTypes: ["videos"], allowsEditing: true, quality: 1 });
    if (picked.canceled || !picked.assets[0]) return;
    const video = picked.assets[0];
    cancelled.current = false;
    setProgress(0);
    setPhase("scanning");
    try {
      const found = await findSteps(video.uri, video.duration ? video.duration / 1000 : null, setProgress, () => cancelled.current);
      if (cancelled.current) return setPhase("intro");
      setCandidates(found.map((f) => ({ ...f, keep: true, title: "" })));
      setPhase("review");
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("intro");
    }
  }

  async function add() {
    setPhase("saving");
    setError(null);
    try {
      const steps = kept.map((c) => ({ id: ulid(), candidate: c }));
      const guide = deviceStore.getState().guides[id];
      if (!guide) throw new Error("This guide isn't on this phone any more.");
      await saveGuide(
        steps.reduce((g, s) => appendItem(g, photoStep(s.id, s.candidate.title, "")), guide),
        steps.map((s) => ({ path: photoPath(s.id), uri: s.candidate.uri })),
      );
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      router.back();
    } catch (e) {
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error);
      setError(e instanceof Error ? e.message : String(e));
      setPhase("review");
    }
  }

  const update = (index: number, change: Partial<Candidate>) =>
    setCandidates((all) => all.map((c, i) => (i === index ? { ...c, ...change } : c)));

  const header = (
    <Stack.Screen
      options={{
        title: "From a recording",
        headerLeft: () => (
          <Pressable
            accessibilityRole="button"
            hitSlop={10}
            disabled={phase === "saving"}
            onPress={() => {
              cancelled.current = true;
              router.back();
            }}
          >
            <Text color={theme.brand}>Cancel</Text>
          </Pressable>
        ),
      }}
    />
  );

  if (phase === "intro" || phase === "scanning") {
    return (
      <Screen>
        {header}
        <ScrollView contentContainerStyle={{ padding: 20, gap: 20, paddingBottom: insets.bottom + 20, width: "100%", maxWidth: 560, alignSelf: "center" }}>
          <View style={{ gap: 6 }}>
            <Text variant="title">Record it on your phone</Text>
            <Text variant="small" muted>
              Each screen you stop on becomes a step{guideTitle ? ` in “${guideTitle}”` : ""}. iOS doesn't let apps see where you tap, so give each step a title
              when you review them.
            </Text>
          </View>
          <Card style={{ gap: 14 }}>
            {HOW.map((text, i) => (
              <View key={i} style={{ flexDirection: "row", gap: 12, alignItems: "flex-start" }}>
                <StepNumber number={i + 1} size={24} />
                <Text variant="small" style={{ flex: 1, paddingTop: 2 }}>
                  {text}
                </Text>
              </View>
            ))}
          </Card>
          {phase === "scanning" ? (
            <Card style={{ gap: 12 }}>
              <Text variant="label">Finding the steps… {Math.round(progress * 100)}%</Text>
              <View style={{ height: 6, borderRadius: radius.full, backgroundColor: theme.muted, overflow: "hidden" }}>
                <View style={{ height: "100%", width: `${Math.round(progress * 100)}%`, backgroundColor: theme.brand }} />
              </View>
              <Button
                variant="ghost"
                title="Stop"
                onPress={() => {
                  cancelled.current = true;
                }}
              />
            </Card>
          ) : (
            <Button variant="brand" size="lg" icon="film" title="Choose a recording" onPress={() => void choose()} />
          )}
          <ErrorText>{error}</ErrorText>
        </ScrollView>
      </Screen>
    );
  }

  let number = 0;
  return (
    <Screen>
      {header}
      <KeyboardAvoidingView behavior="padding" style={{ flex: 1 }} keyboardVerticalOffset={100}>
        <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={{ padding: 16, gap: 14, paddingBottom: 24 }}>
          <Text variant="small" muted style={{ maxWidth: 680 }}>
            {candidates.length
              ? `Found ${candidates.length === 1 ? "1 screen" : `${candidates.length} screens`}. Remove the ones you don't need and give the rest a title. Untitled steps can be written by Write with AI in the desktop app.`
              : "No steps found. Try a recording where you pause on each screen."}
          </Text>
          <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 14 }}>
            {candidates.map((c, i) => {
              if (c.keep) number++;
              return (
                <Card
                  key={c.uri}
                  style={{
                    width: columns === 1 ? "100%" : `${100 / columns - 2}%`,
                    flexGrow: columns === 1 ? 0 : 1,
                    padding: 14,
                    gap: 10,
                    opacity: c.keep ? 1 : 0.45,
                  }}
                >
                  <View style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
                    {c.keep ? <StepNumber number={number} size={26} /> : <Icon name="trash" color={theme.mutedForeground} size={16} />}
                    <Text variant="small" muted style={{ flex: 1 }}>
                      {c.keep ? `At ${clock(c.time)}` : `Removed · at ${clock(c.time)}`}
                    </Text>
                    <Button
                      variant="ghost"
                      size="sm"
                      icon={c.keep ? "trash" : "arrow.uturn.backward"}
                      title={c.keep ? "Remove" : "Restore"}
                      onPress={() => update(i, { keep: !c.keep })}
                    />
                  </View>
                  <Image
                    source={{ uri: c.uri }}
                    contentFit="contain"
                    style={{
                      width: "100%",
                      aspectRatio: c.width / Math.max(c.height, 1),
                      maxHeight: 420,
                      borderRadius: radius.lg,
                      backgroundColor: theme.muted,
                      borderWidth: 1,
                      borderColor: theme.border,
                    }}
                  />
                  {c.keep ? <Input value={c.title} onChangeText={(title) => update(i, { title })} placeholder="What to do here, e.g. Tap Settings" /> : null}
                </Card>
              );
            })}
          </View>
        </ScrollView>
        <View style={{ padding: 16, paddingBottom: insets.bottom + 12, gap: 8, borderTopWidth: 1, borderTopColor: theme.border, backgroundColor: theme.background }}>
          <ErrorText>{error}</ErrorText>
          <View style={{ flexDirection: "row", gap: 10, width: "100%", maxWidth: 560, alignSelf: "center" }}>
            <Button variant="outline" size="lg" title="Choose another" disabled={phase === "saving"} onPress={() => void choose()} style={{ flex: 1 }} />
            <Button
              variant="brand"
              size="lg"
              title={kept.length === 1 ? "Add 1 step" : `Add ${kept.length} steps`}
              loading={phase === "saving"}
              disabled={!kept.length}
              onPress={() => void add()}
              style={{ flex: 1.4 }}
            />
          </View>
        </View>
      </KeyboardAvoidingView>
    </Screen>
  );
}
