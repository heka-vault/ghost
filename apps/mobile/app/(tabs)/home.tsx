/**
 * Home: one thing to do (Start session), how the phone reaches the desk, and the last two
 * decisions. Start sits low, in thumb reach. Nothing listens here: the mic opens only after
 * Start. While Ghost is asking, the question is the hero and only settled decisions make the
 * list, so the screen never says the same thing twice.
 */
import type { AuditEntry } from "@ghost/client-ts";
import { errorText, lineName, splitSummary } from "@ghost/ui";
import { useRouter } from "expo-router";
import { AudioLines, ChevronRight, Inbox, Mic } from "lucide-react-native";
import { useState } from "react";
import { Pressable, ScrollView, View } from "react-native";

import { useDesk } from "../../src/desk/context";
import { update, usePhone } from "../../src/phone/store";
import { space, useColors } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { GhostMark } from "../../src/ui/Brand";
import { EntryRow, TargetText } from "../../src/ui/entries";
import { Cta, Divider, EmptyState, LinkDot, linkLabel, List, Screen, SectionHead, Skeleton } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";
import { VoicePresence } from "../../src/ui/VoicePresence";

const RECENT = 2;

export default function Home() {
  const c = useColors();
  const router = useRouter();
  const { desk, link, status, pending, audit, phase, online, loadingModels, inSession, deskBusy } = useDesk();
  const { pairing } = usePhone();
  const { tone, title } = linkLabel(link);
  const [starting, setStarting] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const settled = (items: AuditEntry[] | null) => items?.filter((e) => e.status !== "pending" && e.id !== pending?.id).slice(0, RECENT) ?? null;
  const recent = settled(audit);
  const stale = link === "reconnecting" || link === "offline";
  const line = status?.conversation && status.conversation !== "ghost" ? `${lineName(status.conversation)} line` : "main line";
  const workstation = pairing?.workstation ?? "your desk";

  const start = async () => {
    setStarting(true);
    setError(null);
    try {
      await desk.start();
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

  let hero: { presence: Parameters<typeof VoicePresence>[0]["phase"]; title: string; body: string; question?: boolean };
  if (link === "offline") {
    hero = { presence: "offline", title: "Can’t reach your desk", body: `Ghost isn’t answering on ${workstation}. Check that it’s running and that this phone is on the same Wi-Fi.` };
  } else if (inSession && pending) {
    hero = { presence: phase, title: "Ghost is waiting on you", body: splitSummary(pending).text, question: true };
  } else if (inSession) {
    hero = { presence: phase, title: "You’re in a session", body: `On the ${line}. The mic is open on this phone.` };
  } else if (deskBusy) {
    hero = { presence: phase, title: "Ghost is busy at your desk", body: `A session is running on the ${line} at ${workstation}. Start here once it ends.` };
  } else if (link === "reconnecting") {
    hero = { presence: "offline", title: "Lost your desk for a moment", body: "Start comes back as soon as the phone reaches it again." };
  } else if (loadingModels) {
    hero = { presence: "starting", title: "Getting ready", body: "Ghost is loading its voice models. Usually under a minute." };
  } else {
    hero = { presence: "passive", title: "Ready when you are", body: "Tap Start and talk. The mic stays off until you do." };
  }

  return (
    <Screen bottom={false}>
      <View style={{ flexDirection: "row", alignItems: "center", gap: space[3], paddingTop: space[3], paddingHorizontal: space[5], paddingBottom: space[2] }}>
        <GhostMark size={32} />
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text size="xl" weight="semibold" accessibilityRole="header">Ghost</Text>
          <View accessibilityRole="text" accessibilityLiveRegion="polite" style={{ flexDirection: "row", alignItems: "center", gap: space[2] }}>
            <LinkDot tone={tone} />
            <Text size="md" color="text2" numberOfLines={1} style={{ flexShrink: 1 }}>
              {title}
              {link === "online" && pairing ? <Text size="md" color="text3">{` · ${pairing.workstation} · ${pairing.via}`}</Text> : null}
            </Text>
          </View>
        </View>
      </View>

      <ScrollView contentContainerStyle={{ paddingHorizontal: space[5], paddingBottom: space[4] }}>
        <View style={{ alignItems: "center", paddingTop: space[6], paddingHorizontal: space[2], paddingBottom: space[5] }}>
          {link === "connecting" ? (
            <View style={{ alignItems: "center", gap: space[4] }}>
              <Skeleton width={96} height={96} round />
              <Skeleton width={220} height={28} />
              <Skeleton width={260} height={16} />
            </View>
          ) : (
            <>
              <VoicePresence phase={hero.presence} size={96} label={hero.question ? undefined : ""} />
              <Text size="3xl" weight="semibold" align="center" style={{ marginTop: space[5] }}>
                {hero.title}
              </Text>
              <Text size="lg" color={hero.question ? "text1" : "text2"} align="center" numberOfLines={hero.question ? 3 : undefined} style={{ marginTop: space[2], maxWidth: 320 }}>
                {hero.body}
              </Text>
              {hero.question && pending ? (
                <View style={{ marginTop: space[2] }}>
                  <TargetText {...splitSummary(pending)} size="lg" />
                </View>
              ) : null}
            </>
          )}
        </View>

        {link !== "connecting" && recent && recent.length > 0 && (
          <View style={{ marginTop: space[2] }}>
            <SectionHead
              title="Recent decisions"
              trailing={
                <Pressable accessibilityRole="link" onPress={() => router.navigate("/activity")} hitSlop={12} style={{ flexDirection: "row", alignItems: "center", gap: space[0.5], minHeight: space[12] }}>
                  <Text size="lg" weight="medium" color="accentText">See all</Text>
                  <ChevronRight size={16} color={c.accentText} />
                </Pressable>
              }
            />
            {stale && <Text size="md" color="text2" style={{ marginBottom: space[2] }}>Showing what this phone saw last.</Text>}
            <List>
              {recent.map((e, i) => (
                <View key={e.id}>
                  {i > 0 && <Divider />}
                  <EntryRow e={e} onPress={() => router.navigate({ pathname: "/activity", params: { open: e.id } })} />
                </View>
              ))}
            </List>
          </View>
        )}
        {online && recent && recent.length === 0 && (
          <View style={{ marginTop: space[2] }}>
            <SectionHead title="Recent decisions" />
            <List>
              <EmptyState icon={(color) => <Inbox size={24} color={color} />} title="Nothing yet" description="When Ghost changes something, or asks to, it shows up here." />
            </List>
          </View>
        )}
      </ScrollView>

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
        ) : (
          <TouchButton variant="primary" size="xl" fullWidth icon={(col) => <Mic size={22} color={col} />} loading={starting} disabled={!online || loadingModels || deskBusy} onPress={start}>
            Start session
          </TouchButton>
        )}
      </Cta>
    </Screen>
  );
}
