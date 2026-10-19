import { Inter_400Regular, Inter_500Medium, Inter_600SemiBold, useFonts } from "@expo-google-fonts/inter";
import { JetBrainsMono_400Regular } from "@expo-google-fonts/jetbrains-mono";
import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import * as SystemUI from "expo-system-ui";
import { useEffect, useMemo } from "react";
import { View } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { DeskProvider } from "../src/desk/context";
import { LiveDesk } from "../src/desk/live";
import { MockDesk } from "../src/desk/mock";
import { MockPanel } from "../src/desk/MockPanel";
import { loadPairing, MOCK_DESK, pairingSecret } from "../src/phone/pairing";
import { usePhone } from "../src/phone/store";
import { useColors } from "../src/theme";

export default function RootLayout() {
  const c = useColors();
  const [fontsLoaded] = useFonts({ Inter_400Regular, Inter_500Medium, Inter_600SemiBold, JetBrainsMono_400Regular });
  const { hydrated, pairing } = usePhone();
  const secret = pairingSecret();

  // The paired desk over the LAN, or the mock one on the web and in mock builds. A new pairing
  // gets a new desk.
  const desk = useMemo(
    () => (!MOCK_DESK && pairing && secret ? new LiveDesk({ address: pairing.address, deviceId: pairing.deviceId, secret }) : new MockDesk()),
    [pairing?.deviceId, pairing?.address, secret],
  );
  useEffect(() => () => (desk instanceof LiveDesk ? desk.dispose() : undefined), [desk]);

  useEffect(() => {
    void loadPairing();
  }, []);

  useEffect(() => {
    void SystemUI.setBackgroundColorAsync(c.bgCanvas);
  }, [c.bgCanvas]);

  if (!fontsLoaded || !hydrated) return <View style={{ flex: 1, backgroundColor: c.bgCanvas }} />;

  return (
    <SafeAreaProvider>
      <DeskProvider desk={desk}>
        <StatusBar style="auto" />
        <Stack screenOptions={{ headerShown: false, contentStyle: { backgroundColor: c.bgCanvas }, animation: "fade" }} />
        {desk instanceof MockDesk && <MockPanel desk={desk} />}
      </DeskProvider>
    </SafeAreaProvider>
  );
}
