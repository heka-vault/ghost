/**
 * Activity: every change Ghost made or asked to make, newest first, grouped by day. A row
 * opens in place to show how it was decided. Read-only on the phone.
 */
import type { AuditEntry } from "@ghost/client-ts";
import { autonomyLabel, formatTime, lineName, statusLabel } from "@ghost/ui";
import { useLocalSearchParams } from "expo-router";
import { Power, ScrollText } from "lucide-react-native";
import { type ComponentRef, useEffect, useRef, useState } from "react";
import { ScrollView, View } from "react-native";

import { useDesk } from "../../src/desk/context";
import { space, useColors } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { entryLine, EntrySupport, EntryTitle, LineMark } from "../../src/ui/entries";
import { Divider, EmptyState, LargeHead, List, Row, Screen, Skeleton } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";

const PAGE = 30;

export default function Activity() {
  const { desk, audit, auditTotal, link } = useDesk();
  const { open: openParam } = useLocalSearchParams<{ open?: string }>();
  const [limit, setLimit] = useState(PAGE);
  const [open, setOpen] = useState<string | null>(openParam ?? null);
  const away = link === "offline" || link === "reconnecting";
  const shown = audit?.slice(0, limit) ?? null;
  const groups = groupByDay(shown ?? []);

  // Opened from a row elsewhere: open that entry and scroll it into view.
  const scroller = useRef<ComponentRef<typeof ScrollView>>(null);
  const offsets = useRef<Record<string, number>>({});
  useEffect(() => {
    if (openParam) setOpen(openParam);
  }, [openParam]);
  const scrollTo = (entryId: string, y: number) => {
    offsets.current[entryId] = y;
    if (entryId === openParam) scroller.current?.scrollTo({ y: Math.max(0, y - 48), animated: false });
  };

  return (
    <Screen bottom={false}>
      <ScrollView ref={scroller} contentContainerStyle={{ paddingHorizontal: space[5], paddingBottom: space[6] }}>
        <LargeHead title="Activity" sub="Everything Ghost changed, or asked to." />
        {away && shown && shown.length > 0 && <Text size="md" color="text2" style={{ marginBottom: space[3] }}>Showing what this phone saw last.</Text>}

        {shown == null && link === "connecting" ? (
          <List>
            {[0, 1, 2, 3, 4].map((i) => (
              <View key={i}>
                {i > 0 && <Divider />}
                <View style={{ padding: space[4], gap: space[2] }}>
                  <Skeleton width={`${70 + ((i * 13) % 25)}%`} height={16} />
                  <Skeleton width="40%" height={12} />
                </View>
              </View>
            ))}
          </List>
        ) : shown == null ? (
          <EmptyState icon={(c) => <Power size={24} color={c} />} title="Can’t reach your desk" description="Activity loads from Ghost on your workstation. The phone keeps trying." />
        ) : shown.length === 0 ? (
          <EmptyState icon={(c) => <ScrollText size={24} color={c} />} title="Nothing yet" description="When Ghost changes something, or asks to, it shows up here." />
        ) : (
          <>
            {groups.map(([day, entries]) => (
              <View key={day} style={{ marginBottom: space[4] }}>
                <Text size="md" weight="semibold" color="text2" accessibilityRole="header" style={{ marginBottom: space[2] }}>
                  {day}
                </Text>
                <List>
                  {entries.map((e, i) => (
                    <View key={e.id} onLayout={(ev) => scrollTo(e.id, ev.nativeEvent.layout.y)}>
                      {i > 0 && <Divider />}
                      <ActivityRow e={e} open={open === e.id} onToggle={() => setOpen(open === e.id ? null : e.id)} />
                    </View>
                  ))}
                </List>
              </View>
            ))}
            {shown.length < auditTotal && (
              <TouchButton
                variant="ghost"
                fullWidth
                onPress={() => {
                  setLimit((l) => l + PAGE);
                  void desk.loadAudit(limit + PAGE);
                }}
              >
                Show older
              </TouchButton>
            )}
          </>
        )}
      </ScrollView>
    </Screen>
  );
}

function ActivityRow({ e, open, onToggle }: { e: AuditEntry; open: boolean; onToggle: () => void }) {
  const c = useColors();
  const how = decidedBy(e);
  return (
    <View>
      <Row top leading={<LineMark slug={entryLine(e)} />} expanded={open} onPress={onToggle} title={<EntryTitle e={e} lines={open ? undefined : 2} />} support={<EntrySupport e={e} />} />
      {open && (
        <View style={{ paddingHorizontal: space[4], paddingBottom: space[4], gap: space[2], borderTopWidth: 1, borderTopColor: c.borderSubtle, paddingTop: space[3] }}>
          <Detail term="Status" value={statusLabel(e.status)} />
          <Detail term="Your rule" value={autonomyLabel(e.autonomy)} />
          {how && <Detail term="Decided by" value={how} />}
          {e.department ? <Detail term="Department" value={lineName(e.department)} /> : null}
          <Detail term="Line" value={lineName(entryLine(e))} />
          {e.toolkit ? <Detail term="Toolkit" value={e.toolkit} mono /> : null}
          <Detail term="When" value={formatTime(e.created_at)} />
          <Detail term="Operation" value={`${e.ability}.${e.operation}`} mono />
        </View>
      )}
    </View>
  );
}

function Detail({ term, value, mono }: { term: string; value: string; mono?: boolean }) {
  return (
    <View style={{ flexDirection: "row", gap: space[3] }}>
      <Text size="md" color="text3" style={{ width: 96 }}>{term}</Text>
      <Text size="md" mono={mono} style={{ flex: 1 }}>{value}</Text>
    </View>
  );
}

/** How a confirm-level change was settled, from its user_turn event, in plain words. */
function decidedBy(e: AuditEntry): string | null {
  const turn = e.events.find((x) => x.event === "user_turn");
  const d = (turn?.detail ?? {}) as { verdict?: string; classifier?: string; text?: string };
  if (e.status === "expired") return "Expired · Ghost left it alone";
  if (!turn) return null;
  if (d.verdict === "negative") return `You said no${d.text ? ` (“${d.text}”)` : ""}`;
  if (d.classifier === "ui") return "The Confirm button at your desk";
  return d.text ? `You said “${d.text}”` : "Your voice";
}

function groupByDay(items: AuditEntry[]): [string, AuditEntry[]][] {
  const out = new Map<string, AuditEntry[]>();
  const today = new Date();
  const key = (iso: string) => {
    const d = new Date(iso);
    const days = Math.round((Date.UTC(today.getFullYear(), today.getMonth(), today.getDate()) - Date.UTC(d.getFullYear(), d.getMonth(), d.getDate())) / 86400000);
    if (days === 0) return "Today";
    if (days === 1) return "Yesterday";
    return d.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
  };
  for (const e of items) {
    const k = key(e.created_at);
    out.set(k, [...(out.get(k) ?? []), e]);
  }
  return [...out.entries()];
}
