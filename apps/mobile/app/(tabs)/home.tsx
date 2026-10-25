/**
 * Home: the ghost is the Start control: it floats while Ghost is ready, and a tap opens the
 * mic into a session on the main line. Under it, the other lines (threads, ADR 0014) as a
 * dial pad: three big keys to a row, each a ghost in the line's own colour over its name, so
 * a line is known by colour and a press, not a scroll, calls it directly (the session opens
 * there, greeting as that line). Nothing listens here; the mic opens only after a tap.
 * Activity has the history. While Ghost is asking, the question is the hero.
 */
import { errorText, lineHue, lineName, splitSummary } from "@ghost/ui";
import { useRouter } from "expo-router";
import { AudioLines } from "lucide-react-native";
import { useEffect, useState } from "react";
import { Modal, Pressable, ScrollView, useWindowDimensions, View } from "react-native";

import { useDesk } from "../../src/desk/context";
import { update, usePhone } from "../../src/phone/store";
import { opacityDisabled, radius, space, useColors } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { GhostGlyph } from "../../src/ui/Brand";
import { TargetText } from "../../src/ui/entries";
import { Cta, Divider, LinkDot, linkLabel, List, Row, Screen, Skeleton, viaLabel } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";
import { VoicePresence } from "../../src/ui/VoicePresence";

/** Decided once per app launch, so turning the preference on later doesn't start a session on the spot. */
let launchChecked = false;
let launchWantsStart = false;

export default function Home() {
  const router = useRouter();
  const { desk, link, status, pending, phase, online, loadingModels, inSession, deskBusy, via, conversations } = useDesk();
  const phone = usePhone();
  const { pairing } = phone;
  const { tone, title } = linkLabel(link);
  const [starting, setStarting] = useState(false);
  const [wave, setWave] = useState(0);
  const [retrying, setRetrying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const line = status?.conversation && status.conversation !== "ghost" ? `${lineName(status.conversation)} agent` : "Ghost";
  const lines = conversations.filter((c) => !c.retired_at);
  // The dial pad groups by department (ADR 0024): Ghost, then a key per department (its own
  // agent); the agents hired into it sit behind the key, in a sheet, so the pad stays one
  // screen of big keys.
  const keys = lines.filter((c) => c.is_default);
  const topicsOf = (slug: string) => lines.filter((c) => c.department === slug && !c.is_default);
  const [opened, setOpened] = useState<string | null>(null);
  const openedLine = opened ? (lines.find((c) => c.slug === opened) ?? null) : null;
  const colors = useColors();
  // Three keys to a row, like a dial pad: big enough that a thumb never hits the wrong line.
  const { width } = useWindowDimensions();
  const keyGap = space[3];
  const keySize = Math.floor((width - 2 * space[5] - 2 * keyGap) / 3);
  const workstation = pairing?.workstation ?? "your desk";
  const ready = online && !loadingModels && !deskBusy && !inSession;

  const start = async (conversation?: string) => {
    if (!ready || starting) return;
    // Wisp waves while the desk opens the session.
    setWave((w) => w + 1);
    setStarting(true);
    setError(null);
    try {
      await desk.start(conversation);
      update({ startedAt: new Date().toISOString(), muted: false });
      router.push("/session");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setStarting(false);
    }
  };

  const retry = async () => {
    setRetrying(true);
    setError(null);
    try {
      await desk.retry();
    } catch {
      setError("Still can’t reach it. The phone keeps trying on its own.");
    } finally {
      setRetrying(false);
    }
  };

  // Hands-free: opened by voice with the preference on, the first Home that can start, does.
  useEffect(() => {
    if (!phone.hydrated) return;
    if (!launchChecked) {
      launchChecked = true;
      launchWantsStart = phone.prefs.autoStart;
    }
    if (launchWantsStart && ready && !starting) {
      launchWantsStart = false;
      void start();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- start is stable enough for a one-shot
  }, [phone.hydrated, phone.prefs.autoStart, ready, starting]);

  let hero: { presence: Parameters<typeof VoicePresence>[0]["phase"]; title: string; body: string; question?: boolean };
  if (link === "offline") {
    hero = { presence: "offline", title: "Can’t reach your desk", body: `Ghost isn’t answering on ${workstation}. Check that it’s running and that this phone is on the same Wi-Fi.` };
  } else if (inSession && pending) {
    hero = { presence: phase, title: "Ghost is waiting on you", body: splitSummary(pending).text, question: true };
  } else if (inSession) {
    hero = { presence: phase, title: "You’re in a session", body: `With ${line}. The mic is open on this phone.` };
  } else if (deskBusy) {
    hero = { presence: phase, title: "Ghost is busy at your desk", body: `A session is running with ${line} at ${workstation}. Start here once it ends.` };
  } else if (link === "reconnecting") {
    hero = { presence: "offline", title: "Lost your desk for a moment", body: "Start comes back as soon as the phone reaches it again." };
  } else if (loadingModels) {
    hero = { presence: "starting", title: "Getting ready", body: "Ghost is loading its voice models. Usually under a minute." };
  } else if (starting) {
    hero = { presence: "starting", title: "Opening the mic", body: "" };
  } else {
    hero = { presence: "passive", title: "Tap the ghost and talk", body: "The mic stays off until you do." };
  }

  return (
    <Screen bottom={false}>
      <View style={{ flexDirection: "row", alignItems: "center", gap: space[3], paddingTop: space[3], paddingHorizontal: space[5], paddingBottom: space[2] }}>
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text size="xl" weight="semibold" accessibilityRole="header">Ghost</Text>
          <View accessibilityRole="text" accessibilityLiveRegion="polite" style={{ flexDirection: "row", alignItems: "center", gap: space[2] }}>
            <LinkDot tone={tone} />
            <Text size="md" color="text2" numberOfLines={1} style={{ flexShrink: 1 }}>
              {title}
              {link === "online" && pairing ? <Text size="md" color="text3">{` · ${pairing.workstation}${via ? ` · ${viaLabel(via)}` : ""}`}</Text> : null}
            </Text>
          </View>
        </View>
      </View>

      <ScrollView contentContainerStyle={{ flexGrow: 1 }} keyboardShouldPersistTaps="handled">
      <View style={{ flex: 1, alignItems: "center", justifyContent: "center", paddingHorizontal: space[6], paddingVertical: space[8] }}>
        {link === "connecting" ? (
          <View style={{ alignItems: "center", gap: space[4] }}>
            <Skeleton width={320} height={320} round />
            <Skeleton width={220} height={28} />
          </View>
        ) : (
          <>
            {/* The ghost is the button: the 320 hero, the screen's main control. It floats only while a tap would start. */}
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Start session"
              accessibilityHint="Opens the mic on this phone"
              accessibilityState={{ disabled: !ready, busy: starting }}
              disabled={!ready}
              onPress={() => start()}
              hitSlop={24}
              style={({ pressed }) => ({ opacity: ready || inSession ? (pressed ? 0.82 : 1) : opacityDisabled, transform: [{ scale: pressed && ready ? 0.96 : 1 }] })}
            >
              <VoicePresence phase={hero.presence} size={320} label={hero.question ? undefined : ""} float={ready && !starting} wave={wave} />
            </Pressable>
            <Text size="3xl" weight="semibold" align="center" style={{ marginTop: space[6] }}>
              {hero.title}
            </Text>
            {hero.body ? (
              <Text size="lg" color={hero.question ? "text1" : "text2"} align="center" numberOfLines={hero.question ? 3 : undefined} style={{ marginTop: space[2], maxWidth: 320 }}>
                {hero.body}
              </Text>
            ) : null}
            {hero.question && pending ? (
              <View style={{ marginTop: space[2] }}>
                <TargetText {...splitSummary(pending)} size="lg" />
              </View>
            ) : null}
          </>
        )}
      </View>

      {/* The dial pad: Ghost and a key per department. A press calls straight into it; a department with topic lines opens them in a sheet first. */}
      {keys.length > 1 && !inSession && link !== "connecting" ? (
        <View style={{ paddingHorizontal: space[5], paddingBottom: space[6] }}>
          <Text size="md" weight="semibold" color="text2" style={{ marginBottom: space[3], marginLeft: space[1] }}>
            Lines
          </Text>
          <View style={{ flexDirection: "row", flexWrap: "wrap", gap: keyGap, opacity: ready ? 1 : opacityDisabled }}>
            {keys.map((c) => {
              const hue = colors.hue[lineHue(c.slug)];
              const name = lineName(c.slug, c.name);
              const topics = c.is_ghost ? [] : topicsOf(c.slug);
              return (
                <Pressable
                  key={c.slug}
                  accessibilityRole="button"
                  accessibilityLabel={topics.length ? `${name}: ${topics.length === 1 ? "one line" : `${topics.length} agents`}` : `Call ${name}`}
                  accessibilityHint={topics.length ? "Shows the department and its agents; pick one to call" : "Opens the mic on this phone, with that agent"}
                  accessibilityState={{ disabled: !ready }}
                  disabled={!ready}
                  onPress={() => (topics.length ? setOpened(c.slug) : void start(c.slug))}
                  style={({ pressed }) => ({
                    width: keySize,
                    height: keySize,
                    borderRadius: radius.lg,
                    borderWidth: 1,
                    borderColor: pressed ? hue.fg : colors.border,
                    backgroundColor: pressed ? hue.bg : colors.bgSurface,
                    alignItems: "center",
                    justifyContent: "center",
                    gap: space[2],
                    paddingHorizontal: space[2],
                    transform: [{ scale: pressed ? 0.97 : 1 }],
                  })}
                >
                  <GhostGlyph size={Math.round(keySize * 0.4)} color={hue.fg} />
                  <Text size="md" weight="semibold" align="center" numberOfLines={2} style={{ color: colors.text1 }}>
                    {name}
                  </Text>
                  {topics.length > 0 ? (
                    <Text size="sm" color="text3" align="center" numberOfLines={1}>
                      {topics.length === 1 ? "1 agent" : `${topics.length} agents`}
                    </Text>
                  ) : null}
                </Pressable>
              );
            })}
          </View>
        </View>
      ) : null}
      </ScrollView>

      {/* A department's lines: the department's own line first, then each topic line; a tap calls it. */}
      <Modal visible={!!openedLine} transparent animationType="fade" onRequestClose={() => setOpened(null)} statusBarTranslucent>
        <Pressable accessible={false} onPress={() => setOpened(null)} style={{ flex: 1, backgroundColor: colors.bgOverlay, justifyContent: "flex-end" }}>
          <Pressable accessible={false} onPress={() => {}} style={{ backgroundColor: colors.bgRaised, borderTopLeftRadius: radius.xl, borderTopRightRadius: radius.xl, padding: space[5], paddingBottom: space[8], gap: space[3], borderWidth: 1, borderColor: colors.border }}>
            {openedLine ? (
              <>
                <Text size="xl" weight="semibold" accessibilityRole="header">
                  {lineName(openedLine.slug, openedLine.name)}
                </Text>
                <List>
                  {[openedLine, ...topicsOf(openedLine.slug)].map((c, i) => {
                    const name = lineName(c.slug, c.name);
                    return (
                      <View key={c.slug}>
                        {i > 0 && <Divider />}
                        <Row
                          leading={<GhostGlyph size={28} color={colors.hue[lineHue(c.slug)].fg} />}
                          title={name}
                          support={<Text size="md" color="text2">{i === 0 ? "The department’s own agent" : `An agent in ${lineName(openedLine.slug, openedLine.name)}`}</Text>}
                          accessibilityLabel={`Call ${name}`}
                          onPress={() => {
                            setOpened(null);
                            void start(c.slug);
                          }}
                        />
                      </View>
                    );
                  })}
                </List>
                <TouchButton variant="ghost" fullWidth onPress={() => setOpened(null)}>
                  Cancel
                </TouchButton>
              </>
            ) : null}
          </Pressable>
        </Pressable>
      </Modal>

      <Cta>
        {error ? (
          <Text size="md" color={link === "offline" ? "text2" : "danger"} align="center" accessibilityRole="alert">
            {error}
          </Text>
        ) : null}
        {link === "offline" ? (
          <TouchButton size="xl" fullWidth loading={retrying} onPress={retry}>
            Try again
          </TouchButton>
        ) : inSession ? (
          <TouchButton variant="primary" size="xl" fullWidth icon={(col) => <AudioLines size={22} color={col} />} onPress={() => router.push("/session")}>
            Back to the session
          </TouchButton>
        ) : null}
      </Cta>
    </Screen>
  );
}
