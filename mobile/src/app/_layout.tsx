import "@/lib/polyfills";
import { InstrumentSerif_400Regular } from "@expo-google-fonts/instrument-serif/400Regular";
import { Inter_400Regular } from "@expo-google-fonts/inter/400Regular";
import { Inter_500Medium } from "@expo-google-fonts/inter/500Medium";
import { Inter_600SemiBold } from "@expo-google-fonts/inter/600SemiBold";
import { Inter_700Bold } from "@expo-google-fonts/inter/700Bold";
import { useFonts } from "expo-font";
import { Stack } from "expo-router";
import * as SplashScreen from "expo-splash-screen";
import { StatusBar } from "expo-status-bar";
import * as SystemUI from "expo-system-ui";
import { useEffect } from "react";
import { AppState } from "react-native";
import { deviceStore } from "@/lib/device";
import { sessionStore } from "@/lib/session";
import { syncStore } from "@/lib/sync";
import { webStore } from "@/lib/web";
import { themeStore, useTheme } from "@/theme/theme";
import { fonts } from "@/theme/themes";

void SplashScreen.preventAutoHideAsync();

const SYNC_EVERY_MS = 60_000;

export default function RootLayout() {
  const theme = useTheme();
  const status = sessionStore((s) => s.status);
  const workspaceId = sessionStore((s) => s.workspaceId);
  const [fontsLoaded] = useFonts({ Inter_400Regular, Inter_500Medium, Inter_600SemiBold, Inter_700Bold, InstrumentSerif_400Regular });

  useEffect(() => {
    deviceStore.getState().reload();
    // The sync state first: signing in picks its workspace.
    void Promise.all([themeStore.getState().load(), syncStore.getState().load().then(() => sessionStore.getState().load())]);
  }, []);

  useEffect(() => {
    const signedIn = status === "signedIn" ? workspaceId : null;
    void webStore.getState().open(signedIn);
    if (status === "loading") return;
    syncStore.getState().useWorkspace(signedIn);
    if (signedIn) void syncStore.getState().syncNow();
  }, [status, workspaceId]);

  // Sync now and then while the app is open, and catch up when it comes back.
  useEffect(() => {
    const catchUp = () => {
      if (sessionStore.getState().status !== "signedIn" || AppState.currentState !== "active") return;
      void syncStore.getState().syncNow();
      void webStore.getState().refresh();
    };
    const sub = AppState.addEventListener("change", (state) => state === "active" && catchUp());
    const timer = setInterval(catchUp, SYNC_EVERY_MS);
    return () => {
      sub.remove();
      clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    void SystemUI.setBackgroundColorAsync(theme.background);
  }, [theme.background]);

  const ready = fontsLoaded && status !== "loading";
  useEffect(() => {
    if (ready) void SplashScreen.hideAsync();
  }, [ready]);
  if (!ready) return null;

  return (
    <>
      <StatusBar style={theme.dark ? "light" : "dark"} />
      <Stack
        screenOptions={{
          headerStyle: { backgroundColor: theme.background },
          headerTintColor: theme.brand,
          headerTitleStyle: { fontFamily: fonts.semibold, color: theme.foreground },
          headerShadowVisible: false,
          headerBackButtonDisplayMode: "minimal",
          contentStyle: { backgroundColor: theme.background },
        }}
      >
        <Stack.Protected guard={status === "signedIn"}>
          <Stack.Screen name="index" options={{ title: "Library" }} />
          <Stack.Screen name="guides/[id]/index" options={{ title: "" }} />
          <Stack.Screen name="guides/[id]/capture" options={{ presentation: "fullScreenModal", headerShown: false, animation: "fade" }} />
          <Stack.Screen name="guides/[id]/import" options={{ presentation: "modal", title: "From a recording" }} />
          <Stack.Screen name="new-guide" options={{ presentation: "modal", title: "New guide" }} />
          <Stack.Screen name="settings" options={{ presentation: "modal", title: "Settings" }} />
        </Stack.Protected>
        <Stack.Protected guard={status === "signedOut"}>
          <Stack.Screen name="sign-in" options={{ headerShown: false }} />
        </Stack.Protected>
      </Stack>
    </>
  );
}
