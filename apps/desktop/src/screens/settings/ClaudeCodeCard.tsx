/**
 * Owner: settings. The operator's own Claude Code as Ghost's model (ADR 0069), as the first
 * run's model step leads with it and Settings › Model shows it: what the program says of
 * itself, in Ghost's words (`claudeCode.ts`). Ghost runs the program; it never offers a sign-in
 * of its own, so a signed-out program gets the program's own command and Look again.
 */
import { RefreshCw, Terminal } from "lucide-react";

import { Badge, Button, Card, ListRow, Notice } from "../../ui";
import type { ClaudeCodeOffer } from "./claudeCode";

/** The daemon's sentence, its `command` in backticks shown as a command. */
function Said({ text }: { text: string }) {
  return (
    <>
      {text.split("`").map((part, i) =>
        i % 2 ? (
          <span key={i} className="t-mono">
            {part}
          </span>
        ) : (
          part
        ),
      )}
    </>
  );
}

/** Signed in: the one-click choice, with the way to the others beside it. */
export function ClaudeCodeLead({ offer, chosen, onElsewhere }: { offer: Extract<ClaudeCodeOffer, { kind: "use" }>; chosen: boolean; onElsewhere?: () => void }) {
  return (
    <Card list className="claude-code-lead is-selected">
      <ListRow
        leading={<Terminal aria-hidden />}
        title={offer.title}
        description={
          <>
            {offer.body}
            {offer.note && <span className="set-help is-readable"> {offer.note}</span>}
          </>
        }
        trailing={
          chosen ? (
            <Badge tone="ok" dot>
              Chosen
            </Badge>
          ) : undefined
        }
      />
      {onElsewhere && (
        <div className="claude-code-lead__else">
          <Button size="sm" variant="ghost" onClick={onElsewhere}>
            Use something else
          </Button>
        </div>
      )}
    </Card>
  );
}

/** Installed and not usable as it is: what to do, in the operator's own terminal. */
export function ClaudeCodeFix({ offer, look, looking }: { offer: Extract<ClaudeCodeOffer, { kind: "fix" }>; look: () => void; looking: boolean }) {
  return (
    <Notice
      tone="info"
      title={offer.title}
      action={
        <Button size="sm" variant="secondary" leadingIcon={<RefreshCw />} loading={looking} onClick={look}>
          Look again
        </Button>
      }
    >
      <Said text={offer.body} />
      {offer.note && <> {offer.note}</>}
    </Notice>
  );
}
