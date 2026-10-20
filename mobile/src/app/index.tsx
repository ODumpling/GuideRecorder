// The library: the guides on this phone, and the workspace's guides on the
// web, newest first. On iPad (and wide windows) the selected guide shows
// beside the list.

import type { Guide } from "@guiderecorder/guide-schema";
import { Stack, useRouter } from "expo-router";
import { useMemo, useState } from "react";
import { FlatList, Pressable, RefreshControl, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { GuideDetail, syncLabel } from "@/components/GuideDetail";
import { GuideImage, type ImageRef } from "@/components/GuideImage";
import { STATUS } from "@/components/GuideView";
import { Badge, Button, Card, ErrorText, Icon, Input, Screen, Text } from "@/components/ui";
import { deviceStore, imageFile } from "@/lib/device";
import { coverImage, plainText, stepCount } from "@/lib/guides";
import { sessionStore } from "@/lib/session";
import { syncStore } from "@/lib/sync";
import { sortedGuides, webStore } from "@/lib/web";
import { useTheme } from "@/theme/theme";
import { radius } from "@/theme/themes";

const WIDE = 768;

type Tab = "phone" | "web";

interface Row {
  id: string;
  guide: Guide;
  cover: ImageRef;
  /** The guide's review status, from the web. */
  status: string | null | undefined;
  /** Sync state on the phone tab; "On phone" on the web tab. */
  badge: { text: string; short?: string; tone: "success" | "info" | "warning" | "danger" | "neutral" } | null;
}

function matches(guide: Guide, words: string[]): boolean {
  const text = `${guide.title} ${guide.tags.join(" ")} ${guide.summary}`.toLowerCase();
  return words.every((w) => text.includes(w));
}

export default function Library() {
  const theme = useTheme();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  const { width } = useWindowDimensions();
  const wide = width >= WIDE;
  const [tab, setTab] = useState<Tab>("phone");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);

  const local = deviceStore((s) => s.guides);
  const syncs = syncStore((s) => s.state.guides);
  const syncing = syncStore((s) => s.running);
  const syncError = syncStore((s) => s.error);
  const web = webStore((s) => s.guides);
  const workspaceId = webStore((s) => s.workspaceId);
  const refreshing = webStore((s) => s.refreshing);
  const webError = webStore((s) => s.error);
  const workspace = sessionStore((s) => s.workspaces.find((w) => w.id === s.workspaceId));

  const rows = useMemo((): Row[] => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (tab === "phone") {
      return Object.values(local)
        .filter((g) => matches(g, words))
        .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
        .map((guide) => {
          const cover = coverImage(guide);
          const sync = syncs[guide.id];
          return { id: guide.id, guide, cover: cover ? { uri: imageFile(guide.id, cover).uri } : null, status: sync?.review?.status ?? guide.status, badge: syncLabel(sync) };
        });
    }
    return sortedGuides(web)
      .filter(({ revision }) => matches(revision.guide, words))
      .map(({ revision, review }) => {
        const cover = coverImage(revision.guide);
        return {
          id: revision.guideId,
          guide: revision.guide,
          cover: cover && workspaceId ? { workspaceId, sha256: revision.assets[cover] } : null,
          status: review?.status ?? revision.guide.status,
          badge: local[revision.guideId] ? { text: "On phone", tone: "success" as const } : null,
        };
      });
  }, [tab, local, syncs, web, workspaceId, query]);

  const current = wide ? (selected && (local[selected] || web[selected]) ? selected : rows[0]?.id ?? null) : null;
  const open = (id: string) => (wide ? setSelected(id) : router.push({ pathname: "/guides/[id]", params: { id } }));
  const refresh = () => void (tab === "phone" ? syncStore.getState().syncNow() : webStore.getState().refresh());
  const error = tab === "phone" ? syncError : webError;

  const header = (
    <Stack.Screen
      options={{
        title: workspace?.name ?? "Library",
        headerLeft: () => (
          <Pressable accessibilityLabel="Settings" hitSlop={10} onPress={() => router.push("/settings")}>
            <Icon name="gearshape" color={theme.brand} size={20} />
          </Pressable>
        ),
        headerRight: () => (
          <Pressable accessibilityLabel="New guide" hitSlop={10} onPress={() => router.push("/new-guide")}>
            <Icon name="plus" color={theme.brand} size={20} weight="semibold" />
          </Pressable>
        ),
      }}
    />
  );

  const listView = (
    <FlatList
      data={rows}
      keyExtractor={(r) => r.id}
      contentContainerStyle={{ padding: 16, gap: 10, paddingBottom: insets.bottom + 24 }}
      refreshControl={<RefreshControl refreshing={tab === "phone" ? syncing : refreshing} onRefresh={refresh} tintColor={theme.mutedForeground} />}
      keyboardDismissMode="on-drag"
      ListHeaderComponent={
        <View style={{ gap: 10, marginBottom: 6 }}>
          <Tabs tab={tab} onChange={setTab} />
          <Input value={query} onChangeText={setQuery} placeholder="Search guides" clearButtonMode="while-editing" returnKeyType="search" autoCorrect={false} />
          <ErrorText>{error}</ErrorText>
        </View>
      }
      ListEmptyComponent={
        query ? (
          <Card style={{ alignItems: "center", paddingVertical: 32 }}>
            <Text variant="heading">No guides match</Text>
          </Card>
        ) : tab === "phone" ? (
          <Card style={{ alignItems: "center", paddingVertical: 32 }}>
            <Text variant="heading">No guides on this phone</Text>
            <Text variant="small" muted style={{ textAlign: "center" }}>
              Start one with your camera or a screen recording, or download one from the web.
            </Text>
            <View style={{ flexDirection: "row", gap: 10 }}>
              <Button variant="brand" icon="plus" title="New guide" onPress={() => router.push("/new-guide")} />
              <Button variant="outline" icon="globe" title="Browse the web" onPress={() => setTab("web")} />
            </View>
          </Card>
        ) : refreshing ? undefined : (
          <Card style={{ alignItems: "center", paddingVertical: 32 }}>
            <Text variant="heading">No guides on the web yet</Text>
            <Text variant="small" muted style={{ textAlign: "center" }}>
              Turn on Sync with web for a guide on this phone, or sync one from the desktop app.
            </Text>
          </Card>
        )
      }
      renderItem={({ item }) => <GuideRow row={item} active={item.id === current} onPress={() => open(item.id)} />}
    />
  );

  if (!wide) {
    return (
      <Screen>
        {header}
        {listView}
      </Screen>
    );
  }
  return (
    <Screen style={{ flexDirection: "row" }}>
      {header}
      <View style={{ width: Math.min(400, width * 0.38), borderRightWidth: 1, borderRightColor: theme.border, backgroundColor: theme.background }}>{listView}</View>
      <View style={{ flex: 1 }}>
        {current ? (
          <GuideDetail key={current} guideId={current} onRemoved={() => setSelected(null)} />
        ) : (
          <View style={{ flex: 1, alignItems: "center", justifyContent: "center" }}>
            <Text muted>Choose a guide.</Text>
          </View>
        )}
      </View>
    </Screen>
  );
}

/** The phone / web switch, after the web viewer's view toggle. */
function Tabs({ tab, onChange }: { tab: Tab; onChange: (tab: Tab) => void }) {
  const theme = useTheme();
  const options: { id: Tab; label: string; icon: "iphone" | "globe" }[] = [
    { id: "phone", label: "On this phone", icon: "iphone" },
    { id: "web", label: "Web", icon: "globe" },
  ];
  return (
    <View accessibilityRole="tablist" style={{ flexDirection: "row", padding: 3, borderRadius: radius.lg, backgroundColor: theme.dark ? theme.muted : theme.secondary }}>
      {options.map((o) => {
        const active = tab === o.id;
        return (
          <Pressable
            key={o.id}
            accessibilityRole="tab"
            accessibilityState={{ selected: active }}
            onPress={() => onChange(o.id)}
            style={{
              flex: 1,
              height: 34,
              flexDirection: "row",
              gap: 6,
              alignItems: "center",
              justifyContent: "center",
              borderRadius: radius.md,
              backgroundColor: active ? theme.card : "transparent",
              shadowColor: "#000",
              shadowOpacity: active ? 0.06 : 0,
              shadowRadius: 2,
              shadowOffset: { width: 0, height: 1 },
            }}
          >
            <Icon name={o.icon} size={14} color={active ? theme.foreground : theme.mutedForeground} />
            <Text variant="label" color={active ? theme.foreground : theme.mutedForeground}>
              {o.label}
            </Text>
          </Pressable>
        );
      })}
    </View>
  );
}

function GuideRow({ row, active, onPress }: { row: Row; active: boolean; onPress: () => void }) {
  const theme = useTheme();
  const steps = stepCount(row.guide);
  const status = STATUS[row.status ?? ""];
  const updated = new Date(row.guide.updatedAt).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      style={({ pressed }) => ({
        flexDirection: "row",
        gap: 12,
        alignItems: "center",
        padding: 12,
        borderRadius: radius.xl,
        borderWidth: 1,
        borderColor: active ? theme.ring : theme.border,
        backgroundColor: active ? theme.accent : theme.card,
        opacity: pressed ? 0.85 : 1,
      })}
    >
      <View style={{ width: 76 }}>
        <GuideImage source={row.cover} aspectRatio={4 / 3} style={{ borderRadius: radius.md }} />
      </View>
      <View style={{ flex: 1, gap: 4 }}>
        <Text variant="label" numberOfLines={2} style={{ fontSize: 15 }}>
          {plainText(row.guide.title) || "Untitled guide"}
        </Text>
        <Text variant="small" muted numberOfLines={1} style={{ fontSize: 13 }}>
          {steps === 1 ? "1 step" : `${steps} steps`} · {updated}
        </Text>
        <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 6 }}>
          {row.badge ? <Badge tone={row.badge.tone}>{row.badge.short ?? row.badge.text}</Badge> : null}
          {status ? <Badge tone={status.tone}>{status.label}</Badge> : null}
        </View>
      </View>
      <Icon name="chevron.right" color={theme.mutedForeground} size={13} />
    </Pressable>
  );
}
