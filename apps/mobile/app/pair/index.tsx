/**
 * Pairing, before anything else works: scan the QR code your desk shows, or type its address
 * and the six-digit code. The code is a `ghost://pair` link (docs/phone-protocol.md) carrying
 * the code and every address the desk listens on, so a scanned pairing knows the tunnel too.
 *
 * The camera is the only sensor this screen uses, and only while the screen is in front; the
 * mic stays off. The app asks for the camera the first time this screen opens. The phone's own
 * camera app can open the same link: then this screen shows where the link points and pairs
 * only after a tap, since a link can come from anywhere. The desk pairs with one phone, and
 * the screen says so. On the mock desk (web, mock builds) the viewfinder is a tap to pair.
 */
import { CameraView, useCameraPermissions } from "expo-camera";
import * as Linking from "expo-linking";
import { useFocusEffect, useRouter } from "expo-router";
import { useCallback, useRef, useState } from "react";
import { Pressable, ScrollView, View } from "react-native";

import { parsePairLink } from "@ghost/client-ts/pairLink";

import { MOCK_DESK, pairByLink, pairWithMock } from "../../src/phone/pairing";
import { usePhone } from "../../src/phone/store";
import { radius, space } from "../../src/theme";
import { GhostMark } from "../../src/ui/Brand";
import { TouchButton } from "../../src/ui/Button";
import { Badge, Cta, Notice, Screen, TopBar } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";

/** The camera feed and its captions are dark in both themes. */
const FEED_BG = "#0c0c0e";
const FEED_TEXT = "#ededef";

/** Links already acted on this run, so coming back to this screen doesn't offer one again. */
const handledLinks = new Set<string>();

export default function PairScan() {
  const router = useRouter();
  const { pairing } = usePhone();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ title: string; message: string } | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [focused, setFocused] = useState(false);
  const opened = Linking.useURL();
  const [, bump] = useState(0);
  const offered = opened && !handledLinks.has(opened) ? parsePairLink(opened) : null;

  useFocusEffect(
    useCallback(() => {
      setFocused(true);
      return () => setFocused(false);
    }, []),
  );

  const pair = async (text: string) => {
    setBusy(true);
    setError(null);
    setHint(null);
    const r = await pairByLink(text);
    setBusy(false);
    if (!r) return false;
    if (r.ok) {
      router.replace("/pair/done");
      return true;
    }
    setError({ title: r.field === "code" ? "That code didn’t work" : "Couldn’t reach your desk", message: r.message });
    return false;
  };

  const intro = MOCK_DESK || !offered
    ? "On your workstation, open Ghost, go to Settings, then Phone, and choose Pair a phone. Point the camera at the code it shows."
    : "This link pairs the phone with the desk at the address below. Pair only with your own desk.";

  return (
    <Screen>
      {pairing ? <TopBar /> : null}
      <ScrollView contentContainerStyle={{ paddingHorizontal: space[5], paddingBottom: space[6] }}>
        <View style={{ alignItems: "center", paddingTop: pairing ? space[2] : space[10], gap: space[3] }}>
          <GhostMark size={40} />
          <Text size="3xl" weight="semibold" align="center" accessibilityRole="header">Pair with your desk</Text>
          <Text size="lg" color="text2" align="center" style={{ maxWidth: 320 }}>{intro}</Text>
        </View>

        {error ? (
          <View style={{ marginTop: space[6] }}>
            <Notice tone="danger" title={error.title}>{error.message}</Notice>
          </View>
        ) : null}

        {MOCK_DESK ? (
          <MockViewfinder onPaired={() => router.replace("/pair/done")} />
        ) : offered && opened ? (
          <View style={{ marginTop: space[8], gap: space[4] }}>
            <View style={{ gap: space[1] }}>
              <Text size="md" color="text2">Your desk</Text>
              {offered.addresses.map((a, i) => (
                <Text key={a + i} size="lg" mono>{a}</Text>
              ))}
            </View>
            <TouchButton variant="primary" size="xl" fullWidth loading={busy} onPress={() => void pair(opened).then(() => handledLinks.add(opened))}>
              Pair with this desk
            </TouchButton>
            <TouchButton
              variant="ghost"
              fullWidth
              disabled={busy}
              onPress={() => {
                handledLinks.add(opened);
                setError(null);
                bump((n) => n + 1);
              }}
            >
              Scan a code instead
            </TouchButton>
          </View>
        ) : (
          <Scanner active={focused && !busy} busy={busy} hint={hint} onHint={setHint} onLink={pair} onRetry={() => setError(null)} failed={!!error} />
        )}

        <Text size="md" color="text2" align="center" style={{ marginTop: space[5] }}>
          {pairing
            ? `Your desk pairs with one phone. Pairing again replaces this phone’s pairing with ${pairing.workstation || "your desk"}.`
            : "Your desk pairs with one phone, this one. If another phone was paired, this one replaces it."}
        </Text>
      </ScrollView>
      <Cta>
        <TouchButton variant="secondary" size="lg" fullWidth onPress={() => router.push("/pair/code")}>
          Enter the address instead
        </TouchButton>
      </Cta>
    </Screen>
  );
}

/**
 * The live viewfinder. Asks for the camera once; a scan that is a pairing link pairs, anything
 * else says so and keeps looking. A code that failed is not tried again until the operator
 * taps Scan again or points at a different one, so a spent code can't burn the desk's tries.
 */
function Scanner({ active, busy, hint, failed, onHint, onLink, onRetry }: { active: boolean; busy: boolean; hint: string | null; failed: boolean; onHint: (h: string | null) => void; onLink: (text: string) => Promise<boolean>; onRetry: () => void }) {
  const [permission, ask] = useCameraPermissions();
  const working = useRef(false);
  const lastFailed = useRef<string | null>(null);

  const scanned = ({ data }: { data: string }) => {
    if (working.current || busy || data === lastFailed.current) return;
    if (!parsePairLink(data)) {
      onHint("That isn’t a Ghost pairing code. Point at the code in Pair a phone on your desk.");
      return;
    }
    working.current = true;
    void onLink(data).then((ok) => {
      working.current = false;
      if (!ok) lastFailed.current = data;
    });
  };

  const granted = permission?.granted ?? false;
  const caption = busy ? "Pairing with your desk…" : hint ?? (granted ? "Point at the code on your desk" : null);

  return (
    <View style={{ marginTop: space[8], gap: space[3] }}>
      <View style={{ aspectRatio: 1, borderRadius: radius.xl, backgroundColor: FEED_BG, overflow: "hidden", alignItems: "center", justifyContent: "center" }}>
        {granted ? (
          <>
            <CameraView
              style={{ position: "absolute", top: 0, left: 0, right: 0, bottom: 0 }}
              facing="back"
              active={active}
              barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
              onBarcodeScanned={active ? scanned : undefined}
            />
            <Corners />
          </>
        ) : permission === null ? null : (
          <View style={{ alignItems: "center", gap: space[4], padding: space[6] }}>
            <Text size="lg" align="center" style={{ color: FEED_TEXT }}>
              {permission.canAskAgain ? "Ghost uses the camera only to read this code." : "The camera is off for Ghost. Turn it on in Android settings, or enter the address instead."}
            </Text>
            <TouchButton variant="primary" onPress={() => void (permission.canAskAgain ? ask() : Linking.openSettings())}>
              {permission.canAskAgain ? "Allow the camera" : "Open settings"}
            </TouchButton>
          </View>
        )}
        {caption ? (
          <View style={{ position: "absolute", left: space[5], right: space[5], bottom: space[5] }}>
            <Text size="lg" align="center" style={{ color: FEED_TEXT }} accessibilityLiveRegion="polite">{caption}</Text>
          </View>
        ) : null}
      </View>
      {failed && !busy ? (
        <TouchButton
          variant="secondary"
          fullWidth
          onPress={() => {
            lastFailed.current = null;
            onHint(null);
            onRetry();
          }}
        >
          Scan again
        </TouchButton>
      ) : null}
    </View>
  );
}

/** The mock desk: tap the viewfinder to pair. */
function MockViewfinder({ onPaired }: { onPaired: () => void }) {
  const [scanning, setScanning] = useState(false);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel="Camera. Mock: tap to pair with the mock desk"
      onPress={() => {
        setScanning(true);
        setTimeout(() => {
          pairWithMock();
          onPaired();
        }, 900);
      }}
      style={{ marginTop: space[8], aspectRatio: 1, borderRadius: radius.xl, backgroundColor: FEED_BG, alignItems: "center", justifyContent: "flex-end", padding: space[5], overflow: "hidden" }}
    >
      <Corners />
      <View style={{ flexDirection: "row", alignItems: "center", gap: space[2] }}>
        <Text size="lg" style={{ color: FEED_TEXT }}>{scanning ? "Pairing with workstation…" : "Tap to pair with the mock desk"}</Text>
        {!scanning && <Badge tone="accent">Mock</Badge>}
      </View>
    </Pressable>
  );
}

/** The viewfinder's four corner marks. */
function Corners() {
  const arm = 28;
  const w = 3;
  const color = "rgba(255, 255, 255, 0.85)";
  const at = (pos: object, edges: object) => <View style={[{ position: "absolute", width: arm, height: arm, borderColor: color }, pos, edges]} />;
  return (
    <View pointerEvents="none" style={{ position: "absolute", top: "18%", left: "18%", right: "18%", bottom: "18%" }}>
      {at({ top: 0, left: 0 }, { borderTopWidth: w, borderLeftWidth: w, borderTopLeftRadius: 8 })}
      {at({ top: 0, right: 0 }, { borderTopWidth: w, borderRightWidth: w, borderTopRightRadius: 8 })}
      {at({ bottom: 0, left: 0 }, { borderBottomWidth: w, borderLeftWidth: w, borderBottomLeftRadius: 8 })}
      {at({ bottom: 0, right: 0 }, { borderBottomWidth: w, borderRightWidth: w, borderBottomRightRadius: 8 })}
    </View>
  );
}
