// A guide: the guide screen on phones, the right pane on iPad. A guide on
// the phone can be edited and synced; a guide only on the web can be read
// and downloaded.

import type { Guide } from "@guiderecorder/guide-schema";
import type { GuideSync } from "@guiderecorder/sync/engine";
import { useRouter } from "expo-router";
import { useState } from "react";
import { Alert, RefreshControl, ScrollView, Switch, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { deviceStore, imageFile } from "@/lib/device";
import { sessionStore } from "@/lib/session";
import { removeFromPhone, syncStore } from "@/lib/sync";
import { webStore, type Entry } from "@/lib/web";
import { useTheme } from "@/theme/theme";
import { GuideView } from "./GuideView";
import { Button, Card, ErrorText, Text } from "./ui";

const FLOATING = { borderRadius: 999, paddingHorizontal: 20, shadowColor: "#000", shadowOpacity: 0.18, shadowRadius: 12, shadowOffset: { width: 0, height: 4 } };

export function GuideDetail({ guideId, onRemoved }: { guideId: string; onRemoved?: () => void }) {
  const local = deviceStore((s) => s.guides[guideId]);
  const web = webStore((s) => s.guides[guideId]);
  if (local) return <PhoneGuide guide={local} onRemoved={onRemoved} />;
  if (web) return <WebGuide entry={web} />;
  return (
    <View style={{ flex: 1, alignItems: "center", justifyContent: "center", padding: 24 }}>
      <Text muted>This guide isn't on this phone or in the workspace any more.</Text>
    </View>
  );
}

function PhoneGuide({ guide, onRemoved }: { guide: Guide; onRemoved?: () => void }) {
  const theme = useTheme();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  const sync = syncStore((s) => s.state.guides[guide.id]);
  const running = syncStore((s) => s.running);

  function remove() {
    Alert.alert(
      "Remove from this phone?",
      sync?.enabled && sync.revisionId ? "Its copy on the web stays. Changes not synced yet are lost." : "This guide is only on this phone, so it's gone for good.",
      [
        { text: "Cancel", style: "cancel" },
        {
          text: "Remove",
          style: "destructive",
          onPress: () => {
            removeFromPhone(guide.id);
            if (onRemoved) onRemoved();
            else router.back();
          },
        },
      ],
    );
  }

  return (
    <View style={{ flex: 1 }}>
      <ScrollView
        contentContainerStyle={{ padding: 20, gap: 20, paddingBottom: insets.bottom + 100 }}
        refreshControl={<RefreshControl refreshing={running} onRefresh={() => void syncStore.getState().syncNow()} tintColor={theme.mutedForeground} />}
      >
        <SyncCard guideId={guide.id} sync={sync} />
        <GuideView guide={guide} image={(path) => ({ uri: imageFile(guide.id, path).uri })} review={sync?.review} />
        <View style={{ width: "100%", maxWidth: 820, alignSelf: "center" }}>
          <Button variant="destructive" icon="trash" title="Remove from this phone" onPress={remove} />
        </View>
      </ScrollView>
      <View
        style={{ position: "absolute", left: 0, right: 0, bottom: insets.bottom + 16, flexDirection: "row", justifyContent: "center", gap: 10 }}
        pointerEvents="box-none"
      >
        <Button
          variant="brand"
          size="lg"
          icon="camera.fill"
          title="Take photo"
          style={FLOATING}
          onPress={() => router.push({ pathname: "/guides/[id]/capture", params: { id: guide.id } })}
        />
        <Button
          variant="outline"
          size="lg"
          icon="film"
          title="From recording"
          style={[FLOATING, { backgroundColor: theme.card }]}
          onPress={() => router.push({ pathname: "/guides/[id]/import", params: { id: guide.id } })}
        />
      </View>
    </View>
  );
}

/** What a guide's sync status means to the user. */
export function syncLabel(sync: GuideSync | undefined): { text: string; short?: string; tone: "success" | "info" | "warning" | "danger" | "neutral" } {
  if (!sync || (!sync.enabled && sync.status !== "deleted")) return { text: "Only on this phone", tone: "neutral" };
  switch (sync.status) {
    case "synced":
      return { text: "Synced", tone: "success" };
    case "conflict":
      return { text: "Changed here and on the web", short: "Conflict", tone: "warning" };
    case "deleted":
      return { text: "Deleted on the web", tone: "danger" };
    case "error":
      return { text: sync.error ?? "Couldn't sync", short: "Sync error", tone: "danger" };
    default:
      return { text: "Waiting to sync", tone: "info" };
  }
}

function SyncCard({ guideId, sync }: { guideId: string; sync: GuideSync | undefined }) {
  const theme = useTheme();
  const signedIn = sessionStore((s) => s.status === "signedIn" && Boolean(s.workspaceId));
  const workspace = sessionStore((s) => s.workspaces.find((w) => w.id === s.workspaceId)?.name);
  const running = syncStore((s) => s.running);
  const error = syncStore((s) => s.error);
  const [resolving, setResolving] = useState(false);
  const label = syncLabel(sync);
  const enabled = Boolean(sync?.enabled);

  async function resolve(resolution: "mine" | "theirs" | "both") {
    setResolving(true);
    await syncStore.getState().resolve(guideId, resolution);
    setResolving(false);
  }

  return (
    <Card style={{ width: "100%", maxWidth: 820, alignSelf: "center", padding: 16, gap: 10 }}>
      <View style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
        <View style={{ flex: 1, gap: 2 }}>
          <Text variant="label">Sync with web</Text>
          <Text variant="small" color={label.tone === "neutral" ? theme.mutedForeground : theme[label.tone]}>
            {enabled && running ? "Syncing…" : enabled && workspace ? `${label.text} · ${workspace}` : label.text}
          </Text>
        </View>
        <Switch
          value={enabled}
          disabled={!signedIn}
          onValueChange={(on) => syncStore.getState().setEnabled(guideId, on)}
          trackColor={{ true: theme.brand, false: theme.input }}
          accessibilityLabel="Sync with web"
        />
      </View>
      {sync?.status === "conflict" ? (
        <View style={{ gap: 8 }}>
          <Text variant="small" muted>
            The same parts were changed here and on the web. Which should stay?
          </Text>
          <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 8 }}>
            <Button size="sm" variant="brand" title="Keep this phone's" disabled={resolving} onPress={() => void resolve("mine")} />
            <Button size="sm" variant="outline" title="Keep the web's" disabled={resolving} onPress={() => void resolve("theirs")} />
            <Button size="sm" variant="outline" title="Keep both" disabled={resolving} onPress={() => void resolve("both")} />
          </View>
        </View>
      ) : null}
      {enabled ? <ErrorText>{error}</ErrorText> : null}
    </Card>
  );
}

function WebGuide({ entry }: { entry: Entry }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const workspaceId = webStore((s) => s.workspaceId);
  const refreshing = webStore((s) => s.refreshing);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function download() {
    setBusy(true);
    setError(null);
    try {
      await syncStore.getState().download(entry.revision, entry.review);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  }

  return (
    <View style={{ flex: 1 }}>
      <ScrollView
        contentContainerStyle={{ padding: 20, gap: 20, paddingBottom: insets.bottom + 100 }}
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void webStore.getState().refresh()} tintColor={theme.mutedForeground} />}
      >
        <Card style={{ width: "100%", maxWidth: 820, alignSelf: "center", padding: 16, gap: 4 }}>
          <Text variant="label">On the web</Text>
          <Text variant="small" muted>
            Download it to add steps on this phone. It stays in sync with the web.
          </Text>
          <ErrorText>{error}</ErrorText>
        </Card>
        <GuideView
          guide={entry.revision.guide}
          image={(path) => (workspaceId ? { workspaceId, sha256: entry.revision.assets[path] } : null)}
          review={entry.review}
        />
      </ScrollView>
      <View style={{ position: "absolute", left: 0, right: 0, bottom: insets.bottom + 16, alignItems: "center" }} pointerEvents="box-none">
        <Button variant="brand" size="lg" icon="arrow.down.circle" title="Download to phone" loading={busy} style={FLOATING} onPress={() => void download()} />
      </View>
    </View>
  );
}
