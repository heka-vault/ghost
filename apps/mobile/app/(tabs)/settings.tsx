/**
 * Settings for this phone: the desk it's paired with, how a session behaves in the car, and
 * the one privacy fact that matters (the mic opens only after Start). The whole row is the
 * control.
 */
import { relativeTime } from "@ghost/ui";
import Constants from "expo-constants";
import { useRouter } from "expo-router";
import { Lock, Monitor, Mic } from "lucide-react-native";
import { type ReactNode, useState } from "react";
import { ScrollView, View } from "react-native";

import { useDesk } from "../../src/desk/context";
import { forgetPairing } from "../../src/phone/pairing";
import { setPref, usePhone } from "../../src/phone/store";
import { space, useColors } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { ConfirmDialog, Divider, LargeHead, LinkDot, linkLabel, List, Row, Screen, SwitchRow, viaLabel } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";

export default function Settings() {
  const c = useColors();
  const router = useRouter();
  const { link, via, lastCall } = useDesk();
  const { pairing, prefs } = usePhone();
  const { tone, title } = linkLabel(link);
  const [asking, setAsking] = useState(false);
  const version = Constants.expoConfig?.version ?? "0.1.0";

  const unpair = () => {
    setAsking(false);
    void forgetPairing();
    router.replace("/pair");
  };

  return (
    <Screen bottom={false}>
      <ScrollView contentContainerStyle={{ paddingHorizontal: space[5], paddingBottom: space[6] }}>
        <LargeHead title="Settings" />

        <Group title="Your desk">
          <Row
            top
            leading={<Monitor size={20} color={c.text2} />}
            title={<Text size="xl" numberOfLines={1}>{pairing?.workstation ?? "Not paired"}</Text>}
            support={
              pairing ? (
                <View style={{ gap: space[0.5] }}>
                  <View style={{ flexDirection: "row", alignItems: "center", gap: space[2] }}>
                    <LinkDot tone={tone} />
                    <Text size="md" color="text2">{title}</Text>
                  </View>
                  {pairing.addresses.map((a, i) => (
                    <Text key={a + i} size="md" color="text2">
                      <Text size="md" color="text2" mono>{a}</Text>
                      {i === 0 ? " · home Wi-Fi" : " · away, through your tunnel"}
                    </Text>
                  ))}
                  {via ? <Text size="md" color="text2">Reaching it over {viaLabel(via)} now</Text> : null}
                  <Text size="md" color="text2">Paired {relativeTime(pairing.pairedAt)}</Text>
                  {lastCall ? (
                    <Text size="md" color="text3">
                      Last call {relativeTime(lastCall.endedAt)}, {viaLabel(lastCall.via)}, {Math.round(lastCall.seconds)} s
                      {lastCall.lossPct !== null ? `, ${lastCall.lossPct.toFixed(1)}% lost` : ""}
                      {lastCall.jitterMs !== null ? `, jitter ${Math.round(lastCall.jitterMs)} ms` : ""}
                      {lastCall.rttMs !== null ? `, round trip ${Math.round(lastCall.rttMs)} ms` : ""}
                    </Text>
                  ) : null}
                </View>
              ) : undefined
            }
          />
        </Group>

        <Group title="Hands-free">
          <SwitchRow
            title="Start talking when Ghost opens"
            support="For “Hey Google, open Ghost” in the car: the mic opens as soon as the app is in front and your desk answers. Off, you tap the ghost."
            value={prefs.autoStart}
            onChange={(v) => setPref("autoStart", v)}
          />
        </Group>

        <Group title="During a session">
          <SwitchRow title="Captions" support="Show what Ghost says on screen. Off keeps the screen calm while you drive." value={prefs.captions} onChange={(v) => setPref("captions", v)} />
          <Divider />
          <SwitchRow title="Keep the screen on" support="Until you tap End." value={prefs.keepAwake} onChange={(v) => setPref("keepAwake", v)} />
          <Divider />
          <SwitchRow title="Buzz when Ghost is listening" support="A short vibration each time it’s your turn." value={prefs.haptics} onChange={(v) => setPref("haptics", v)} />
        </Group>

        <Group title="Ghost’s questions">
          <Row top leading={<Mic size={20} color={c.text2} />} title="Answer out loud" support={<Text size="md" color="text2">Ghost asks before a change you’ve set to ask first. Say yes or no; there is no button for it on the phone.</Text>} />
        </Group>

        <Group title="Microphone">
          <Row top leading={<Lock size={20} color={c.text2} />} title="Only after you tap Start" support={<Text size="md" color="text2">The mic closes when you tap End. This phone never listens in the background.</Text>} />
        </Group>

        <View style={{ marginTop: space[6], gap: space[3], alignItems: "stretch" }}>
          <TouchButton variant="danger-ghost" fullWidth onPress={() => setAsking(true)}>
            Unpair this phone
          </TouchButton>
          <Text size="md" color="text2" align="center">Ghost for Android {version} · mock desk</Text>
        </View>
      </ScrollView>

      <ConfirmDialog
        open={asking}
        title="Unpair this phone?"
        description={`It won’t be able to start sessions on ${pairing?.workstation ?? "your desk"} until you pair it again with a new code.`}
        confirmLabel="Unpair"
        onConfirm={unpair}
        onClose={() => setAsking(false)}
      />
    </Screen>
  );
}

function Group({ title, children }: { title: string; children: ReactNode }) {
  return (
    <View style={{ marginTop: space[4] }}>
      <Text size="md" weight="semibold" color="text2" accessibilityRole="header" style={{ marginBottom: space[2] }}>
        {title}
      </Text>
      <List>{children}</List>
    </View>
  );
}
