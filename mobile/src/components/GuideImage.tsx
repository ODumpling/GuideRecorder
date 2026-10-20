import { Image } from "expo-image";
import { useEffect, useState } from "react";
import { StyleSheet, View, type StyleProp, type ViewStyle } from "react-native";
import { imageUrl } from "@/lib/assets";
import { useTheme } from "@/theme/theme";
import { radius } from "@/theme/themes";
import { Icon } from "./ui";

/** Where a guide image is: a file on the phone, or a workspace image by its hash. */
export type ImageRef = { uri: string } | { workspaceId: string; sha256: string | undefined } | null;

/** An image of a guide. Sized to the image once it loads, unless `aspectRatio` is fixed. */
export function GuideImage({ source, aspectRatio, style }: { source: ImageRef; aspectRatio?: number; style?: StyleProp<ViewStyle> }) {
  const theme = useTheme();
  const [uri, setUri] = useState<string | null>(null);
  const [ratio, setRatio] = useState<number | null>(null);
  const [failed, setFailed] = useState(false);

  const local = source && "uri" in source ? source.uri : null;
  const workspaceId = source && "workspaceId" in source ? source.workspaceId : null;
  const sha256 = source && "sha256" in source ? source.sha256 : undefined;

  useEffect(() => {
    let live = true;
    setUri(local);
    setFailed(false);
    if (local || !workspaceId || !sha256) return;
    imageUrl(workspaceId, sha256).then(
      (url) => live && setUri(url),
      () => live && setFailed(true),
    );
    return () => {
      live = false;
    };
  }, [local, workspaceId, sha256]);

  return (
    <View
      style={[
        {
          width: "100%",
          aspectRatio: aspectRatio ?? ratio ?? 16 / 10,
          borderRadius: radius.lg,
          overflow: "hidden",
          backgroundColor: theme.muted,
          borderWidth: 1,
          borderColor: theme.border,
          alignItems: "center",
          justifyContent: "center",
        },
        style,
      ]}
    >
      {uri ? (
        <Image
          source={sha256 ? { uri, cacheKey: sha256 } : { uri }}
          style={StyleSheet.absoluteFill}
          contentFit={aspectRatio ? "cover" : "contain"}
          transition={150}
          cachePolicy="memory-disk"
          onLoad={(e) => setRatio(e.source.width / Math.max(e.source.height, 1))}
          onError={() => setFailed(true)}
        />
      ) : failed || (!local && !sha256) ? (
        <Icon name="photo" color={theme.mutedForeground} size={22} />
      ) : null}
    </View>
  );
}
