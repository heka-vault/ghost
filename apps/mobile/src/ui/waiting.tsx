/**
 * What needs you, on Home (issue #21, ADR 0051): each item and question with who it is from
 * and when, and what the operator can do about it from here. A question's words are the
 * asker's, shown as text; a change waiting for a yes gets Yes and No, which the gate takes as
 * the paired phone's own confirm. Every button waits for the desk: a row leaves the list when
 * the stream says it was answered, never on the phone's guess.
 */
import { errorText, relativeTime } from "@ghost/ui";
import { useState } from "react";
import { TextInput, View } from "react-native";

import type { Desk } from "../desk/types";
import type { WaitingRow } from "../desk/waiting";
import { fonts, radius, space, useColors } from "../theme";
import { TouchButton } from "./Button";
import { Divider, List } from "./parts";
import { Text } from "./Text";

const LATER_MINUTES = 120;

export function WaitingList({ rows, desk, stale, onCall, canCall }: { rows: WaitingRow[]; desk: Desk; stale: string | null; onCall: () => void; canCall: boolean }) {
  return (
    <View>
      <Text size="md" weight="semibold" color="text2" accessibilityRole="header" style={{ marginBottom: space[1], marginLeft: space[1] }}>
        What needs you
      </Text>
      {stale ? (
        <Text size="md" color="text3" accessibilityLiveRegion="polite" style={{ marginBottom: space[2], marginLeft: space[1] }}>
          {stale}
        </Text>
      ) : (
        <View style={{ height: space[2] }} />
      )}
      <List>
        {rows.map((row, i) => (
          <View key={row.key}>
            {i > 0 && <Divider />}
            <WaitingRowView row={row} desk={desk} onCall={onCall} canCall={canCall} />
          </View>
        ))}
      </List>
    </View>
  );
}

function WaitingRowView({ row, desk, onCall, canCall }: { row: WaitingRow; desk: Desk; onCall: () => void; canCall: boolean }) {
  const c = useColors();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [typing, setTyping] = useState(false);
  const [text, setText] = useState("");
  const waiting = row.sending || busy !== null;

  const run = async (what: string, fn: () => Promise<void>) => {
    setBusy(what);
    setError(null);
    try {
      await fn();
      if (what === "typed") {
        setTyping(false);
        setText("");
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const who = row.kind === "question" ? `${row.from} asks` : row.kind === "change" ? `${row.from} wants your yes` : row.from;
  const when = relativeTime(row.at);
  const shownError = error ?? row.error;

  return (
    <View style={{ paddingVertical: space[3], paddingHorizontal: space[4], gap: space[2] }}>
      <View style={{ gap: space[0.5] }}>
        <Text size="md" color="text3" numberOfLines={1}>
          {who}
          {when ? ` · ${when}` : ""}
          {row.acknowledged ? " · heard" : ""}
        </Text>
        <Text size="lg" numberOfLines={row.kind === "item" ? 2 : 6}>
          {row.text}
        </Text>
        {row.detail ? (
          <Text size="md" color="text2" numberOfLines={4}>
            {row.detail}
          </Text>
        ) : null}
      </View>

      {row.kind === "change" ? (
        row.changeId ? (
          <View style={{ flexDirection: "row", gap: space[2] }}>
            <TouchButton variant="primary" loading={busy === "yes"} disabled={waiting} onPress={() => void run("yes", () => desk.confirmChange(row.key, row.changeId!))}>
              Yes
            </TouchButton>
            <TouchButton loading={busy === "no"} disabled={waiting} onPress={() => void run("no", () => desk.cancelChange(row.key, row.changeId!))}>
              No
            </TouchButton>
          </View>
        ) : (
          <TouchButton disabled={!canCall} onPress={onCall} accessibilityHint="Opens the mic; say yes or no to Ghost">
            Call Ghost to answer
          </TouchButton>
        )
      ) : row.kind === "question" ? (
        <>
          {row.choices.length > 0 ? (
            <View style={{ flexDirection: "row", flexWrap: "wrap", gap: space[2] }}>
              {row.choices.map((choice) => (
                <TouchButton key={choice} loading={busy === `c:${choice}`} disabled={waiting} onPress={() => void run(`c:${choice}`, () => desk.answer(row.key, choice))}>
                  {choice}
                </TouchButton>
              ))}
            </View>
          ) : null}
          {!row.choicesOnly && typing ? (
            <View style={{ gap: space[2] }}>
              <TextInput
                value={text}
                onChangeText={setText}
                placeholder="Your answer"
                placeholderTextColor={c.text3}
                multiline
                maxLength={2000}
                autoFocus
                accessibilityLabel={`Your answer to ${row.from}`}
                style={{ minHeight: space[12], paddingHorizontal: space[4], paddingVertical: space[3], borderRadius: radius.md, borderWidth: 1, borderColor: c.borderInput, backgroundColor: c.bgInput, color: c.text1, fontFamily: fonts.regular, fontSize: 16 }}
              />
              <View style={{ flexDirection: "row", gap: space[2] }}>
                <TouchButton variant="primary" loading={busy === "typed"} disabled={waiting || !text.trim()} onPress={() => void run("typed", () => desk.answer(row.key, text.trim()))}>
                  Send
                </TouchButton>
                <TouchButton variant="ghost" disabled={waiting} onPress={() => setTyping(false)}>
                  Cancel
                </TouchButton>
              </View>
            </View>
          ) : (
            <View style={{ flexDirection: "row", flexWrap: "wrap", gap: space[2] }}>
              {!row.choicesOnly ? (
                <TouchButton variant="ghost" disabled={waiting} onPress={() => setTyping(true)}>
                  Answer in your words
                </TouchButton>
              ) : null}
              <TouchButton variant="ghost" loading={busy === "later"} disabled={waiting} onPress={() => void run("later", () => desk.snooze(row.key, LATER_MINUTES))}>
                Later
              </TouchButton>
              <TouchButton variant="ghost" loading={busy === "decline"} disabled={waiting} onPress={() => void run("decline", () => desk.decline(row.key))}>
                Won’t answer
              </TouchButton>
            </View>
          )}
        </>
      ) : (
        <View style={{ flexDirection: "row", gap: space[2] }}>
          {!row.acknowledged ? (
            <TouchButton loading={busy === "ack"} disabled={waiting} onPress={() => void run("ack", () => desk.acknowledge(row.key))}>
              Got it
            </TouchButton>
          ) : null}
          <TouchButton variant="ghost" loading={busy === "later"} disabled={waiting} onPress={() => void run("later", () => desk.snooze(row.key, LATER_MINUTES))}>
            Later
          </TouchButton>
        </View>
      )}

      {row.sending && busy === null ? (
        <Text size="md" color="text3" accessibilityLiveRegion="polite">
          Sending to your desk…
        </Text>
      ) : null}
      {shownError ? (
        <Text size="md" color="danger" accessibilityRole="alert">
          {shownError}
        </Text>
      ) : null}
    </View>
  );
}
