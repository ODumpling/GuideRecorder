// Adds steps to a guide from the camera: take a photo with the back or
// front camera, give it a title and description, add it, and take the next.

import { CameraView, useCameraPermissions, type CameraType, type FlashMode } from "expo-camera";
import * as Haptics from "expo-haptics";
import { Image } from "expo-image";
import { useLocalSearchParams, useRouter } from "expo-router";
import { useRef, useState } from "react";
import { KeyboardAvoidingView, Linking, Pressable, ScrollView, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { Button, ErrorText, Field, Icon, Input, Text, type IconName } from "@/components/ui";
import { appendItem, photoPath, photoStep, stepCount } from "@/lib/guides";
import { deviceStore } from "@/lib/device";
import { saveGuide } from "@/lib/sync";
import { saveStepImage, type Photo } from "@/lib/photos";
import { ulid } from "@/lib/ulid";
import { useTheme } from "@/theme/theme";
import { radius } from "@/theme/themes";

const FLASH_ORDER: FlashMode[] = ["off", "auto", "on"];
const FLASH_ICON = { off: "bolt.slash.fill", auto: "bolt.badge.automatic.fill", on: "bolt.fill", screen: "bolt.fill" } as const;

export default function Capture() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  const [permission, requestPermission] = useCameraPermissions();
  const camera = useRef<CameraView>(null);
  const [facing, setFacing] = useState<CameraType>("back");
  const [flash, setFlash] = useState<FlashMode>("off");
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const [photo, setPhoto] = useState<Photo | null>(null);
  const [added, setAdded] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const guideTitle = deviceStore((s) => s.guides[id]?.title);

  const close = () => router.back();

  if (!permission) return <View style={styles.black} />;
  if (!permission.granted) {
    return (
      <View style={[styles.black, styles.center, { padding: 32, gap: 16 }]}>
        <Icon name="camera.fill" color="#fff" size={36} />
        <Text variant="heading" color="#fff" style={{ textAlign: "center" }}>
          Allow camera access to take photos for your steps
        </Text>
        <Button
          variant="brand"
          size="lg"
          title={permission.canAskAgain ? "Allow camera" : "Open Settings"}
          onPress={() => (permission.canAskAgain ? void requestPermission() : void Linking.openSettings())}
        />
        <Button variant="ghost" title="Not now" onPress={close} style={{ backgroundColor: "transparent" }} />
      </View>
    );
  }

  async function take() {
    if (!camera.current || !ready || busy) return;
    setBusy(true);
    setError(null);
    try {
      void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
      const shot = await camera.current.takePictureAsync({ quality: 1 });
      setPhoto(await saveStepImage(shot.uri, shot.width, shot.height));
      setAdded(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (photo) {
    return (
      <Review
        photo={photo}
        guideId={id}
        onRetake={() => setPhoto(null)}
        onAdded={(number) => {
          setPhoto(null);
          setAdded(number);
        }}
        onClose={close}
      />
    );
  }

  return (
    <View style={styles.black}>
      <CameraView
        ref={camera}
        style={StyleSheet.absoluteFill}
        facing={facing}
        flash={flash}
        mode="picture"
        onCameraReady={() => setReady(true)}
        onMountError={(e) => setError(e.message)}
      />
      <View style={[styles.bar, { paddingTop: insets.top + 8 }]}>
        <RoundButton icon="xmark" label="Close" onPress={close} />
        <View style={{ flex: 1, alignItems: "center", paddingHorizontal: 8 }}>
          <View style={styles.pill}>
            <Text variant="small" color="#fff" numberOfLines={1}>
              {added ? `Step ${added} added` : guideTitle || "Add a step"}
            </Text>
          </View>
        </View>
        <RoundButton
          icon={FLASH_ICON[flash]}
          label={`Flash ${flash}`}
          onPress={() => setFlash(FLASH_ORDER[(FLASH_ORDER.indexOf(flash) + 1) % FLASH_ORDER.length]!)}
        />
      </View>

      <View style={[styles.controls, { paddingBottom: insets.bottom + 24 }]}>
        {error ? (
          <View style={[styles.pill, { marginBottom: 16, backgroundColor: "rgba(217,59,59,0.85)" }]}>
            <Text variant="small" color="#fff">
              {error}
            </Text>
          </View>
        ) : null}
        <View style={styles.row}>
          <View style={styles.side}>{added ? <RoundButton icon="checkmark" label="Done" onPress={close} /> : null}</View>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Take photo"
            onPress={() => void take()}
            disabled={!ready || busy}
            style={({ pressed }) => [styles.shutter, { opacity: !ready || busy ? 0.5 : 1, transform: [{ scale: pressed ? 0.92 : 1 }] }]}
          >
            <View style={styles.shutterInner} />
          </Pressable>
          <View style={styles.side}>
            <RoundButton
              icon="arrow.triangle.2.circlepath.camera"
              label={facing === "back" ? "Use the front camera" : "Use the back camera"}
              onPress={() => {
                void Haptics.selectionAsync();
                setFacing(facing === "back" ? "front" : "back");
              }}
            />
          </View>
        </View>
        <Text variant="caption" color="rgba(255,255,255,0.8)" style={{ marginTop: 14 }}>
          {facing === "back" ? "Back camera" : "Front camera"}
        </Text>
      </View>
    </View>
  );
}

function Review({
  photo,
  guideId,
  onRetake,
  onAdded,
  onClose,
}: {
  photo: Photo;
  guideId: string;
  onRetake: () => void;
  onAdded: (number: number) => void;
  onClose: () => void;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function add(thenClose: boolean) {
    setSaving(true);
    setError(null);
    try {
      const stepId = ulid();
      const guide = deviceStore.getState().guides[guideId];
      if (!guide) throw new Error("This guide isn't on this phone any more.");
      const edited = appendItem(guide, photoStep(stepId, title, description));
      await saveGuide(edited, [{ path: photoPath(stepId), uri: photo.uri }]);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      if (thenClose) onClose();
      else onAdded(stepCount(edited));
    } catch (e) {
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error);
      setError(e instanceof Error ? e.message : String(e));
      setSaving(false);
    }
  }

  return (
    <KeyboardAvoidingView behavior="padding" style={{ flex: 1, backgroundColor: theme.background }}>
      <ScrollView
        keyboardShouldPersistTaps="handled"
        contentContainerStyle={{ padding: 20, paddingTop: insets.top + 12, paddingBottom: insets.bottom + 20, gap: 16, width: "100%", maxWidth: 720, alignSelf: "center" }}
      >
        <View style={{ flexDirection: "row", alignItems: "center", justifyContent: "space-between" }}>
          <Button variant="ghost" icon="arrow.uturn.backward" title="Retake" onPress={onRetake} disabled={saving} />
          <Text variant="heading">New step</Text>
          <Button variant="ghost" title="Cancel" onPress={onClose} disabled={saving} />
        </View>
        <Image
          source={{ uri: photo.uri }}
          style={{
            width: "100%",
            aspectRatio: photo.width / Math.max(photo.height, 1),
            maxHeight: 460,
            borderRadius: radius.lg,
            borderWidth: 1,
            borderColor: theme.border,
            backgroundColor: theme.muted,
          }}
          contentFit="contain"
        />
        <Field label="Title">
          <Input value={title} onChangeText={setTitle} placeholder="e.g. Turn the valve to OFF" returnKeyType="next" autoFocus />
        </Field>
        <Field label="Description">
          <Input value={description} onChangeText={setDescription} placeholder="Optional. Markdown works: **bold**, lists." multiline />
        </Field>
        <ErrorText>{error}</ErrorText>
        <View style={{ gap: 10 }}>
          <Button variant="brand" size="lg" icon="camera.fill" title="Add and take another" loading={saving} onPress={() => void add(false)} />
          <Button variant="outline" size="lg" title="Add and finish" disabled={saving} onPress={() => void add(true)} />
        </View>
      </ScrollView>
    </KeyboardAvoidingView>
  );
}

function RoundButton({ icon, label, onPress }: { icon: IconName; label: string; onPress: () => void }) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      hitSlop={8}
      style={({ pressed }) => [styles.round, { opacity: pressed ? 0.7 : 1 }]}
    >
      <Icon name={icon} color="#fff" size={20} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  black: { flex: 1, backgroundColor: "#000" },
  center: { alignItems: "center", justifyContent: "center" },
  bar: { position: "absolute", top: 0, left: 0, right: 0, flexDirection: "row", alignItems: "center", paddingHorizontal: 16 },
  pill: { paddingHorizontal: 12, paddingVertical: 6, borderRadius: 999, backgroundColor: "rgba(0,0,0,0.45)", maxWidth: "100%" },
  controls: { position: "absolute", left: 0, right: 0, bottom: 0, alignItems: "center", paddingTop: 24, backgroundColor: "rgba(0,0,0,0.25)" },
  row: { flexDirection: "row", alignItems: "center", justifyContent: "center", width: "100%", maxWidth: 420 },
  side: { flex: 1, alignItems: "center" },
  shutter: { width: 78, height: 78, borderRadius: 39, borderWidth: 4, borderColor: "#fff", alignItems: "center", justifyContent: "center" },
  shutterInner: { width: 62, height: 62, borderRadius: 31, backgroundColor: "#fff" },
  round: { width: 44, height: 44, borderRadius: 22, backgroundColor: "rgba(0,0,0,0.45)", alignItems: "center", justifyContent: "center" },
});
