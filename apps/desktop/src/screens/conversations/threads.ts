/**
 * Owner: conversations. The agent fields the generated client carries (ADR 0014, ADR 0035)
 * and the helpers every screen reads them through: whether an agent is retired, and since
 * when.
 */
import type { ConversationCreate, ConversationDetail, ConversationUpdate, ConversationView, GhostClient } from "@ghost/client-ts";

export type AgentView = ConversationView;
export type AgentDetail = ConversationDetail;
export type AgentCreate = ConversationCreate;
export type AgentUpdate = ConversationUpdate;

/** When the agent was retired (left the roster and voice routing), or null. */
export function retiredAt(d: Pick<AgentView, "retired_at"> | null | undefined): string | null {
  const v = d?.retired_at;
  return typeof v === "string" && v ? v : null;
}

export const isRetired = (d: Pick<AgentView, "retired_at"> | null | undefined) => retiredAt(d) !== null;

/** Retire an agent: it leaves voice routing and the roster; everything stays readable. */
export const retireAgent = (client: GhostClient, slug: string) => client.retireConversation(slug);
/** Bring a retired agent back. */
export const rehireAgent = (client: GhostClient, slug: string) => client.rehireConversation(slug);
