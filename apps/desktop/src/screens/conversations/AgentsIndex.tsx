/**
 * Owner: conversations. Every agent at a glance (#/agents), grouped by department: Ghost and
 * its agents, then each department with its own agent first and the agents hired into it.
 * Each row is a name, the job, the agent's persona and when it was last used; a row opens
 * the agent, and while a session is live, Switch shows on the row under the pointer or focus
 * and transfers the session from here. Retired agents wait in their own group below.
 */
import type { ConversationView, DepartmentView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Archive, CloudOff, MessagesSquare, Plus } from "lucide-react";
import { useEffect, useState } from "react";

import { Link, hrefFor, navigate } from "../../app/router";
import { useDesign } from "../../design/context";
import {
  Button,
  Card,
  Disclosure,
  EmptyState,
  LineGlyph,
  ListRow,
  LiveStatus,
  Notice,
  Page,
  Section,
  Skeleton,
  agentName,
  cx,
  errorText,
  quoted,
  relativeTime,
  useLive,
  useToast,
} from "../../ui";
import { drafts } from "./draft";
import { LineStateText, lineState } from "./lineState";
import { NewAgentDialog } from "./NewAgentDialog";
import { personaCache } from "./personaCache";
import { departmentTitle, groupByDepartment, isRetired, retiredAt } from "./threads";
import { designFail, designRetry, focusOnOpen, newAgentSeed, readParam, writeParam } from "./url";
import { parseVoice, voiceKind } from "./voices";

/** The first line of a persona, as the row's one-line description. */
export function personaLead(persona: string): string {
  return persona.split("\n").map((s) => s.trim()).find(Boolean) ?? "";
}

export function AgentsIndex() {
  const { client, status, conversationsVersion, pending } = useGhost();
  const { online, connection, phase } = useLive();
  const toast = useToast();
  const design = useDesign();
  const [agents, setAgents] = useState<ConversationView[] | null>(null);
  const [departments, setDepartments] = useState<DepartmentView[]>([]);
  /**
   * Each agent's persona, by slug. The list route carries no persona (ConversationView), so
   * it is read from each agent's detail after the list lands; `null` when that read failed,
   * so the row falls back to the voice. Missing while loading, unless the cache has seen
   * the slug before: then the last known persona shows at once and the read refreshes it.
   */
  const [personas, setPersonas] = useState<Record<string, string | null>>(() => Object.fromEntries(personaCache));
  /** The list itself failed to load (or refresh). */
  const [loadError, setLoadError] = useState<string | null>(null);
  /** An action on one agent failed: its own title, near what failed. */
  const [notice, setNotice] = useState<{ title: string; text: string } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [retrying, setRetrying] = useState(false);
  // New agent opens from the URL, from a retired agent's “Start an agent from this memory”, or
  // from a department's page (which names the department).
  const [seed] = useState<string | null>(() => {
    const s = newAgentSeed.slug ?? readParam("seed");
    newAgentSeed.slug = null;
    return s;
  });
  const [presetDepartment] = useState<string | null>(() => {
    const d = newAgentSeed.department ?? readParam("department");
    newAgentSeed.department = null;
    return d;
  });
  const [creating, setCreating] = useState(() => {
    const asked = newAgentSeed.open;
    newAgentSeed.open = false;
    return readParam("open") === "new" || !!seed || asked || !!presetDepartment;
  });
  const [retiredAsked] = useState(() => {
    const asked = newAgentSeed.retired;
    newAgentSeed.retired = false;
    return asked;
  });
  const [switching, setSwitching] = useState<string | null>(null);

  const inSession = !!status?.session_id;
  const liveSlug = inSession ? (status?.conversation ?? "ghost") : null;

  useEffect(() => {
    let on = true;
    const get = designFail(design, "agents")
      ? Promise.reject(new Error("Ghost couldn’t read the agents (database is locked)."))
      : client.conversations();
    client
      .departments()
      .then((d) => on && setDepartments(d))
      .catch(() => {});
    get
      .then((l) => {
        if (!on) return;
        setAgents(l);
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
  const sorted = [...(agents ?? [])].sort(
    (a, b) => rank(a) - rank(b) || (b.last_active_at ?? "").localeCompare(a.last_active_at ?? "") || a.name.localeCompare(b.name),
  );
  // Retired agents (ADR 0024) are off the roster; they wait in their own group below.
  const working = sorted.filter((l) => !isRetired(l));
  const retiredAgents = sorted.filter(isRetired).sort((a, b) => (retiredAt(b) ?? "").localeCompare(retiredAt(a) ?? ""));
  const groups = groupByDepartment(working);
  const hired = working.filter((l) => !l.is_default);
  // With only the departments' own agents, the empty state below the list carries the one call to action.
  const onlyDefaults = agents != null && hired.length === 0;

  const switchTo = async (l: ConversationView) => {
    const name = agentName(l.slug, l.name);
    setSwitching(l.slug);
    try {
      const r = await client.switchConversation(l.slug);
      if (r.ok === false) setNotice({ title: `Couldn’t switch to ${name}`, text: r.detail ?? "Ghost didn’t take the switch. Try again, or say it." });
      else toast({ title: `Transferred to ${name}` });
    } catch (e) {
      setNotice({ title: `Couldn’t switch to ${name}`, text: errorText(e) });
    } finally {
      setSwitching(null);
    }
  };

  const row = (l: ConversationView) => {
    const name = agentName(l.slug, l.name);
    const live = l.slug === liveSlug;
    const unsaved = drafts.has(l.slug);
    const v = parseVoice(l.voice);
    const state = lineState({ live, pending: !!pending, connection, phase, lastActive: l.last_active_at });
    const persona = personas[l.slug];
    const lead = typeof persona === "string" ? personaLead(persona) : "";
    return (
      <ListRow
        key={l.slug}
        href={hrefFor(`/agents/${l.slug}`)}
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
          ) : l.is_default ? (
            <span className="conv-row-desc t-3">{l.is_ghost ? "Every wake starts here" : "The department’s own agent; it speaks as its rules"}</span>
          ) : persona === null ? (
            <span className="conv-row-desc">{v.name}, {voiceKind(v)}</span>
          ) : (
            <span className="conv-row-desc t-3">No persona yet</span>
          )
        }
        meta={unsaved ? <span className="conv-row-meta">Unsaved changes</span> : l.job ? <span className="conv-row-dept">{l.job}</span> : undefined}
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
  };

  return (
    <Page
      width="default"
      title="Agents"
      status={<LiveStatus />}
      description={
        <>
          The agents you can be transferred to, by department. Say {quoted("transfer me to marketing")}, {quoted("get me the brand agent")}, or {quoted("back to Ghost")}. Each keeps its own persona, job and memory.
        </>
      }
      actions={
        onlyDefaults ? undefined : (
          <Button variant="primary" leadingIcon={<Plus />} disabled={!online} onClick={() => setCreating(true)}>
            New agent
          </Button>
        )
      }
    >
      {notice && (
        <Notice tone="danger" title={notice.title} onDismiss={() => setNotice(null)} className="conv-notice">
          {notice.text}
        </Notice>
      )}
      {loadError && agents != null && (
        <Notice tone="danger" title="Couldn’t refresh your agents" onDismiss={() => setLoadError(null)} className="conv-notice">
          {loadError} This is the list as it was.
        </Notice>
      )}

      {agents == null && loadError ? (
        <Card>
          <EmptyState
            icon={<CloudOff />}
            title="Couldn’t load your agents"
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
                  designRetry("agents");
                  setAttempt((n) => n + 1);
                }}
              >
                Try again
              </Button>
            }
          />
        </Card>
      ) : agents == null ? (
        <Card list aria-busy="true" aria-label="Loading agents">
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
          {groups.map((g) => {
            const title = departmentTitle(g.slug, departments);
            const dept = departments.find((d) => d.slug === g.slug);
            return (
              <Section
                key={g.slug}
                id={`conv-dept-${g.slug}`}
                title={g.slug === "ghost" ? "Ghost" : title}
                description={
                  g.slug === "ghost" ? (
                    <>Where every wake lands. Agents hired here work with Ghost’s own rules and skills.</>
                  ) : (
                    <>
                      {dept?.rules_lead ? <span data-user-content>{dept.rules_lead} </span> : null}
                      <Link to={`/departments/${g.slug}`}>The {title} department</Link>: its rules, skills and permissions.
                    </>
                  )
                }
              >
                <Card list>{[...(g.head ? [g.head] : []), ...g.agents].map(row)}</Card>
              </Section>
            );
          })}
          {onlyDefaults && (
            <EmptyState
              icon={<MessagesSquare />}
              title="Hire an agent for each part of your work"
              description={<>Brand design, the Q3 audit, a morning standup: each agent keeps its own persona, job and memory, and works in a department. Hire one here, then say {quoted("transfer me to brand")} in any session.</>}
              actions={
                <Button variant="primary" leadingIcon={<Plus />} disabled={!online} onClick={() => setCreating(true)}>
                  New agent
                </Button>
              }
            />
          )}
          {hired.length > 0 && (
            <p className="conv-foot t-small t-3">
              {inSession
                ? `Ghost is with ${liveSlug === "ghost" ? "Ghost" : agentName(liveSlug, sorted.find((l) => l.slug === liveSlug)?.name)}. Switch transfers the session; Ghost says so out loud.`
                : "Switch works while a session is live. Wake Ghost, then switch here or say it."}
            </p>
          )}
          {retiredAgents.length > 0 && (
            <Disclosure
              className="conv-settled-group"
              defaultOpen={readParam("open") === "retired" || retiredAsked}
              summary={
                <span className="conv-settled-group__summary">
                  Retired
                  <span className="conv-settled-group__count t-num">{retiredAgents.length}</span>
                </span>
              }
            >
              <p className="conv-settled-group__lead t-small t-2">Agents you’ve let go. Out of voice routing, still there to read, and a new agent can start from one’s memory.</p>
              <Card list className="conv-settled-group__list">
                {retiredAgents.map((l) => {
                  const name = agentName(l.slug, l.name);
                  const persona = personas[l.slug];
                  const lead = typeof persona === "string" ? personaLead(persona) : "";
                  const when = retiredAt(l);
                  return (
                    <ListRow
                      key={l.slug}
                      href={hrefFor(`/agents/${l.slug}`)}
                      leading={<LineGlyph slug={l.slug} name={name} size="lg" />}
                      title={name}
                      description={
                        lead ? (
                          <span className="conv-row-desc" data-user-content>
                            {lead}
                          </span>
                        ) : (
                          <span className="conv-row-desc t-3">No persona</span>
                        )
                      }
                      meta={<span className="conv-row-dept">in {departmentTitle(l.department, departments)}</span>}
                      trailing={
                        <span className="conv-row-end">
                          <span className="conv-row-end__when conv-row-settled">
                            <Archive size={12} aria-hidden />
                            {when ? `Retired ${relativeTime(when)}` : "Retired"}
                          </span>
                        </span>
                      }
                    />
                  );
                })}
              </Card>
            </Disclosure>
          )}
        </>
      )}

      <NewAgentDialog
        open={creating}
        onClose={() => {
          setCreating(false);
          writeParam("open", null);
        }}
        taken={(agents ?? []).flatMap((l) => [l.name.toLowerCase(), l.slug])}
        retired={retiredAgents}
        departments={departments}
        department={presetDepartment}
        seed={seed}
        onCreated={(d) => {
          setCreating(false);
          writeParam("open", null);
          toast({ title: `Hired ${d.name}`, description: "Give it a persona, then transfer to it." });
          focusOnOpen.slug = d.slug;
          navigate(`/agents/${d.slug}`);
        }}
      />
    </Page>
  );
}
