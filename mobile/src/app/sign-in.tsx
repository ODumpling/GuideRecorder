import { ScrollView, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { Button, Card, ErrorText, Icon, Screen, Text } from "@/components/ui";
import { WEB_APP_URL } from "@/lib/config";
import { sessionStore } from "@/lib/session";
import { useTheme } from "@/theme/theme";
import { radius } from "@/theme/themes";

const POINTS = [
  { icon: "camera", text: "Take photos with the front or back camera and add each one as a step." },
  { icon: "rectangle.stack", text: "Read every guide in your workspace, on your phone or iPad." },
  { icon: "arrow.triangle.2.circlepath", text: "Changes sync with the desktop app and the web editor." },
] as const;

export default function SignIn() {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const signIn = sessionStore((s) => s.signIn);
  const error = sessionStore((s) => s.error);
  const { startSignIn, openSignInPage, cancelSignIn } = sessionStore.getState();

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ flexGrow: 1, justifyContent: "center", padding: 24, paddingTop: insets.top + 24, paddingBottom: insets.bottom + 24 }}>
        <View style={{ width: "100%", maxWidth: 440, alignSelf: "center", gap: 28 }}>
          <View style={{ gap: 14 }}>
            <View style={{ width: 52, height: 52, borderRadius: radius.xl, backgroundColor: theme.brand, alignItems: "center", justifyContent: "center" }}>
              <Icon name="list.number" color={theme.brandForeground} size={26} />
            </View>
            <Text variant="caption" muted>
              GuideRecorder
            </Text>
            <Text variant="display">Do a task once, get a step-by-step guide.</Text>
          </View>

          <View style={{ gap: 14 }}>
            {POINTS.map((p) => (
              <View key={p.icon} style={{ flexDirection: "row", gap: 12, alignItems: "flex-start" }}>
                <Icon name={p.icon} color={theme.brand} size={20} />
                <Text variant="small" style={{ flex: 1 }}>
                  {p.text}
                </Text>
              </View>
            ))}
          </View>

          {signIn ? (
            <Card style={{ gap: 16 }}>
              <Text variant="heading">Approve this code on the web</Text>
              <Text variant="small" muted>
                Check that the page shows the same code, then tap Approve. This screen moves on by itself.
              </Text>
              <Text variant="mono" style={{ fontSize: 28, lineHeight: 34, letterSpacing: 4, textAlign: "center" }} selectable>
                {signIn.userCode.toUpperCase()}
              </Text>
              <Button variant="brand" size="lg" title="Open the sign-in page" icon="safari" onPress={() => void openSignInPage()} />
              <Button variant="ghost" title="Cancel" onPress={cancelSignIn} />
            </Card>
          ) : (
            <View style={{ gap: 10 }}>
              <Button variant="brand" size="lg" title="Sign in" onPress={() => void startSignIn()} />
              <Text variant="small" muted style={{ textAlign: "center" }}>
                Sign in with your GuideRecorder account at {new URL(WEB_APP_URL).host}. Syncing needs a Pro or Enterprise workspace.
              </Text>
            </View>
          )}
          <ErrorText>{error}</ErrorText>
        </View>
      </ScrollView>
    </Screen>
  );
}
