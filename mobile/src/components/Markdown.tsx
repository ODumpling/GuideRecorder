// Guide text: the Markdown the editors write (paragraphs, lists, bold,
// italic, code and links), drawn with the theme's text styles.

import { Fragment, type ReactNode } from "react";
import { Linking, View } from "react-native";
import { useTheme } from "@/theme/theme";
import { fonts } from "@/theme/themes";
import { blocks } from "@/lib/markdown";
import { Text } from "./ui";

const INLINE = /(\*\*[^*]+\*\*|__[^_]+__|\*[^*]+\*|_[^_]+_|`[^`]+`|\[[^\]]+\]\([^)\s]+\))/g;

function Inline({ text }: { text: string }) {
  const theme = useTheme();
  const parts: ReactNode[] = text.split(INLINE).map((part, i) => {
    if (/^(\*\*|__).+\1$/.test(part)) return <Text key={i} style={{ fontFamily: fonts.semibold }}>{part.slice(2, -2)}</Text>;
    if (/^`.+`$/.test(part))
      return (
        <Text key={i} variant="mono" style={{ fontSize: 14, backgroundColor: theme.muted }}>
          {part.slice(1, -1)}
        </Text>
      );
    if (/^(\*|_).+\1$/.test(part)) return <Text key={i} style={{ fontStyle: "italic" }}>{part.slice(1, -1)}</Text>;
    const link = /^\[([^\]]+)\]\(([^)\s]+)\)$/.exec(part);
    if (link) {
      const url = link[2]!;
      const safe = /^(https?:|mailto:)/i.test(url);
      return (
        <Text key={i} color={theme.info} style={{ textDecorationLine: "underline" }} onPress={safe ? () => void Linking.openURL(url) : undefined}>
          {link[1]}
        </Text>
      );
    }
    return <Fragment key={i}>{part}</Fragment>;
  });
  return <>{parts}</>;
}

export function Markdown({ text, small }: { text: string; small?: boolean }) {
  const variant = small ? "small" : "body";
  return (
    <View style={{ gap: 8 }}>
      {blocks(text).map((block, i) =>
        block.type === "p" ? (
          <Text key={i} variant={variant}>
            <Inline text={block.text} />
          </Text>
        ) : (
          <View key={i} style={{ gap: 4 }}>
            {block.items.map((item, j) => (
              <View key={j} style={{ flexDirection: "row", gap: 8, paddingLeft: 4 }}>
                <Text variant={variant} muted style={{ minWidth: 16 }}>
                  {block.ordered ? `${j + 1}.` : "•"}
                </Text>
                <Text variant={variant} style={{ flex: 1 }}>
                  <Inline text={item} />
                </Text>
              </View>
            ))}
          </View>
        ),
      )}
    </View>
  );
}
