/**
 * Owner: conversations. Agents (ADR 0014, ADR 0035): conversations with a persona and a job.
 *   #/agents          every agent, with live state, Switch and New agent; Ghost's rules
 *                     (Add rule, Open folder) and what Ghost made in out/
 *   #/agents/:slug    one agent: voice and persona, memory and notes, transcript, history
 * Views and dialogs that need a URL ride in the query string (see url.ts):
 *   ?tab=memory|transcript|history   ?open=new|retired|rule|voice|reset|delete|retire   ?seed=<slug> (New agent from a retired one)   ?dirty=1 (mock only)
 */
import type { RouteProps } from "../../app/routes";
import { AgentDetail } from "./AgentDetail";
import { AgentsIndex } from "./AgentsIndex";
import "./conversations.css";

export default function ConversationsScreen({ params }: RouteProps) {
  return params.slug ? <AgentDetail key={params.slug} slug={params.slug} /> : <AgentsIndex />;
}
