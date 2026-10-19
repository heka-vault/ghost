/** How the phone shows an audit entry: what happened in plain words, the issue as mono detail. */
import type { AuditEntry } from "@ghost/client-ts";
import { didNotHappen, relativeTime, splitSummary, statusLabel, statusTone } from "@ghost/ui";
import { View } from "react-native";

import { space, useColors } from "../theme";
import { Badge, Row } from "./parts";
import { Text } from "./Text";

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

/** The support line: the status badge (when not done), then issue or line · when. */
export function EntrySupport({ e }: { e: AuditEntry }) {
  const { target, conversation } = splitSummary(e);
  return (
    <View style={{ flexDirection: "row", alignItems: "center", flexWrap: "wrap", gap: space[1.5] }}>
      {e.status !== "executed" && <Badge tone={statusTone(e.status)}>{statusLabel(e.status)}</Badge>}
      <TargetText target={target} conversation={conversation} />
      <Text size="md" color="text2">·</Text>
      <Text size="md" color="text2">{relativeTime(e.created_at)}</Text>
    </View>
  );
}

export function EntryRow({ e, onPress }: { e: AuditEntry; onPress?: () => void }) {
  return <Row top onPress={onPress} title={<EntryTitle e={e} lines={2} />} support={<EntrySupport e={e} />} />;
}
