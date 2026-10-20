import { Stack, useRouter } from "expo-router";
import { useState } from "react";
import { Pressable, ScrollView, Switch, View } from "react-native";
import { Button, ErrorText, Field, Input, Screen, Text } from "@/components/ui";
import { newGuide } from "@/lib/guides";
import { sessionStore } from "@/lib/session";
import { saveGuide, syncStore } from "@/lib/sync";
import { ulid } from "@/lib/ulid";
import { useTheme } from "@/theme/theme";

/** The device's language as a BCP 47 primary tag, e.g. `en`. */
function deviceLanguage(): string {
  return Intl.DateTimeFormat().resolvedOptions().locale.split("-")[0]?.toLowerCase() || "en";
}

export default function NewGuide() {
  const theme = useTheme();
  const router = useRouter();
  const canSync = sessionStore((s) => s.status === "signedIn" && Boolean(s.workspaceId));
  const workspace = sessionStore((s) => s.workspaces.find((w) => w.id === s.workspaceId)?.name);
  const [title, setTitle] = useState("");
  const [sync, setSync] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function create(then: "capture" | "import") {
    setSaving(true);
    setError(null);
    try {
      const id = ulid();
      if (sync && canSync) syncStore.getState().setEnabled(id, true);
      await saveGuide(newGuide(id, title, deviceLanguage()));
      router.dismiss();
      router.push({ pathname: "/guides/[id]", params: { id } });
      router.push({ pathname: then === "capture" ? "/guides/[id]/capture" : "/guides/[id]/import", params: { id } });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSaving(false);
    }
  }

  return (
    <Screen>
      <Stack.Screen
        options={{
          headerLeft: () => (
            <Pressable accessibilityRole="button" hitSlop={10} onPress={() => router.back()} disabled={saving}>
              <Text color={theme.brand}>Cancel</Text>
            </Pressable>
          ),
        }}
      />
      <ScrollView
        keyboardShouldPersistTaps="handled"
        contentContainerStyle={{ padding: 20, gap: 16, width: "100%", maxWidth: 560, alignSelf: "center" }}
      >
        <Text variant="small" muted>
          Give the guide a name, then take a photo for each step, or record your screen while you do the task.
        </Text>
        <Field label="Title">
          <Input value={title} onChangeText={setTitle} placeholder="e.g. Reset the router" autoFocus returnKeyType="go" onSubmitEditing={() => title.trim() && void create("capture")} />
        </Field>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
          <View style={{ flex: 1, gap: 2 }}>
            <Text variant="label">Sync with web</Text>
            <Text variant="small" muted>
              {canSync ? `Keep it in ${workspace ?? "your workspace"} too. You can change this later.` : "Sign in to sync guides with the web."}
            </Text>
          </View>
          <Switch value={sync && canSync} disabled={!canSync} onValueChange={setSync} trackColor={{ true: theme.brand, false: theme.input }} accessibilityLabel="Sync with web" />
        </View>
        <ErrorText>{error}</ErrorText>
        <Button variant="brand" size="lg" icon="camera.fill" title="Create and take a photo" loading={saving} disabled={!title.trim()} onPress={() => void create("capture")} />
        <Button variant="outline" size="lg" icon="film" title="Create from a screen recording" disabled={saving || !title.trim()} onPress={() => void create("import")} />
      </ScrollView>
    </Screen>
  );
}
