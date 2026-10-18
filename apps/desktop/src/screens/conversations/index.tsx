/**
 * Owner: conversations. Saved conversations, "lines" (ADR 0014).
 *   #/conversations          every line, with live state, Switch and New line
 *   #/conversations/:slug    one line: voice and persona, memory and notes, transcript, history
 * Views and dialogs that need a URL ride in the query string (see url.ts):
 *   ?tab=memory|transcript|history   ?open=new|voice|reset|delete   ?dirty=1 (mock only)
 */
import type { RouteProps } from "../../app/routes";
import { LineDetail } from "./LineDetail";
import { LinesIndex } from "./LinesIndex";
import "./conversations.css";

export default function ConversationsScreen({ params }: RouteProps) {
  return params.slug ? <LineDetail key={params.slug} slug={params.slug} /> : <LinesIndex />;
}
