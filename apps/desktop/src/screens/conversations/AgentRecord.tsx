/**
 * Owner: conversations. The conversation under an agent's page, drawn as the conversation
 * screen's record (`session/Transcript.tsx`): the same rail, marks, work rows, gaps and session
 * ends, in the agent's own mark. What is already folded into its memory is said in one line above.
 */
import type { TurnView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useMemo } from "react";

import { EmptyState } from "../../ui";
import { useLines } from "../session/hooks";
import { useSessionActivity } from "../session/live";
import { Transcript } from "../session/Transcript";
import { agentRecord } from "./record";
import "../session/session.css";

export function AgentRecord({ turns, slug, name, limit, total }: { turns: TurnView[]; slug: string; name: string; limit: number; total: number }) {
  const { status, activity } = useGhost();
  const lines = useLines();
  const apps = status?.apps ?? [];
  const sessions = useMemo(() => [...new Set(turns.map((t) => t.session_id))], [turns]);
  const kept = useSessionActivity(sessions, activity);
  const appKey = apps.join(",");
  const items = useMemo(() => agentRecord(turns, slug, lines, kept, appKey ? appKey.split(",") : []), [turns, slug, lines, kept, appKey]);
  const nameOf = (s: string) => (s === slug ? name : lines.find((l) => l.slug === s)?.name ?? s);
  const folded = turns.filter((t) => t.compacted).length;

  if (!turns.length)
    return <EmptyState size="compact" title={`Nothing said with ${name} yet`} description={`Call it by name, or type below. What you say and what ${name} does shows here as it happens.`} />;
  return (
    <div className="conv-record">
      {(folded > 0 || turns.length >= limit) && (
        <p className="conv-record__note t-small t-3">
          {turns.length >= limit ? `The last ${limit} of ${total} turns. ` : ""}
          {folded > 0 ? `${folded} of these are folded into ${name}’s memory as a summary; it keeps the rest word for word.` : ""}
        </p>
      )}
      <div className="session-record">
        <Transcript items={items} nameOf={nameOf} thinkingAs={null} lines={lines} />
      </div>
    </div>
  );
}
