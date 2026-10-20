import { useRouter } from "expo-router";
import type { ReactNode } from "react";
import { Pressable, ScrollView, View } from "react-native";
import { Button, Card, Icon, Screen, Text } from "@/components/ui";
import { WEB_APP_URL } from "@/lib/config";
import { sessionStore } from "@/lib/session";
import { syncStore } from "@/lib/sync";
import { themeStore, useTheme } from "@/theme/theme";
import { radius, themes, type ThemeChoice } from "@/theme/themes";

const CHOICES: { id: ThemeChoice; name: string }[] = [
  { id: "system", name: "Match the system" },
  ...Object.values(themes).map((t) => ({ id: t.id, name: t.name })),
];

export default function Settings() {
  const theme = useTheme();
  const router = useRouter();
  const user = sessionStore((s) => s.user);
  const workspaces = sessionStore((s) => s.workspaces);
  const workspaceId = sessionStore((s) => s.workspaceId);
  const choice = themeStore((s) => s.choice);
  const syncing = syncStore((s) => s.running);
  const lastSynced = syncStore((s) => s.lastSynced);
  const syncError = syncStore((s) => s.error);
  const syncedCount = syncStore((s) => Object.values(s.state.guides).filter((g) => g.enabled).length);

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ padding: 20, gap: 24, width: "100%", maxWidth: 640, alignSelf: "center" }}>
        <Section title="Account">
          <Card style={{ gap: 4 }}>
            <Text variant="label">{user?.name ?? "Signed in"}</Text>
            {user?.email ? (
              <Text variant="small" muted>
                {user.email}
              </Text>
            ) : null}
            <Text variant="small" muted>
              {new URL(WEB_APP_URL).host}
            </Text>
          </Card>
        </Section>

        <Section title="Workspace">
          <Card style={{ padding: 6, gap: 0 }}>
            {workspaces.map((w) => (
              <Row key={w.id} selected={w.id === workspaceId} onPress={() => sessionStore.getState().chooseWorkspace(w.id)}>
                <Text>{w.name}</Text>
              </Row>
            ))}
          </Card>
        </Section>

        <Section title="Sync">
          <Card style={{ gap: 10 }}>
            <Text variant="small" muted>
              {syncedCount === 1 ? "1 guide on this phone syncs" : `${syncedCount} guides on this phone sync`} with the web. Turn sync on or off on each guide.
            </Text>
            <Text variant="small" color={syncError ? theme.destructive : theme.mutedForeground}>
              {syncError ?? (lastSynced ? `Last synced ${new Date(lastSynced).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}` : "Not synced yet")}
            </Text>
            <Button variant="outline" icon="arrow.triangle.2.circlepath" title="Sync now" loading={syncing} onPress={() => void syncStore.getState().syncNow()} />
          </Card>
        </Section>

        <Section title="Appearance">
          <Card style={{ padding: 6, gap: 0 }}>
            {CHOICES.map((c) => {
              const t = c.id === "system" ? null : themes[c.id];
              return (
                <Row key={c.id} selected={c.id === choice} onPress={() => themeStore.getState().choose(c.id)}>
                  <View style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
                    <View style={{ flexDirection: "row", borderRadius: radius.full, overflow: "hidden", borderWidth: 1, borderColor: theme.border }}>
                      {(t ? [t.background, t.brand] : [themes.arctic.background, themes.paperDark.background]).map((color) => (
                        <View key={color} style={{ width: 12, height: 22, backgroundColor: color }} />
                      ))}
                    </View>
                    <Text>{c.name}</Text>
                  </View>
                </Row>
              );
            })}
          </Card>
        </Section>

        <Button
          variant="destructive"
          size="lg"
          title="Sign out"
          onPress={() => {
            router.dismissAll();
            void sessionStore.getState().signOut();
          }}
        />
      </ScrollView>
    </Screen>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <View style={{ gap: 8 }}>
      <Text variant="caption" muted style={{ paddingHorizontal: 4 }}>
        {title}
      </Text>
      {children}
    </View>
  );
}

function Row({ selected, onPress, children }: { selected: boolean; onPress: () => void; children: ReactNode }) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityRole="radio"
      accessibilityState={{ selected }}
      onPress={onPress}
      style={({ pressed }) => ({
        flexDirection: "row",
        alignItems: "center",
        justifyContent: "space-between",
        minHeight: 46,
        paddingHorizontal: 12,
        borderRadius: radius.lg,
        backgroundColor: pressed ? theme.accent : "transparent",
      })}
    >
      {children}
      {selected ? <Icon name="checkmark" color={theme.brand} size={16} weight="semibold" /> : null}
    </Pressable>
  );
}
