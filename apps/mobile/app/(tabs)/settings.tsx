/**
 * Settings for this phone: the desk it's paired with, the live connection that brings
 * questions and calls with the app closed (ADR 0051) and the permissions behind it, how a
 * session behaves in the car, and the one privacy fact that matters (the mic opens only after
 * Start). The whole row is the control.
 */
import { relativeTime } from "@ghost/ui";
import Constants from "expo-constants";
import { useRouter } from "expo-router";
import { Bell, Lock, Monitor, Mic, PhoneIncoming } from "lucide-react-native";
import { type ReactNode, useState } from "react";
import { ScrollView, View } from "react-native";

import { openFullScreenSettings, retryLink, setLinkBootStart, setLinkEnabled } from "../../modules/ghost-call/link";
import { useDesk } from "../../src/desk/context";
import { liveLine } from "../../src/desk/waiting";
import { askForNotifications, usePermissions } from "../../src/phone/notify";
import { forgetPairing, MOCK_DESK } from "../../src/phone/pairing";
import { setPref, usePhone } from "../../src/phone/store";
import { space, useColors } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { ConfirmDialog, Divider, LargeHead, LinkDot, linkLabel, List, Row, Screen, SwitchRow, viaLabel } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";
import { version as packageVersion } from "../../package.json";

export default function Settings() {
  const c = useColors();
  const router = useRouter();
  const { link, via, address, lastCall, live } = useDesk();
  const [perms, refreshPerms] = usePermissions();
  const { pairing, prefs } = usePhone();
  const { tone, title } = linkLabel(link);
  const [asking, setAsking] = useState(false);
  const version = Constants.expoConfig?.version ?? packageVersion;

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
                  {via ? (
                    <Text size="md" color="text2">
                      Reaching it over {viaLabel(via)} now{address ? ", at " : ""}
                      {address ? <Text size="md" color="text2" mono>{address}</Text> : null}
                    </Text>
                  ) : null}
                  <Text size="md" color="text2">Paired {relativeTime(pairing.pairedAt)} · the one phone your desk pairs with</Text>
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

        <Group title="Questions and calls with the app closed">
          <Row
            top
            title={<Text size="xl">Connection to your desk</Text>}
            support={
              <View style={{ gap: space[0.5] }}>
                <View style={{ flexDirection: "row", alignItems: "center", gap: space[2] }}>
                  <LinkDot tone={live.state === "connected" ? "ok" : live.state === "refused" || live.state === "offline" ? "danger" : "neutral"} />
                  <Text size="md" color="text2" style={{ flexShrink: 1 }}>{liveLine(live)}</Text>
                </View>
                {live.heardAt && live.state !== "connected" ? <Text size="md" color="text3">Last heard {relativeTime(live.heardAt)}</Text> : null}
              </View>
            }
            trailing={
              live.state !== "connected" && live.state !== "unavailable" ? (
                <TouchButton size="md" variant="ghost" onPress={retryLink}>
                  Retry
                </TouchButton>
              ) : null
            }
          />
          <Divider />
          <SwitchRow
            title="Stay connected"
            support="Questions, changes to confirm and calls from Ghost reach this phone with the app closed. Android shows a quiet notification while it’s on. Off, they arrive only while the app is open."
            value={live.enabled}
            onChange={(v) => setLinkEnabled(v)}
          />
          <Divider />
          <SwitchRow
            title="Start when the phone starts"
            support="Reconnect after a restart without opening the app."
            value={live.bootStart}
            onChange={(v) => setLinkBootStart(v)}
          />
          <Divider />
          <Row
            top
            leading={<Bell size={20} color={c.text2} />}
            title={<Text size="xl">Notifications</Text>}
            support={
              <Text size="md" color="text2">
                {perms == null
                  ? "Not available in this build."
                  : perms.notifications
                    ? perms.asks
                      ? "On. Each kind can be changed in Android’s settings."
                      : "On, but questions and changes are turned off in Android’s settings."
                    : "Off. Ghost can’t tell you when something needs you, so your desk tries its next way to reach you."}
              </Text>
            }
            trailing={
              perms && (!perms.notifications || !perms.asks) ? (
                <TouchButton size="md" onPress={() => void askForNotifications().then(refreshPerms)}>
                  Turn on
                </TouchButton>
              ) : null
            }
          />
          <Divider />
          <Row
            top
            leading={<PhoneIncoming size={20} color={c.text2} />}
            title={<Text size="xl">Calls over the lock screen</Text>}
            support={
              <Text size="md" color="text2">
                {perms == null
                  ? "Not available in this build."
                  : perms.fullScreen && perms.calls
                    ? "Allowed. When your desk rings, the call fills the screen even when the phone is locked."
                    : !perms.calls
                      ? "Calls from Ghost are turned off in Android’s settings."
                      : "Not allowed. A call from Ghost shows as a notification only, and the phone won’t light up for it."}
              </Text>
            }
            trailing={
              perms && (!perms.fullScreen || !perms.calls) ? (
                <TouchButton size="md" onPress={openFullScreenSettings}>
                  Allow
                </TouchButton>
              ) : null
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
          <Row
            top
            leading={<Mic size={20} color={c.text2} />}
            title="Out loud, or here"
            support={<Text size="md" color="text2">In a session, say yes or no when Ghost asks. Outside one, a change waiting for your yes is on Home and in a notification with Yes and No, and an agent’s question takes a tap or your own words.</Text>}
          />
        </Group>

        <Group title="Microphone">
          <Row top leading={<Lock size={20} color={c.text2} />} title="Only after you tap Start" support={<Text size="md" color="text2">The mic closes when you tap End. This phone never listens in the background.</Text>} />
        </Group>

        <View style={{ marginTop: space[6], gap: space[3], alignItems: "stretch" }}>
          <TouchButton variant="danger-ghost" fullWidth onPress={() => setAsking(true)}>
            Unpair this phone
          </TouchButton>
          <Text size="md" color="text2" align="center">Ghost for Android {version}{MOCK_DESK ? " · mock desk" : ""}</Text>
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
