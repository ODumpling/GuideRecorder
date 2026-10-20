// The app's building blocks, after packages/ui (button, card, input, badge):
// same variants, radii and colours, sized for touch.

import { SymbolView, type SymbolViewProps } from "expo-symbols";
import type { ReactNode } from "react";
import {
  ActivityIndicator,
  Pressable,
  Text as RNText,
  TextInput,
  View,
  type PressableProps,
  type StyleProp,
  type TextInputProps,
  type TextProps,
  type TextStyle,
  type ViewProps,
  type ViewStyle,
} from "react-native";
import { useTheme } from "@/theme/theme";
import { fonts, radius, type Theme } from "@/theme/themes";

type TextVariant = "display" | "title" | "heading" | "body" | "small" | "label" | "caption" | "mono";

const textStyles: Record<TextVariant, TextStyle> = {
  display: { fontFamily: fonts.serif, fontSize: 40, lineHeight: 44, letterSpacing: -0.4 },
  title: { fontFamily: fonts.serif, fontSize: 32, lineHeight: 36, letterSpacing: -0.3 },
  heading: { fontFamily: fonts.semibold, fontSize: 18, lineHeight: 24 },
  body: { fontFamily: fonts.sans, fontSize: 16, lineHeight: 23 },
  small: { fontFamily: fonts.sans, fontSize: 14, lineHeight: 20 },
  label: { fontFamily: fonts.medium, fontSize: 14, lineHeight: 20 },
  caption: { fontFamily: fonts.semibold, fontSize: 11, lineHeight: 14, letterSpacing: 0.6, textTransform: "uppercase" },
  mono: { fontFamily: fonts.mono, fontSize: 15, lineHeight: 20 },
};

export function Text({
  variant = "body",
  muted,
  color,
  style,
  ...props
}: TextProps & { variant?: TextVariant; muted?: boolean; color?: string }) {
  const theme = useTheme();
  return (
    <RNText
      {...props}
      style={[textStyles[variant], { color: color ?? (muted ? theme.mutedForeground : theme.foreground) }, style]}
    />
  );
}

/** The app's icons: SF Symbols on iOS, their Material Symbols counterparts on Android and the web. */
const MATERIAL = {
  "arrow.triangle.2.circlepath": "sync",
  "arrow.triangle.2.circlepath.camera": "flip_camera_ios",
  "arrow.down.circle": "download",
  "arrow.uturn.backward": "undo",
  "bolt.badge.automatic.fill": "flash_auto",
  "bolt.fill": "flash_on",
  "bolt.slash.fill": "flash_off",
  camera: "photo_camera",
  "camera.fill": "photo_camera",
  checkmark: "check",
  "chevron.right": "chevron_right",
  film: "movie",
  gearshape: "settings",
  globe: "public",
  iphone: "smartphone",
  "list.number": "format_list_numbered",
  photo: "image",
  plus: "add",
  "rectangle.stack": "stacks",
  trash: "delete",
  safari: "open_in_browser",
  xmark: "close",
} as const;

export type IconName = keyof typeof MATERIAL;

export function Icon({ name, size = 18, color, weight }: { name: IconName; size?: number; color?: string; weight?: "regular" | "semibold" }) {
  const theme = useTheme();
  return (
    <SymbolView
      name={{ ios: name, android: MATERIAL[name], web: MATERIAL[name] } as SymbolViewProps["name"]}
      size={size}
      tintColor={color ?? theme.foreground}
      weight={weight}
      resizeMode="scaleAspectFit"
    />
  );
}

type ButtonVariant = "default" | "brand" | "secondary" | "outline" | "ghost" | "destructive";
type ButtonSize = "default" | "sm" | "lg" | "icon";

function buttonColors(theme: Theme, variant: ButtonVariant) {
  switch (variant) {
    case "brand":
      return { bg: theme.brand, fg: theme.brandForeground, border: theme.brand };
    case "secondary":
      return { bg: theme.secondary, fg: theme.secondaryForeground, border: theme.secondary };
    case "outline":
      return { bg: theme.background, fg: theme.foreground, border: theme.border };
    case "ghost":
      return { bg: "transparent", fg: theme.foreground, border: "transparent" };
    case "destructive":
      return { bg: theme.background, fg: theme.destructive, border: theme.border };
    default:
      return { bg: theme.primary, fg: theme.primaryForeground, border: theme.primary };
  }
}

const sizes: Record<ButtonSize, ViewStyle> = {
  default: { height: 40, paddingHorizontal: 14 },
  sm: { height: 32, paddingHorizontal: 10 },
  lg: { height: 48, paddingHorizontal: 18 },
  icon: { height: 40, width: 40 },
};

export function Button({
  title,
  icon,
  variant = "default",
  size = "default",
  loading,
  disabled,
  style,
  ...props
}: Omit<PressableProps, "style" | "children"> & {
  title?: string;
  icon?: IconName;
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  style?: StyleProp<ViewStyle>;
}) {
  const theme = useTheme();
  const c = buttonColors(theme, variant);
  const off = disabled || loading;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={title}
      disabled={off}
      {...props}
      style={({ pressed }) => [
        {
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "center",
          gap: 8,
          borderRadius: radius.md,
          borderWidth: 1,
          borderColor: c.border,
          backgroundColor: c.bg,
          opacity: off ? 0.5 : pressed ? 0.85 : 1,
        },
        variant === "brand" && { shadowColor: "#000", shadowOpacity: 0.08, shadowRadius: 2, shadowOffset: { width: 0, height: 1 } },
        sizes[size],
        style,
      ]}
    >
      {loading ? <ActivityIndicator color={c.fg} /> : icon ? <Icon name={icon} color={c.fg} size={size === "sm" ? 14 : 17} /> : null}
      {title && size !== "icon" ? (
        <Text variant="label" color={c.fg} style={size === "sm" ? { fontSize: 13 } : size === "lg" ? { fontSize: 16 } : null}>
          {title}
        </Text>
      ) : null}
    </Pressable>
  );
}

export function Card({ style, ...props }: ViewProps) {
  const theme = useTheme();
  return (
    <View
      {...props}
      style={[
        {
          backgroundColor: theme.card,
          borderRadius: radius.xl,
          borderWidth: 1,
          borderColor: theme.border,
          padding: 20,
          gap: 12,
          shadowColor: "#0f1b2d",
          shadowOpacity: theme.dark ? 0 : 0.04,
          shadowRadius: 6,
          shadowOffset: { width: 0, height: 2 },
        },
        style,
      ]}
    />
  );
}

export function Input({ style, ...props }: TextInputProps) {
  const theme = useTheme();
  return (
    <TextInput
      placeholderTextColor={theme.mutedForeground}
      selectionColor={theme.ring}
      keyboardAppearance={theme.dark ? "dark" : "light"}
      {...props}
      style={[
        {
          minHeight: 44,
          borderRadius: radius.md,
          borderWidth: 1,
          borderColor: theme.input,
          backgroundColor: theme.dark ? theme.muted : theme.card,
          color: theme.foreground,
          paddingHorizontal: 12,
          paddingVertical: 10,
          fontFamily: fonts.sans,
          fontSize: 16,
        },
        props.multiline && { minHeight: 96, textAlignVertical: "top" },
        style,
      ]}
    />
  );
}

export function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <View style={{ gap: 6 }}>
      <Text variant="label">{label}</Text>
      {children}
    </View>
  );
}

export type Tone = "success" | "warning" | "info" | "danger" | "neutral";

export function Badge({ tone = "neutral", children }: { tone?: Tone; children: string }) {
  const theme = useTheme();
  const color = theme[tone];
  return (
    <View
      style={{
        alignSelf: "flex-start",
        borderRadius: radius.full,
        borderWidth: 1,
        borderColor: color + "40",
        backgroundColor: color + "18",
        paddingHorizontal: 8,
        paddingVertical: 2,
      }}
    >
      <Text variant="small" color={color} style={{ fontSize: 12, fontFamily: fonts.medium }}>
        {children}
      </Text>
    </View>
  );
}

/** The numbered disc the viewer and exports put before each step. */
export function StepNumber({ number, size = 28 }: { number: number; size?: number }) {
  const theme = useTheme();
  return (
    <View
      style={{
        width: size,
        height: size,
        borderRadius: size / 2,
        backgroundColor: theme.brand,
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      <Text variant="small" color={theme.brandForeground} style={{ fontFamily: fonts.medium, fontVariant: ["tabular-nums"] }}>
        {number}
      </Text>
    </View>
  );
}

export function ErrorText({ children }: { children: string | null | undefined }) {
  const theme = useTheme();
  if (!children) return null;
  return (
    <Text variant="small" color={theme.destructive}>
      {children}
    </Text>
  );
}

/** A screen's background. */
export function Screen({ style, ...props }: ViewProps) {
  const theme = useTheme();
  return <View {...props} style={[{ flex: 1, backgroundColor: theme.background }, style]} />;
}
