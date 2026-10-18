/**
 * Owner: conversations. Every line at a glance (#/conversations): Ghost first, the live
 * one next, the rest by last use. Each row is a name, the line's persona (the one thing
 * that makes lines different) and when it was last used; a row opens the line, and while
 * a session is live, Switch shows on the row under the pointer or focus and patches the
 * session through from here.
 */
import type { ConversationView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { CloudOff, MessagesSquare, Plus } from "lucide-react";
import { useEffect, useState } from "react";

import { hrefFor, navigate } from "../../app/router";
import { useDesign } from "../../design/context";
import {
  Button,
  Card,
  EmptyState,
  LineGlyph,
  ListRow,
  LiveStatus,
  Notice,
  Page,
  Skeleton,
  cx,
  errorText,
  lineName,
  quoted,
  useLive,
  useToast,
} from "../../ui";
import { drafts } from "./draft";
import { LineStateText, lineState } from "./lineState";
import { NewLineDialog } from "./NewLineDialog";
import { personaCache } from "./personaCache";
import { designFail, designRetry, focusOnOpen, readParam, writeParam } from "./url";
import { parseVoice, voiceKind } from "./voices";

/** The first line of a persona, as the row's one-line description. */
export function personaLead(persona: string): string {
  return persona.split("\n").map((s) => s.trim()).find(Boolean) ?? "";
}

export function LinesIndex() {
  const { client, status, conversationsVersion, pending } = useGhost();
  const { online, connection, phase } = useLive();
  const toast = useToast();
  const design = useDesign();
  const [lines, setLines] = useState<ConversationView[] | null>(null);
  /**
   * Each line's persona, by slug. The list route carries no persona (ConversationView), so
   * it is read from each line's detail after the list lands; `null` when that read failed,
   * so the row falls back to the voice. Missing while loading, unless the cache has seen
   * the slug before: then the last known persona shows at once and the read refreshes it.
   */
  const [personas, setPersonas] = useState<Record<string, string | null>>(() => Object.fromEntries(personaCache));
  /** The list itself failed to load (or refresh). */
  const [loadError, setLoadError] = useState<string | null>(null);
  /** An action on one line failed: its own title, near what failed. */
  const [notice, setNotice] = useState<{ title: string; text: string } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [retrying, setRetrying] = useState(false);
  const [creating, setCreating] = useState(() => readParam("open") === "new");
  const [switching, setSwitching] = useState<string | null>(null);

  const inSession = !!status?.session_id;
  const liveSlug = inSession ? (status?.conversation ?? "ghost") : null;

  useEffect(() => {
    let on = true;
    const get = designFail(design, "lines")
      ? Promise.reject(new Error("Ghost couldn’t read the conversations database (database is locked)."))
      : client.conversations();
    get
      .then((l) => {
        if (!on) return;
        setLines(l);
        setLoadError(null);
        return Promise.all(
          l.map((c) =>
            client
              .conversation(c.slug)
              .then((d) => {
                personaCache.set(c.slug, d.persona);
                return [c.slug, d.persona] as const;
              })
              .catch(() => [c.slug, personaCache.get(c.slug) ?? null] as const),
          ),
        ).then((pairs) => on && setPersonas(Object.fromEntries(pairs)));
      })
      .catch((e) => on && setLoadError(errorText(e)))
      .finally(() => on && setRetrying(false));
    return () => {
      on = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, conversationsVersion, attempt]);

  const rank = (l: ConversationView) => (l.is_ghost ? 0 : l.slug === liveSlug ? 1 : 2);
  const sorted = [...(lines ?? [])].sort(
    (a, b) => rank(a) - rank(b) || (b.last_active_at ?? "").localeCompare(a.last_active_at ?? "") || a.name.localeCompare(b.name),
  );
  const others = sorted.filter((l) => !l.is_ghost);
  // With only Ghost, the empty state below the list carries the one call to action.
  const onlyGhost = lines != null && others.length === 0;

  const switchTo = async (l: ConversationView) => {
    const name = lineName(l.slug, l.name);
    setSwitching(l.slug);
    try {
      const r = await client.switchConversation(l.slug);
      if (r.ok === false) setNotice({ title: `Couldn’t switch to ${name}`, text: r.detail ?? "Ghost didn’t take the switch. Try again, or say it." });
      else toast({ title: `Patched through to ${name}` });
    } catch (e) {
      setNotice({ title: `Couldn’t switch to ${name}`, text: errorText(e) });
    } finally {
      setSwitching(null);
    }
  };

  return (
    <Page
      width="default"
      title="Conversations"
      status={<LiveStatus />}
      description={
        <>
          Lines you can be patched through to. Say {quoted("patch me to accounting")}, or {quoted("back to Ghost")}. Each keeps its own voice, persona and memory.
        </>
      }
      actions={
        onlyGhost ? undefined : (
          <Button variant="primary" leadingIcon={<Plus />} disabled={!online} onClick={() => setCreating(true)}>
            New line
          </Button>
        )
      }
    >
      {notice && (
        <Notice tone="danger" title={notice.title} onDismiss={() => setNotice(null)} className="conv-notice">
          {notice.text}
        </Notice>
      )}
      {loadError && lines != null && (
        <Notice tone="danger" title="Couldn’t refresh your lines" onDismiss={() => setLoadError(null)} className="conv-notice">
          {loadError} This is the list as it was.
        </Notice>
      )}

      {lines == null && loadError ? (
        <Card>
          <EmptyState
            icon={<CloudOff />}
            title="Couldn’t load your lines"
            description={
              <>
                Ghost didn’t send them. Try again in a moment.
                <span className="conv-error-detail t-small t-3">{loadError}</span>
              </>
            }
            actions={
              <Button
                loading={retrying}
                onClick={() => {
                  setRetrying(true);
                  designRetry("lines");
                  setAttempt((n) => n + 1);
                }}
              >
                Try again
              </Button>
            }
          />
        </Card>
      ) : lines == null ? (
        <Card list aria-busy="true" aria-label="Loading lines">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="conv-skel-row">
              <Skeleton width={24} height={24} radius="var(--radius-sm)" />
              <span className="stack gap-2 grow">
                <Skeleton width={i ? 120 : 64} height={12} />
                <Skeleton width={i % 2 ? 280 : 220} height={10} />
              </span>
            </div>
          ))}
        </Card>
      ) : (
        <>
          <Card list>
            {sorted.map((l) => {
              const name = lineName(l.slug, l.name);
              const live = l.slug === liveSlug;
              const unsaved = drafts.has(l.slug);
              const v = parseVoice(l.voice);
              const state = lineState({ live, pending: !!pending, connection, phase, lastActive: l.last_active_at });
              const persona = personas[l.slug];
              const lead = typeof persona === "string" ? personaLead(persona) : "";
              // Aliases and the voice live on the detail page, so the persona reads whole on every row.
              return (
                <ListRow
                  key={l.slug}
                  href={hrefFor(`/conversations/${l.slug}`)}
                  leading={<LineGlyph slug={l.slug} name={name} size="lg" />}
                  title={name}
                  description={
                    persona === undefined ? (
                      <span className="conv-row-desc conv-row-desc--loading" aria-hidden>
                        <Skeleton width={l.is_ghost ? 260 : 320} height={10} />
                      </span>
                    ) : lead ? (
                      <span className="conv-row-desc" data-user-content>
                        {lead}
                      </span>
                    ) : persona === null ? (
                      <span className="conv-row-desc">{v.name}, {voiceKind(v)}</span>
                    ) : (
                      <span className="conv-row-desc t-3">No persona yet</span>
                    )
                  }
                  meta={unsaved ? <span className="conv-row-meta">Unsaved changes</span> : undefined}
                  trailing={
                    live ? (
                      <span className="conv-row-end">
                        <LineStateText state={state} connection={connection} phase={phase} />
                        {state.tone === "waiting" && (
                          <Button size="sm" variant="ghost" className="conv-row-action" href={hrefFor("/")} aria-label="Review the change waiting on your okay">
                            Review
                          </Button>
                        )}
                      </span>
                    ) : (
                      // One slot: when it was last used at rest; under the pointer or focus (and beside
                      // it on touch) the Switch, so every row ends on the card's content edge.
                      <span className={cx("conv-row-end", inSession && "conv-row-end--swap", switching === l.slug && "is-switching")}>
                        <span className="conv-row-end__when">{state.words}</span>
                        {inSession && (
                          <Button
                            size="sm"
                            variant="ghost"
                            className="conv-row-action conv-row-switch"
                            loading={switching === l.slug}
                            disabled={!online || (switching != null && switching !== l.slug)}
                            aria-label={`Switch to ${name}`}
                            onClick={() => void switchTo(l)}
                          >
                            Switch
                          </Button>
                        )}
                      </span>
                    )
                  }
                />
              );
            })}
          </Card>
          {onlyGhost && (
            <EmptyState
              icon={<MessagesSquare />}
              title="Start a line for each part of your work"
              description={<>Accounting, legal, a morning standup: each line keeps its own voice, persona and memory, and Ghost stays the main line. Start one here, then say {quoted("patch me to legal")} in any session.</>}
              actions={
                <Button variant="primary" leadingIcon={<Plus />} disabled={!online} onClick={() => setCreating(true)}>
                  New line
                </Button>
              }
            />
          )}
          {others.length > 0 && (
            <p className="conv-foot t-small t-3">
              {inSession
                ? `Ghost is on ${liveSlug === "ghost" ? "the main line" : `the ${lineName(liveSlug, sorted.find((l) => l.slug === liveSlug)?.name)} line`}. Switch patches the session through; Ghost says so out loud.`
                : "Switch works while a session is live. Wake Ghost, then switch here or say it."}
            </p>
          )}
        </>
      )}

      <NewLineDialog
        open={creating}
        onClose={() => {
          setCreating(false);
          writeParam("open", null);
        }}
        taken={(lines ?? []).flatMap((l) => [l.name.toLowerCase(), l.slug])}
        onCreated={(d) => {
          setCreating(false);
          writeParam("open", null);
          toast({ title: `Started ${d.name}`, description: "Give it a persona and a voice, then patch through to it." });
          focusOnOpen.slug = d.slug;
          navigate(`/conversations/${d.slug}`);
        }}
      />
    </Page>
  );
}
