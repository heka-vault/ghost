/** Pair by typing the workstation's address and the six-digit code it shows. */
import { useRouter } from "expo-router";
import { useState } from "react";
import { KeyboardAvoidingView, ScrollView, TextInput, View } from "react-native";

import { groupCode, MOCK_ADDRESS, pairByCode } from "../../src/phone/pairing";
import { fonts, radius, space, useColors } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { Cta, Notice, Screen, TopBar } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";

export default function PairCode() {
  const router = useRouter();
  const [address, setAddress] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [codeError, setCodeError] = useState<string | null>(null);
  const [netError, setNetError] = useState<string | null>(null);
  const ready = address.trim().length > 0 && code.replace(/\D/g, "").length === 6;

  const submit = async () => {
    if (!ready || busy) return;
    setBusy(true);
    setCodeError(null);
    setNetError(null);
    const r = await pairByCode(address.trim(), code);
    setBusy(false);
    if (r.ok) return router.replace("/pair/done");
    if (r.field === "code") setCodeError(r.message);
    else setNetError(r.message);
  };

  return (
    <Screen>
      <KeyboardAvoidingView behavior="height" style={{ flex: 1 }}>
        <TopBar title="Enter the address" />
        <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={{ paddingHorizontal: space[5], paddingBottom: space[6], gap: space[5] }}>
          <Text size="lg" color="text2">Both are under Settings, Pair a phone on your workstation. Works on the same Wi-Fi.</Text>
          {netError && <Notice tone="danger" title="Couldn’t reach your desk">{netError}</Notice>}
          <Field label="Workstation address">
            <Input value={address} onChangeText={setAddress} placeholder={MOCK_ADDRESS} keyboardType="url" autoCapitalize="none" autoCorrect={false} returnKeyType="next" />
          </Field>
          <Field label="Pairing code" help="Six digits. A new one appears every few minutes." error={codeError}>
            <Input
              value={code}
              onChangeText={(t) => {
                setCode(groupCode(t));
                if (codeError) setCodeError(null);
              }}
              placeholder="482 913"
              keyboardType="number-pad"
              autoComplete="one-time-code"
              invalid={!!codeError}
              returnKeyType="go"
              onSubmitEditing={submit}
            />
          </Field>
        </ScrollView>
        <Cta>
          <TouchButton variant="primary" size="xl" fullWidth loading={busy} disabled={!ready} onPress={submit}>
            Pair
          </TouchButton>
        </Cta>
      </KeyboardAvoidingView>
    </Screen>
  );
}

function Field({ label, help, error, children }: { label: string; help?: string; error?: string | null; children: React.ReactNode }) {
  return (
    <View style={{ gap: space[2] }}>
      <Text size="lg" weight="medium">{label}</Text>
      {children}
      {error ? (
        <Text size="md" color="danger" accessibilityRole="alert">{error}</Text>
      ) : help ? (
        <Text size="md" color="text2">{help}</Text>
      ) : null}
    </View>
  );
}

/** A 48dp field with 16px mono text. */
function Input({ invalid, ...rest }: React.ComponentProps<typeof TextInput> & { invalid?: boolean }) {
  const c = useColors();
  const [focused, setFocused] = useState(false);
  return (
    <TextInput
      {...rest}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      placeholderTextColor={c.text3}
      style={{
        height: space[12],
        paddingHorizontal: space[4],
        borderRadius: radius.md,
        borderWidth: focused ? 2 : 1,
        borderColor: invalid ? c.danger : focused ? c.focusRing : c.borderInput,
        backgroundColor: c.bgInput,
        color: c.text1,
        fontFamily: fonts.mono,
        fontSize: 16,
      }}
    />
  );
}
