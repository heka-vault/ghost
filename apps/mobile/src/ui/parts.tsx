/**
 * Phone building blocks, as the prototype draws them (apps/desktop/src/prototypes/mobile/parts.tsx
 * and mobile.css): 56dp rows in a bordered list, a 48dp back bar, the link dot, notices,
 * empty states, skeletons, a disclosure, the confirm dialog, and two clocks.
 */
import type { Tone } from "@ghost/ui";
import { useRouter } from "expo-router";
import { ChevronDown, ChevronLeft } from "lucide-react-native";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { Animated, Modal, Pressable, type StyleProp, Switch, View, type ViewStyle } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import type { Link, Via } from "../desk/types";
import { type Colors, radius, space, useColors } from "../theme";
import { TouchButton } from "./Button";
import { Text } from "./Text";

/* ------------------------------------------------------------------ screen */

/** A screen's root: the canvas, inside the status bar. The bottom inset belongs to the tab bar or the CTA. */
export function Screen({ children, bottom = true }: { children: ReactNode; bottom?: boolean }) {
  const c = useColors();
  return (
    <SafeAreaView edges={bottom ? ["top", "bottom", "left", "right"] : ["top", "left", "right"]} style={{ flex: 1, backgroundColor: c.bgCanvas }}>
      {children}
    </SafeAreaView>
  );
}

/** The bottom action area: Start, Done, Pair. */
export function Cta({ children }: { children: ReactNode }) {
  return <View style={{ gap: space[2], paddingHorizontal: space[5], paddingTop: space[3], paddingBottom: space[4] }}>{children}</View>;
}

/* ------------------------------------------------------------------ top bar */

/** A 48dp back button and the screen title. Without a title it is only the back button. */
export function TopBar({ title, back }: { title?: string; back?: () => void }) {
  const router = useRouter();
  const c = useColors();
  return (
    <View style={{ flexDirection: "row", alignItems: "center", gap: space[1], minHeight: space[12] + space[2], paddingHorizontal: space[2] }}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel="Back"
        onPress={back ?? (() => (router.canGoBack() ? router.back() : router.replace("/home")))}
        style={({ pressed }) => ({ width: space[12], height: space[12], borderRadius: radius.full, alignItems: "center", justifyContent: "center", backgroundColor: pressed ? c.bgActive : "transparent" })}
      >
        <ChevronLeft size={24} color={c.text1} />
      </Pressable>
      {title ? (
        <Text size="xl" weight="semibold" accessibilityRole="header" style={{ flex: 1 }}>
          {title}
        </Text>
      ) : null}
    </View>
  );
}

/** A tab screen's large title, 32/40 like the prototype's .mob-largehead. */
export function LargeHead({ title, sub }: { title: string; sub?: string }) {
  return (
    <View style={{ paddingTop: space[4], paddingBottom: space[4], gap: space[1] }}>
      <Text size="4xl" weight="semibold" accessibilityRole="header">
        {title}
      </Text>
      {sub ? <Text size="lg" color="text2">{sub}</Text> : null}
    </View>
  );
}

/* ------------------------------------------------------------------ connection */

export function linkLabel(l: Link): { tone: "ok" | "warn" | "danger" | "neutral"; title: string } {
  if (l === "online") return { tone: "ok", title: "Connected" };
  if (l === "reconnecting") return { tone: "warn", title: "Reconnecting" };
  if (l === "offline") return { tone: "danger", title: "Not connected" };
  return { tone: "neutral", title: "Connecting…" };
}

/** "Wi-Fi" at home, "away" through the tunnel (ADR 0020). */
export function viaLabel(via: Via): string {
  return via === "tunnel" ? "away" : "Wi-Fi";
}

export function LinkDot({ tone }: { tone: ReturnType<typeof linkLabel>["tone"] }) {
  const c = useColors();
  const bg = { ok: c.ok, warn: c.warn, danger: c.danger, neutral: c.phasePassive }[tone];
  return <View style={{ width: space[2], height: space[2], borderRadius: radius.full, backgroundColor: bg }} />;
}

/* ------------------------------------------------------------------ lists */

/** The bordered surface a set of rows sits on. */
export function List({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  const c = useColors();
  return <View style={[{ backgroundColor: c.bgSurface, borderWidth: 1, borderColor: c.border, borderRadius: radius.lg, overflow: "hidden" }, style]}>{children}</View>;
}

/** A hairline between rows; rows don't draw their own so the first never has one. */
export function Divider() {
  const c = useColors();
  return <View style={{ height: 1, backgroundColor: c.borderSubtle }} />;
}

/** A 56dp+ row: leading, title and supporting lines, trailing. Pressable when given onPress. */
export function Row({ leading, title, support, trailing, onPress, top, accessibilityLabel, expanded }: { leading?: ReactNode; title: ReactNode; support?: ReactNode; trailing?: ReactNode; onPress?: () => void; top?: boolean; accessibilityLabel?: string; expanded?: boolean }) {
  const c = useColors();
  const body = (pressed: boolean) => (
    <View style={{ flexDirection: "row", alignItems: top ? "flex-start" : "center", gap: space[3], minHeight: space[12] + space[2], paddingVertical: space[3], paddingHorizontal: space[4], backgroundColor: pressed ? c.bgActive : "transparent" }}>
      {leading ? <View style={{ marginTop: top ? space[1] : 0 }}>{leading}</View> : null}
      <View style={{ flex: 1, minWidth: 0, gap: space[0.5] }}>
        {typeof title === "string" ? <Text size="xl">{title}</Text> : title}
        {support}
      </View>
      {trailing}
    </View>
  );
  if (!onPress) return body(false);
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={accessibilityLabel} accessibilityState={expanded == null ? undefined : { expanded }} onPress={onPress}>
      {({ pressed }) => body(pressed)}
    </Pressable>
  );
}

/** The whole row is the switch. */
export function SwitchRow({ title, support, value, onChange }: { title: string; support?: string; value: boolean; onChange: (v: boolean) => void }) {
  const c = useColors();
  return (
    <Pressable accessibilityRole="switch" accessibilityState={{ checked: value }} accessibilityLabel={title} accessibilityHint={support} onPress={() => onChange(!value)}>
      {({ pressed }) => (
        <View style={{ flexDirection: "row", alignItems: "center", gap: space[3], minHeight: space[12] + space[2], paddingVertical: space[3], paddingHorizontal: space[4], backgroundColor: pressed ? c.bgActive : "transparent" }}>
          <View style={{ flex: 1, gap: space[0.5] }}>
            <Text size="xl">{title}</Text>
            {support ? <Text size="md" color="text2">{support}</Text> : null}
          </View>
          <Switch
            value={value}
            onValueChange={onChange}
            importantForAccessibility="no"
            trackColor={{ false: c.controlTrack, true: c.accent }}
            thumbColor="#ffffff"
          />
        </View>
      )}
    </Pressable>
  );
}

/** A section title above a list, 16/24 semibold, with an optional trailing link. */
export function SectionHead({ title, trailing }: { title: ReactNode; trailing?: ReactNode }) {
  return (
    <View style={{ flexDirection: "row", alignItems: "center", justifyContent: "space-between", minHeight: space[12] }}>
      {typeof title === "string" ? (
        <Text size="xl" weight="semibold" accessibilityRole="header">
          {title}
        </Text>
      ) : (
        title
      )}
      {trailing}
    </View>
  );
}

/* ------------------------------------------------------------------ badges and notices */

function toneColors(c: Colors, tone: Tone): { fg: string; bg: string; border: string } {
  switch (tone) {
    case "ok":
      return { fg: c.ok, bg: c.okSoft, border: c.okBorder };
    case "warn":
      return { fg: c.warn, bg: c.warnSoft, border: c.warnBorder };
    case "danger":
      return { fg: c.danger, bg: c.dangerSoft, border: c.dangerBorder };
    case "info":
      return { fg: c.info, bg: c.infoSoft, border: c.infoBorder };
    case "accent":
      return { fg: c.accentText, bg: c.accentSoft, border: c.accentBorder };
    case "neutral":
      return { fg: c.text2, bg: c.bgActive, border: c.border };
  }
}

/** A small status pill with a dot: the desktop Badge at size sm. */
export function Badge({ tone, children }: { tone: Tone; children: string }) {
  const c = useColors();
  const t = toneColors(c, tone);
  return (
    <View style={{ flexDirection: "row", alignItems: "center", gap: space[1], height: 20, paddingHorizontal: space[2], borderRadius: radius.full, backgroundColor: t.bg, alignSelf: "flex-start" }}>
      <View style={{ width: 6, height: 6, borderRadius: 3, backgroundColor: t.fg }} />
      <Text size="sm" weight="medium" style={{ color: tone === "neutral" ? c.text2 : t.fg }}>
        {children}
      </Text>
    </View>
  );
}

export function Notice({ tone, title, children }: { tone: Tone; title: string; children?: ReactNode }) {
  const c = useColors();
  const t = toneColors(c, tone);
  return (
    <View accessibilityRole={tone === "danger" ? "alert" : undefined} style={{ borderWidth: 1, borderColor: t.border, backgroundColor: t.bg, borderRadius: radius.md, padding: space[4], gap: space[1] }}>
      <Text size="lg" weight="semibold">{title}</Text>
      {children ? <Text size="lg" color="text2">{children}</Text> : null}
    </View>
  );
}

export function EmptyState({ icon, title, description, action }: { icon: (color: string) => ReactNode; title: string; description: string; action?: ReactNode }) {
  const c = useColors();
  return (
    <View style={{ alignItems: "center", paddingVertical: space[8], paddingHorizontal: space[6], gap: space[2] }}>
      <View style={{ width: space[12], height: space[12], borderRadius: radius.lg, backgroundColor: c.bgActive, alignItems: "center", justifyContent: "center", marginBottom: space[2] }}>
        {icon(c.text2)}
      </View>
      <Text size="xl" weight="semibold" align="center">{title}</Text>
      <Text size="lg" color="text2" align="center">{description}</Text>
      {action ? <View style={{ marginTop: space[3] }}>{action}</View> : null}
    </View>
  );
}

/** A pulsing placeholder shaped like the content. */
export function Skeleton({ width, height, round }: { width: number | `${number}%`; height: number; round?: boolean }) {
  const c = useColors();
  const pulse = useRef(new Animated.Value(0.6)).current;
  useEffect(() => {
    const a = Animated.loop(Animated.sequence([Animated.timing(pulse, { toValue: 1, duration: 700, useNativeDriver: true }), Animated.timing(pulse, { toValue: 0.6, duration: 700, useNativeDriver: true })]));
    a.start();
    return () => a.stop();
  }, [pulse]);
  return <Animated.View style={{ width, height, borderRadius: round ? radius.full : radius.xs, backgroundColor: c.bgActive, opacity: pulse }} />;
}

/* ------------------------------------------------------------------ disclosure */

/** A 48dp disclosure set like a section heading: the label on the content edge, the chevron at the end. */
export function Disclosure({ summary, children }: { summary: string; children: ReactNode }) {
  const c = useColors();
  const [open, setOpen] = useState(false);
  return (
    <View>
      <Pressable accessibilityRole="button" accessibilityState={{ expanded: open }} onPress={() => setOpen(!open)} style={{ flexDirection: "row", alignItems: "center", justifyContent: "space-between", minHeight: space[12] }}>
        <Text size="xl" weight="semibold">{summary}</Text>
        <View style={{ transform: [{ rotate: open ? "180deg" : "0deg" }] }}>
          <ChevronDown size={20} color={c.text2} />
        </View>
      </Pressable>
      {open ? <View style={{ paddingTop: space[2] }}>{children}</View> : null}
    </View>
  );
}

/* ------------------------------------------------------------------ dialog */

/** A confirm dialog with 48dp buttons, Cancel first so a stray tap never does the risky thing. */
export function ConfirmDialog({ open, title, description, confirmLabel, onConfirm, onClose }: { open: boolean; title: string; description: string; confirmLabel: string; onConfirm: () => void; onClose: () => void }) {
  const c = useColors();
  return (
    <Modal visible={open} transparent animationType="fade" onRequestClose={onClose} statusBarTranslucent>
      <Pressable accessible={false} onPress={onClose} style={{ flex: 1, backgroundColor: c.bgOverlay, justifyContent: "center", padding: space[6] }}>
        <Pressable accessible={false} onPress={() => {}} style={{ backgroundColor: c.bgRaised, borderRadius: radius.xl, padding: space[6], gap: space[3], borderWidth: 1, borderColor: c.border }}>
          <Text size="2xl" weight="semibold" accessibilityRole="header">{title}</Text>
          <Text size="lg" color="text2">{description}</Text>
          <View style={{ flexDirection: "row", gap: space[3], marginTop: space[3] }}>
            <View style={{ flex: 1 }}>
              <TouchButton variant="secondary" fullWidth onPress={onClose}>Cancel</TouchButton>
            </View>
            <View style={{ flex: 1 }}>
              <TouchButton variant="danger" fullWidth onPress={onConfirm}>{confirmLabel}</TouchButton>
            </View>
          </View>
        </Pressable>
      </Pressable>
    </Modal>
  );
}

/* ------------------------------------------------------------------ time */

/** Minutes and seconds since `iso`, ticking once a second. */
export function useElapsed(iso: string | null | undefined): string {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const h = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(h);
  }, []);
  if (!iso) return "0:00";
  const s = Math.max(0, Math.floor((now - Date.parse(iso)) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** “6 min”, “1 h 12 min”, “under a minute”. */
export function durationWords(fromIso: string, toIso: string): string {
  const m = Math.round((Date.parse(toIso) - Date.parse(fromIso)) / 60_000);
  if (m < 1) return "under a minute";
  if (m < 60) return `${m} min`;
  return `${Math.floor(m / 60)} h ${m % 60} min`;
}

/** Seconds since the phone lost the desk, ticking; null while connected. */
export function useLostFor(lost: boolean): number | null {
  const [since, setSince] = useState<number | null>(() => (lost ? Date.now() : null));
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!lost) {
      setSince(null);
      return;
    }
    setSince((s) => s ?? Date.now());
    const h = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(h);
  }, [lost]);
  return lost && since != null ? Math.max(0, Math.floor((now - since) / 1000)) : null;
}
