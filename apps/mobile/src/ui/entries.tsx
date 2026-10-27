/**
 * How the phone shows an audit entry: what happened in plain words, the issue as mono
 * detail, and the line it happened in (ADR 0014) as a ghost in that line's colour, so a
 * glance at the colour says which conversation did it.
 */
import type { AuditEntry } from "@ghost/client-ts";
import { didNotHappen, lineHue, lineName, relativeTime, splitSummary, statusLabel, statusTone } from "@ghost/ui";
import { View } from "react-native";

import { space, useColors } from "../theme";
import { GhostGlyph } from "./Brand";
import { Badge, Row } from "./parts";
import { Text } from "./Text";

/** The line an entry was made in: Ghost when it was recorded before lines were kept on entries. */
export function entryLine(e: AuditEntry): string {
  return e.conversation ?? "ghost";
}

/** The ghost in the line's colour, leading a row. */
export function LineMark({ slug, size = 40 }: { slug: string; size?: number }) {
  const c = useColors();
  return <GhostGlyph size={size} color={c.hue[lineHue(slug)].fg} />;
}

/** The target as a row shows it: an issue ref in mono, a line's name in regular text. */
export function TargetText({ target, conversation, size = "md", color = "text2" }: { target: string; conversation: boolean; size?: "md" | "lg" | "xl"; color?: "text1" | "text2" }) {
  return (
    <Text size={size} color={color} mono={!conversation} numberOfLines={1}>
      {target}
    </Text>
  );
}

/** The sentence: the gate's own summary. */
export function EntryTitle({ e, lines }: { e: AuditEntry; lines?: number }) {
  return (
    <Text size="lg" color={didNotHappen(e) ? "text2" : "text1"} numberOfLines={lines}>
      {splitSummary(e).text}
    </Text>
  );
}

/** Where an entry was made, in words: the agent that made it. */
export function whereMade(e: AuditEntry): string {
  return lineName(entryLine(e));
}

/** The support line: the status badge (when not done), the agent, then issue · when. */
export function EntrySupport({ e }: { e: AuditEntry }) {
  const { target, conversation } = splitSummary(e);
  return (
    <View style={{ flexDirection: "row", alignItems: "center", flexWrap: "wrap", gap: space[1.5] }}>
      {e.status !== "executed" && <Badge tone={statusTone(e.status)}>{statusLabel(e.status)}</Badge>}
      <Text size="md" color="text2">{whereMade(e)}</Text>
      <Text size="md" color="text2">·</Text>
      <TargetText target={target} conversation={conversation} />
      <Text size="md" color="text2">·</Text>
      <Text size="md" color="text2">{relativeTime(e.created_at)}</Text>
    </View>
  );
}

export function EntryRow({ e, onPress }: { e: AuditEntry; onPress?: () => void }) {
  return <Row top leading={<LineMark slug={entryLine(e)} />} onPress={onPress} title={<EntryTitle e={e} lines={2} />} support={<EntrySupport e={e} />} />;
}
