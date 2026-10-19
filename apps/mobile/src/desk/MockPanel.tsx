/**
 * The mock desk's controls: the voice side of a session (the phone has no mic path yet) and
 * the network. A small “Mock” pill at the top center opens it, where no screen puts a control.
 * Pairing has its own mock affordance, so the pill stays out of the way there. Gone once the
 * real desk lands.
 */
import { usePathname } from "expo-router";
import { useState } from "react";
import { Modal, Pressable, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { radius, space, useColors } from "../theme";
import { TouchButton } from "../ui/Button";
import { Text } from "../ui/Text";
import { useDesk } from "./context";
import type { MockAct, MockDesk } from "./mock";

const ACTS: { act: MockAct; label: string; when: string }[] = [
  { act: "ask", label: "Ghost asks to close infra#77", when: "In a session" },
  { act: "yes", label: "Say “yeah, go ahead”", when: "While Ghost is asking" },
  { act: "no", label: "Say “no, leave it”", when: "While Ghost is asking" },
  { act: "talk", label: "Ask Ghost something", when: "In a session" },
  { act: "drop", label: "Lose the desk for 6 seconds", when: "Any time" },
  { act: "deskSession", label: "Session starts or ends at the desk", when: "Out of a session" },
  { act: "deskDown", label: "Desk stops answering", when: "Any time" },
  { act: "deskUp", label: "Desk comes back", when: "After it stopped" },
];

export function MockPanel({ desk }: { desk: MockDesk }) {
  const c = useColors();
  const insets = useSafeAreaInsets();
  const [open, setOpen] = useState(false);
  const { inSession, pending, link } = useDesk();
  const pairing = usePathname().startsWith("/pair");

  const enabled = (a: MockAct) => {
    if (a === "ask" || a === "talk") return inSession && !pending && link === "online";
    if (a === "yes" || a === "no") return !!pending && link === "online";
    if (a === "deskUp") return link === "offline";
    return link === "online";
  };

  if (pairing) return null;

  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel="Mock desk controls"
        onPress={() => setOpen(true)}
        hitSlop={8}
        style={{ position: "absolute", top: insets.top + 2, alignSelf: "center", height: 22, paddingHorizontal: space[2], borderRadius: radius.full, backgroundColor: c.accentSoft, borderWidth: 1, borderColor: c.accentBorder, justifyContent: "center" }}
      >
        <Text size="xs" weight="semibold" color="accentText">Mock</Text>
      </Pressable>
      <Modal visible={open} transparent animationType="slide" onRequestClose={() => setOpen(false)}>
        <Pressable accessible={false} onPress={() => setOpen(false)} style={{ flex: 1, backgroundColor: c.bgOverlay, justifyContent: "flex-end" }}>
          <Pressable accessible={false} onPress={() => {}} style={{ backgroundColor: c.bgRaised, borderTopLeftRadius: radius.xl, borderTopRightRadius: radius.xl, padding: space[5], paddingBottom: insets.bottom + space[4], gap: space[2] }}>
            <Text size="xl" weight="semibold">Mock desk</Text>
            <Text size="md" color="text2">
              This build talks to a pretend Ghost on the phone itself. Nothing reaches your workstation. These stand in for your voice and the network.
            </Text>
            <View style={{ gap: space[2], marginTop: space[2] }}>
              {ACTS.map(({ act, label, when }) => (
                <TouchButton
                  key={act}
                  fullWidth
                  disabled={!enabled(act)}
                  accessibilityHint={when}
                  onPress={() => {
                    setOpen(false);
                    void desk.act(act);
                  }}
                >
                  {label}
                </TouchButton>
              ))}
            </View>
          </Pressable>
        </Pressable>
      </Modal>
    </>
  );
}
