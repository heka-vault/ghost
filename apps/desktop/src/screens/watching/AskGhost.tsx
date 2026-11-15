/**
 * Owner: watching. Making something Ghost does on its own is asking Ghost (the owner's
 * simplification, 2026-11-14): the operator's sentence goes to the session as an ordinary typed
 * turn (`client.say`, as Add watch already did), and the agent sets it up with the tools it
 * already has (`ghost_schedule_manage`, `ghost_watch_manage`, ADR 0047, ADR 0052) through the
 * gate, which asks where the operator's permissions or ADR 0052's defaults say so. Nothing here
 * writes a schedule or a watch, chooses one, or reads the words: the window sends them and shows
 * what came back. A change waiting for a yes is the one `PendingChange` card, whose Yes calls
 * `client.confirm()` and nothing else (ADR 0013). Ghost's reply is shown in place, and the list
 * is read again each time one arrives.
 */
import type { Turn } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ArrowUp } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Link } from "../../app/router";
import { Button, Card, Input, PendingChange, Spinner, alwaysDone, errorText, useLive, useToast } from "../../ui";

/** A few sentences to start from, one of each kind: they fill the field, nothing more. */
export const EXAMPLES = [
  "Every weekday at 9, check the build on main and tell me if it’s red",
  "Tell me when a new issue is opened in the app repo",
  "Every Friday at 4, have Accounting list the invoices still unpaid",
];

/**
 * Ghost's replies to an asked sentence: every assistant turn after the operator's own turn with
 * those words, in order. Pure, so it is tested.
 */
export function repliesTo(turns: readonly Pick<Turn, "role" | "text">[], asked: string): string[] {
  const want = asked.trim();
  let at = -1;
  for (let i = turns.length - 1; i >= 0; i--) {
    if (turns[i].role === "user" && turns[i].text.trim() === want) {
      at = i;
      break;
    }
  }
  if (at < 0) return [];
  const out: string[] = [];
  for (const t of turns.slice(at + 1)) {
    if (t.role === "user") break;
    if (t.role === "assistant" && t.text.trim()) out.push(t.text);
  }
  return out;
}

export function AskGhost({ onReply }: { onReply: () => void }) {
  const { client, turns, pending } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [text, setText] = useState("");
  const [asked, setAsked] = useState<string | null>(null);
  /** What Ghost said back, kept after the typed session ends and its turns go. */
  const [replies, setReplies] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!asked) return;
    const now = repliesTo(turns, asked);
    if (now.length > replies.length) {
      setReplies(now);
      onReply();
    }
  }, [turns, asked, replies.length, onReply]);

  const send = async () => {
    const words = text.trim();
    if (!words || busy || !online) return;
    setBusy(true);
    setError(null);
    try {
      await client.say(words);
      setAsked(words);
      setReplies([]);
      setText("");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const again = () => {
    setAsked(null);
    setReplies([]);
    requestAnimationFrame(() => field.current?.focus());
  };

  return (
    <Card padding="md" className="wat-ask">
      <form
        className="wat-ask__form"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <label htmlFor="wat-ask-field" className="sr-only">
          Ask Ghost to do something on its own
        </label>
        <div className="wat-ask__row">
          <Input
            ref={field}
            id="wat-ask-field"
            maxLength={2000}
            value={text}
            aria-describedby="wat-ask-help"
            placeholder="What, and when: every weekday at 9, check unpaid invoices"
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void send();
              }
            }}
          />
          <Button variant="primary" type="submit" leadingIcon={<ArrowUp />} loading={busy} disabled={!text.trim() || !online}>
            Ask Ghost
          </Button>
        </div>
        {!asked && (
          <div id="wat-ask-help" className="wat-ask__examples" role="group" aria-label="Examples">
            {EXAMPLES.map((x) => (
              <button key={x} type="button" className="wat-ask__example t-small" onClick={() => (setText(x), field.current?.focus())}>
                {x}
              </button>
            ))}
          </div>
        )}
        {error && <p className="t-small t-danger">Ghost didn’t take it: {error}</p>}
      </form>

      {asked && (
        <div className="wat-ask__said" aria-live="polite">
          <p className="wat-ask__you">
            <span className="t-small t-3">You asked</span>
            <span data-user-content>{asked}</span>
          </p>
          {replies.map((r, i) => (
            <p key={i} className="wat-ask__ghost">
              <span className="t-small t-3">Ghost</span>
              <span data-user-content>{r}</span>
            </p>
          ))}
          {pending ? (
            <PendingChange
              summary={pending.summary}
              rationale={pending.rationale}
              operation={`${pending.source}.${pending.operation}`}
              expiresAt={pending.expires_at}
              disabled={!online}
              onConfirm={() => void client.confirm()}
              onCancel={() => void client.cancel()}
              target={pending.target}
              // ADR 0070: the same decision made to stick, on the card; the click is the
              // operator's own and calls `decideAlways` and nothing else.
              always={pending.always}
              onAlways={(choice) => {
                client.decideAlways(pending.id, choice).then(
                  () => toast(alwaysDone(choice, pending.always)),
                  (e) => setError(errorText(e)),
                );
              }}
            />
          ) : replies.length === 0 ? (
            <p className="wat-ask__wait t-small t-2">
              <Spinner size={14} /> Ghost is setting it up…
            </p>
          ) : null}
          <div className="wat-ask__after">
            <Button variant="ghost" size="sm" onClick={again}>
              Ask something else
            </Button>
            <Link to="/session" className="t-small">
              Open the conversation
            </Link>
          </div>
        </div>
      )}
    </Card>
  );
}
