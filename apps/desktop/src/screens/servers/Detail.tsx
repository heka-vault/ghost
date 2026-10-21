/** Owner: servers. The selected server: whether Ghost reached it, its scope, and what it offers, declares and leaves out. */
import type { McpServerInfo, McpToolInfo, McpWriteInfo, SourceInfo } from "@ghost/client-ts";
import { autonomyLabel } from "@ghost/ui";
import { ArrowRight, Code, Hand, Megaphone, ShieldOff, VolumeX } from "lucide-react";
import type { ReactNode, Ref } from "react";

import { Badge, Button, Card, Notice, cx, quoted } from "../../ui";
import { argumentWords, count, humanize, kindWords, labelSaysId, notOffered, policyFileHref, policyHref, readsOffered, transportLabel, writeDescription } from "./model";

const LEVEL_ICON: Record<string, ReactNode> = { confirm: <Hand />, announce: <Megaphone />, quiet: <VolumeX />, deny: <ShieldOff /> };

export interface DetailProps {
  server: McpServerInfo;
  /** What the gate built from this server, when it is connected: the declaration's own words for each write. */
  source: SourceInfo | null;
  /** Placed just above the header, offset by the compact page bar; the screen scrolls to it on a new pick. */
  headerAnchor?: Ref<HTMLSpanElement>;
}

export function Detail({ server: s, source, headerAnchor }: DetailProps) {
  const name = humanize(s.name);
  const reads = readsOffered(s);
  const extra = notOffered(s);
  const scope = s.scope ?? null;
  const words = scope ? kindWords(scope.kind) : null;
  const many = scope ? scope.arguments.length > 1 : false;
  return (
    <article className="srv-detail" aria-labelledby="srv-detail-title">
      <header className="srv-detail__header">
        {/* Where a scroll lands so the header sits under the compact page bar, level with the list. */}
        <span ref={headerAnchor} className="srv-detail__anchor" aria-hidden />
        <div className="srv-detail__titles">
          <h2 id="srv-detail-title" className="t-title srv-detail__title">
            {name}
          </h2>
          <p className="srv-detail__meta">
            <span className="t-mono">{s.name}</span>
            <span aria-hidden>·</span>
            <span>{transportLabel(s.transport)}</span>
          </p>
        </div>
        <div className="srv-detail__control">{s.connected ? <Badge tone="ok" dot>Connected</Badge> : <Badge tone="danger" dot>Not connected</Badge>}</div>
      </header>

      {source && (
        <p className="srv-detail__description t-body-lg" data-user-content>
          {source.description}
        </p>
      )}

      {!s.connected && (
        <Notice
          tone="danger"
          title="Ghost couldn’t reach it"
          className="srv-detail__notice"
          action={
            <Button size="sm" variant="secondary" href={policyFileHref()} leadingIcon={<Code />}>
              Edit the policy file
            </Button>
          }
        >
          <p className="srv-notice__p" data-user-content>
            {s.error || "It didn’t answer."}
          </p>
          <p className="srv-notice__p">Nothing from it reaches the model until it connects. Check its declaration in the policy file.</p>
        </Notice>
      )}

      <div className="srv-sections">
        <Block
          title="Scope"
          description={
            scope && words
              ? `A line can be limited to ${words.many} on this server. The ${argumentWords(scope.arguments)} ${many ? "arguments name" : "argument names"} which one a call touches.`
              : "No scope declared, so a line can’t be limited on this server: every line sees everything it offers. Add a scope to its declaration to change that."
          }
        >
          {scope && words ? (
            scope.items.length > 0 ? (
              <Card list>
                {scope.items.map((item) => (
                  <div key={item} className="srv-item srv-item--compact">
                    <p className="srv-item__title t-mono">{item}</p>
                  </div>
                ))}
              </Card>
            ) : (
              <p className="srv-none">
                The declaration lists no {words.many} yet, so no line can be limited here. Add them under <span className="t-mono">scope</span> in the policy file.
              </p>
            )
          ) : null}
        </Block>

        <Block
          title="What Ghost can look up"
          count={reads.length}
          description={reads.length > 0 ? "Offered to the model as the server describes them. They never change anything." : undefined}
        >
          {reads.length > 0 ? (
            <Card list>
              {reads.map((t) => (
                <ToolRow key={t.name} tool={t} />
              ))}
            </Card>
          ) : (
            <p className="srv-none">
              {s.connected ? (
                <>
                  No reads offered. The server marks none of its tools read-only; list the ones to offer under <span className="t-mono">reads</span> in its
                  declaration.
                </>
              ) : (
                "Nothing while it isn’t connected."
              )}
            </p>
          )}
        </Block>

        <Block
          title="What Ghost can change"
          count={s.writes.length}
          description={s.writes.length > 0 ? "Each write is an operation the gate knows: it goes through at the level you set, and every one is logged. To reverse one, ask Ghost for the opposite change." : undefined}
          actions={
            s.writes.length > 0 ? (
              <Button size="sm" variant="ghost" href={policyHref(s.name)} trailingIcon={<ArrowRight />}>
                Change levels
              </Button>
            ) : undefined
          }
        >
          {s.writes.length > 0 ? (
            <Card list>
              {s.writes.map((w) => (
                <WriteRow key={w.operation} server={s} write={w} description={writeDescription(source, w)} />
              ))}
            </Card>
          ) : (
            <p className="srv-none">
              No writes declared, so Ghost can’t change anything through this server. Declare one under{" "}
              <span className="t-mono">[mcp.{s.name}.writes.&lt;operation&gt;]</span> in the policy file.
            </p>
          )}
        </Block>

        {extra.length > 0 && (
          <Block
            title="Not offered"
            count={extra.length}
            description={
              <>
                Tools the server has that no declaration covers. Ghost never calls them. To offer one as a write, declare it under{" "}
                <span className="t-mono">[mcp.{s.name}.writes.&lt;operation&gt;]</span>; to offer it as a read, list it under <span className="t-mono">reads</span>.
              </>
            }
          >
            <Card list>
              {extra.map((t) => (
                <ToolRow key={t.name} tool={t} quiet />
              ))}
            </Card>
          </Block>
        )}
      </div>
    </article>
  );
}

/**
 * One tool as the server describes it: the humanized name leads, the exact name shows only
 * when the label doesn't already say it, and the server's own words follow, wrapping to two
 * lines. Tool descriptions are untrusted text (ADR 0022): shown, never acted on.
 */
function ToolRow({ tool: t, quiet }: { tool: McpToolInfo; quiet?: boolean }) {
  return (
    <div className={cx("srv-item", quiet && "is-quiet")}>
      <div className="srv-item__text">
        <p className="srv-item__title">
          {humanize(t.name)}
          {!labelSaysId(t.name) && <span className="srv-item__id">{t.name}</span>}
        </p>
        {t.description && (
          <p className="srv-item__desc" data-user-content>
            {t.description}
          </p>
        )}
      </div>
    </div>
  );
}

/** One declared write: the operation the policy knows, what it calls, how it is asked for, and what the gate keeps for it. */
function WriteRow({ server, write: w, description }: { server: McpServerInfo; write: McpWriteInfo; description: string | null }) {
  const asked = w.asked_by.map((word) => quoted(word));
  const stale = w.read
    ? (
        <>
          Checks <span className="t-mono">{w.read}</span> first, so a change to something that moved since is refused.
        </>
      )
    : "No read declared, so no stale check. The gate still keeps the level, your yes and the audit.";
  // Why a declared write isn't offered: the policy says no, or the server has no such tool.
  const why = w.offered || !server.connected ? null : w.autonomy === "deny" ? "Not allowed, so not offered." : `Not offered: the server has no ${w.tool} with these arguments.`;
  return (
    <div className="srv-item srv-item--write">
      <div className="srv-item__text">
        <p className="srv-item__title">
          {humanize(w.operation)}
          <span className="srv-item__id">{w.operation}</span>
          {why && (
            <Badge size="sm" variant="outline">
              Not offered
            </Badge>
          )}
        </p>
        {description && (
          <p className="srv-item__desc" data-user-content>
            {description}
          </p>
        )}
        <ul className="srv-item__facts" aria-label={`How ${humanize(w.operation)} works`}>
          <li>
            Calls <span className="t-mono">{w.tool}</span> · {w.risk} risk
          </li>
          <li>{asked.length > 0 ? <>You ask with {argumentWords(asked).replace(/ and /, " or ")}</> : "No words declared: only when the model proposes it"}</li>
          <li>{stale}</li>
          {why && <li className="srv-item__why">{why}</li>}
        </ul>
      </div>
      <span className={`srv-level srv-level--${w.autonomy}`}>
        {LEVEL_ICON[w.autonomy]}
        {autonomyLabel(w.autonomy)}
      </span>
    </div>
  );
}

/**
 * A titled block. Like Section, but its action sits on the heading's line rather than the
 * description's last line (Section bottom-aligns actions).
 */
function Block({ title, count: n, description, actions, children }: { title: string; count?: number; description?: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="srv-block">
      <header className="srv-block__header">
        <div className="srv-block__head">
          <h3 className="srv-block__title">
            {title}
            {n !== undefined && (
              <span className="srv-block__count t-num" aria-label={count(n, "item")}>
                {n}
              </span>
            )}
          </h3>
          {actions && <div className="srv-block__actions">{actions}</div>}
        </div>
        {description && <p className="srv-block__description">{description}</p>}
      </header>
      {children}
    </section>
  );
}
