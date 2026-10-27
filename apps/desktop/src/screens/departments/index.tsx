/**
 * Owner: departments. The departments (ADR 0024): folders under ~/.config/ghost, each a
 * layer around a group of servers, skills, rules and a shared memory, with agents hired
 * into it. Ghost is one too.
 *   #/departments          every department: what it reaches, its agents, its problems
 *   #/departments/:slug    one department: rules, shared memory, servers, skills, docs,
 *                          agents, the journal, Add rule, Open folder, New agent
 * The folder is the editor: Open folder reveals it; the screen shows and never rewrites
 * mcp.json or department.json. A rule added here goes through the gate, verbatim.
 */
import type { DepartmentView, JournalEntry } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Building2, ChevronLeft, CloudOff, FolderOpen, Plus, SearchX, TriangleAlert, UserPlus } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { Link, hrefFor, navigate } from "../../app/router";
import {
  Badge,
  Button,
  Card,
  Dialog,
  EmptyState,
  Field,
  Input,
  LineGlyph,
  ListRow,
  LiveStatus,
  Notice,
  Page,
  Section,
  relativeTime,
  Skeleton,
  Textarea,
  agentName,
  errorText,
  formatDateTime,
  quoted,
  useLive,
  useToast,
} from "../../ui";
import { newAgentSeed } from "../conversations/url";
import { count, listWords, openFile, reachWords, revealFolder, tildePath } from "./model";
import "./departments.css";

export default function DepartmentsScreen({ params }: RouteProps) {
  return params.slug ? <DepartmentDetail key={params.slug} slug={params.slug} /> : <DepartmentsIndex />;
}

/* ------------------------------------------------------------------ index */

function DepartmentsIndex() {
  const { client, conversationsVersion } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [departments, setDepartments] = useState<DepartmentView[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [creating, setCreating] = useState(false);

  useEffect(() => {
    let on = true;
    client
      .departments()
      .then((d) => on && (setDepartments(d), setLoadError(null)))
      .catch((e) => on && setLoadError(errorText(e)));
    return () => {
      on = false;
    };
  }, [client, conversationsVersion, attempt]);

  const problems = (departments ?? []).reduce((n, d) => n + d.problems.length, 0);

  return (
    <Page
      width="default"
      title="Departments"
      status={<LiveStatus />}
      description={<>A department is a folder: its servers, rules, skills and docs, a memory every agent in it shares, and the agents hired into it. Say {quoted("transfer me to marketing")} to work in one.</>}
      actions={
        <Button variant="primary" leadingIcon={<Plus />} disabled={!online} onClick={() => setCreating(true)}>
          New department
        </Button>
      }
    >
      {loadError && departments != null && (
        <Notice tone="danger" title="Couldn’t refresh the departments" onDismiss={() => setLoadError(null)} className="dept-notice">
          {loadError} This is the list as it was.
        </Notice>
      )}
      {departments == null && loadError ? (
        <Card>
          <EmptyState
            icon={<CloudOff />}
            title="Couldn’t load the departments"
            description={<>Ghost didn’t send them. Try again in a moment.</>}
            actions={<Button onClick={() => setAttempt((n) => n + 1)}>Try again</Button>}
          />
        </Card>
      ) : departments == null ? (
        <Card list aria-busy="true" aria-label="Loading departments">
          {[0, 1, 2].map((i) => (
            <div key={i} className="dept-skel-row">
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
          {problems > 0 && (
            <Notice tone="warn" title={count(problems, "problem") + " in the department folders"} className="dept-notice">
              Each department below says what is wrong in its files. Ghost runs without the part that is broken.
            </Notice>
          )}
          <Card list>
            {departments.map((d) => (
              <ListRow
                key={d.slug}
                href={hrefFor(`/departments/${d.slug}`)}
                leading={<LineGlyph slug={d.slug} name={d.title} size="lg" />}
                title={d.title}
                description={
                  <span className={d.rules_lead ? "dept-row-desc" : "dept-row-desc t-3"} data-user-content={d.rules_lead ? true : undefined}>
                    {d.rules_lead || (d.is_ghost ? "Where every wake lands" : "No rules yet: open the folder and write AGENTS.md")}
                  </span>
                }
                meta={d.problems.length > 0 ? <Badge tone="warn" dot size="sm">{count(d.problems.length, "problem")}</Badge> : undefined}
                trailing={<span className="dept-row-meta">{reachWords(d)}</span>}
              />
            ))}
          </Card>
          {departments.length <= 1 && (
            <EmptyState
              icon={<Building2 />}
              title="Start a department for each part of your work"
              description={<>Marketing, accounting, a project: a folder with its own servers, rules and skills, and the agents you hire into it. Start one here, then say {quoted("transfer me to marketing")} in any session.</>}
              actions={
                <Button variant="primary" leadingIcon={<Plus />} disabled={!online} onClick={() => setCreating(true)}>
                  New department
                </Button>
              }
            />
          )}
        </>
      )}
      <NewDepartmentDialog
        open={creating}
        onClose={() => setCreating(false)}
        taken={(departments ?? []).flatMap((d) => [d.title.toLowerCase(), d.slug])}
        onCreated={(d) => {
          setCreating(false);
          toast({ title: `Started ${d.title}`, description: "Open its folder to add servers and rules." });
          navigate(`/departments/${d.slug}`);
        }}
      />
    </Page>
  );
}

function NewDepartmentDialog({ open, onClose, taken, onCreated }: { open: boolean; onClose: () => void; taken: string[]; onCreated: (d: DepartmentView) => void }) {
  const { client } = useGhost();
  const { online } = useLive();
  const [name, setName] = useState("");
  const [aliases, setAliases] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open) {
      setName("");
      setAliases("");
      setError(null);
      const id = requestAnimationFrame(() => nameRef.current?.focus());
      return () => cancelAnimationFrame(id);
    }
  }, [open]);
  const trimmed = name.trim();
  const clash = trimmed && taken.includes(trimmed.toLowerCase()) ? `There’s already a department called ${trimmed}.` : null;
  const submit = async () => {
    if (!trimmed || clash || busy) return;
    setBusy(true);
    setError(null);
    try {
      onCreated(await client.createDepartment({ name: trimmed, aliases: aliases.split(",").map((a) => a.trim()).filter(Boolean), voice: null }));
    } catch (e) {
      setError(`Ghost couldn’t start it: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title="New department"
      description="A folder under ~/.config/ghost/departments with a short AGENTS.md, an empty mcp.json and its department.json. Add servers and rules in the folder."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="dept-new" loading={busy} disabled={!trimmed || !!clash || !online}>
            Start {trimmed || "department"}
          </Button>
        </>
      }
    >
      <form
        id="dept-new"
        className="dept-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="Name" error={clash ?? error} help={<>You’ll say {quoted(`transfer me to ${(trimmed || "marketing").toLowerCase()}`)}.</>}>
          <Input ref={nameRef} value={name} placeholder="Marketing" maxLength={60} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field label="Also answers to" optional help="Other names for it, separated by commas.">
          <Input value={aliases} placeholder="the marketers" onChange={(e) => setAliases(e.target.value)} />
        </Field>
      </form>
    </Dialog>
  );
}

/* ------------------------------------------------------------------ detail */

function DepartmentDetail({ slug }: { slug: string }) {
  const { client, conversationsVersion } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [d, setD] = useState<DepartmentView | null>(null);
  const [agents, setAgents] = useState<Record<string, { name: string; job: string; retired: boolean }>>({});
  const [journal, setJournal] = useState<JournalEntry[] | null>(null);
  const [missing, setMissing] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ title: string; text: string } | null>(null);
  const [ruling, setRuling] = useState(false);
  const [rule, setRule] = useState("");
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      const [dept, agentList, j] = await Promise.all([
        client.department(slug),
        client.conversations(undefined, slug).catch(() => []),
        client.departmentJournal(slug).catch(() => null),
      ]);
      setD(dept);
      setAgents(Object.fromEntries(agentList.map((a) => [a.slug, { name: a.name, job: a.job ?? "", retired: !!a.retired_at }])));
      setJournal(j);
      setMissing(false);
      setLoadError(null);
    } catch (e) {
      if ((e as { status?: number } | null)?.status === 404) setMissing(true);
      else setLoadError(errorText(e));
    }
  };
  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, slug, conversationsVersion]);

  const openFolder = async () => {
    if (!d) return;
    const r = await revealFolder(d.path);
    if (!r.ok) toast({ title: `${d.title}’s folder`, description: `${r.path} (open it in your file manager; this build can’t open folders itself).` });
  };

  /** Reveal or open a file Ghost made; a browser build shows the path instead. */
  const showFile = async (path: string, how: (p: string) => Promise<{ ok: true } | { ok: false; path: string }>) => {
    const r = await how(path);
    if (!r.ok) toast({ title: "Open it yourself", description: `${r.path} (this build can’t open files itself).` });
  };

  const addRule = async () => {
    const text = rule.trim();
    if (!text || busy) return;
    setBusy(true);
    try {
      setD(await client.addRule(slug, text));
      setRuling(false);
      setRule("");
      toast({ title: "Rule added", description: "Appended to AGENTS.md as written. It is in the audit log.", tone: "ok" });
    } catch (e) {
      setNotice({ title: "Couldn’t add the rule", text: errorText(e) });
      setRuling(false);
    } finally {
      setBusy(false);
    }
  };

  if (missing) {
    return (
      <Page width="default" title="Department not found" bare>
        <EmptyState size="page" icon={<SearchX />} title={`There’s no department called ${quoted(slug)}`} description="Its folder may have been removed, or the link has a typo." actions={<Button href="#/departments">See all departments</Button>} />
      </Page>
    );
  }

  const title = d?.title ?? agentName(slug);
  const meta: string[] = [];
  if (d) {
    meta.push(d.is_ghost ? "where every wake lands" : reachWords(d));
    if (d.root) meta.push(`resources in ${tildePath(d.root)}`);
    if (d.problems.length) meta.push(count(d.problems.length, "problem"));
  }
  const own = d?.agents.find((a) => a === d.slug) ?? null;
  const hired = (d?.agents ?? []).filter((a) => a !== d?.slug);

  return (
    <Page
      width="default"
      title={title}
      leading={<LineGlyph slug={slug} name={title} size="xl" />}
      crumb={
        <Link to="/departments">
          <ChevronLeft size={14} aria-hidden />
          Departments
        </Link>
      }
      status={<LiveStatus />}
      description={d ? <span className="dept-meta">{meta.map((m, i) => (i === 0 ? m[0].toUpperCase() + m.slice(1) : m)).join(" · ")}</span> : <Skeleton width={240} height={14} />}
      actions={
        d && (
          <>
            <Button variant="secondary" leadingIcon={<FolderOpen />} onClick={() => void openFolder()}>
              Open folder
            </Button>
            <Button
              variant="secondary"
              leadingIcon={<UserPlus />}
              disabled={!online}
              onClick={() => {
                newAgentSeed.department = slug;
                newAgentSeed.open = true;
                navigate("/agents");
              }}
            >
              New agent
            </Button>
            <Button variant="primary" disabled={!online || !!d.root} title={d.root ? `Its rules live in ${tildePath(d.root)}; edit them there` : undefined} onClick={() => setRuling(true)}>
              Add a rule
            </Button>
          </>
        )
      }
    >
      {notice && (
        <Notice tone="danger" title={notice.title} onDismiss={() => setNotice(null)} className="dept-notice">
          {notice.text}
        </Notice>
      )}
      {loadError && !d ? (
        <Card>
          <EmptyState icon={<CloudOff />} title={`Couldn’t load ${title}`} description={loadError} actions={<Button onClick={() => void load()}>Try again</Button>} />
        </Card>
      ) : !d ? (
        <div className="stack gap-6" aria-busy="true" aria-label="Loading">
          <Skeleton height={96} radius="var(--radius-md)" />
          <Skeleton height={64} radius="var(--radius-md)" />
        </div>
      ) : (
        <>
          {d.problems.length > 0 && (
            <Notice tone="warn" title={`${count(d.problems.length, "problem")} in ${title}’s files`} className="dept-notice">
              <ul>
                {d.problems.map((p) => (
                  <li key={p} className="t-mono t-small">
                    {p}
                  </li>
                ))}
              </ul>
            </Notice>
          )}

          <Section id="dept-rules" title="Rules" description={<>AGENTS.md, in every agent’s prompt here, as written. {d.root ? <>Read from <span className="t-mono">{tildePath(d.root)}</span>; Ghost never writes there.</> : <>Say {quoted(`add a rule to ${title.toLowerCase()}`)} and Ghost appends your words, verbatim, after you say yes.</>}</>}>
            <Card padding="none">
              <pre className={d.rules.trim() ? "dept-text" : "dept-text dept-text--empty"} data-user-content={d.rules.trim() ? true : undefined}>
                {d.rules.trim() || "No rules yet."}
              </pre>
            </Card>
          </Section>

          <Section id="dept-memory" title="Shared memory" description={<>What every agent in {title} knows, as data. Say {quoted(`${title.toLowerCase()} should remember that …`)} to add a line; edit memory.md in the folder to trim it.</>}>
            <Card padding="none">
              <pre className={d.memory.trim() ? "dept-text" : "dept-text dept-text--empty"} data-user-content={d.memory.trim() ? true : undefined}>
                {d.memory.trim() || "Nothing shared yet."}
              </pre>
            </Card>
          </Section>

          <Section
            id="dept-servers"
            title="Servers"
            description={
              d.is_ghost
                ? "Ghost’s servers connect when Ghost starts. Declared in mcp.json and department.json in Ghost’s folder."
                : `Its own servers start when a session enters ${title} and stay up for a while after. Declared in its mcp.json and department.json.`
            }
          >
            <Card list>
              {d.servers.length === 0 && d.uses.length === 0 ? (
                <p className="dept-text dept-text--empty">No servers. Add one to mcp.json, and its writes to department.json.</p>
              ) : (
                <>
                  {d.servers.map((s) => (
                    <div key={s} className="dept-cell">
                      <span className="dept-cell__text t-mono">{d.is_ghost ? s : `${d.slug}/${s}`}</span>
                      <span className="dept-cell__meta">{d.servers_up ? "up" : d.is_ghost ? "not connected" : "starts on transfer"}</span>
                    </div>
                  ))}
                  {d.uses.map((s) => (
                    <div key={`use-${s}`} className="dept-cell">
                      <span className="dept-cell__text t-mono">{s}</span>
                      <span className="dept-cell__meta">Ghost’s, with {title}’s own declaration</span>
                    </div>
                  ))}
                </>
              )}
            </Card>
            <p className="t-small t-3">
              Levels and declared writes show on <Link to="/servers">Servers</Link>.
            </p>
          </Section>

          {(d.watches.length > 0 || d.hooks.length > 0) && (
            <Section
              id="dept-watches"
              title="Watches and webhooks"
              description={<>What {title} keeps an eye on (ADR 0028) and the webhooks it takes (ADR 0027), declared in its department.json. Nothing here uses the model unless a watch asks for it.</>}
            >
              <Card list>
                {d.watches.map((w) => (
                  <div key={`w-${w.name}`} className="dept-cell">
                    <span className="dept-cell__text">
                      <span className="t-mono">{w.name}</span> <span className="t-3">reads {w.server}.{w.read} every {w.every_secs < 120 ? `${w.every_secs} s` : `${Math.round(w.every_secs / 60)} min`}</span>
                    </span>
                    <span className="dept-cell__meta">
                      {w.error ? `can’t read: ${w.error}` : w.last_ok ? `${w.open} open · looked ${relativeTime(w.last_ok)}` : "not run yet"}
                    </span>
                  </div>
                ))}
                {d.hooks.map((h) => (
                  <div key={`h-${h.name}`} className="dept-cell">
                    <span className="dept-cell__text">
                      <span className="t-mono">{h.name}</span> <span className="t-3">webhook at {h.path}</span>
                    </span>
                    <span className="dept-cell__meta">
                      {h.on ? `${sumOf(h.accepted)} taken · ${sumOf(h.rejected)} refused` : `off: ${h.off_reason ?? "the listener is off"}`}
                    </span>
                  </div>
                ))}
              </Card>
            </Section>
          )}

          {d.outputs.length > 0 && (
            <Section id="dept-outputs" title="Outputs" description={<>What Ghost made{d.is_ghost ? "" : ` for ${title}`}, in {d.is_ghost ? "Ghost’s" : "its"} out/ folder. Ghost never runs these; open them yourself.</>}>
              <Card list>
                {d.outputs.map((o) => (
                  <div key={o.name} className="dept-cell">
                    <span className="dept-cell__text">
                      <span className="t-mono">{o.name}</span> <span className="t-3">{formatBytes(o.bytes)} · {relativeTime(o.modified)}</span>
                    </span>
                    <span className="dept-outputs__actions">
                      <Button size="sm" variant="ghost" onClick={() => void showFile(o.path, revealFolder)}>
                        Reveal
                      </Button>
                      <Button size="sm" variant="secondary" onClick={() => void showFile(o.path, openFile)}>
                        Open
                      </Button>
                    </span>
                  </div>
                ))}
              </Card>
            </Section>
          )}

          <Section id="dept-skills" title="Skills" description={`Read first by every agent in ${title}, then your ~/.agents/skills, then Ghost’s own.`}>
            <Card list>
              {d.skills.length === 0 ? <p className="dept-text dept-text--empty">No skills of its own. Add a SKILL.md folder under skills/ in the department’s folder.</p> : d.skills.map((s) => (
                <div key={s} className="dept-cell">
                  <span className="dept-cell__text t-mono">{s}</span>
                </div>
              ))}
            </Card>
          </Section>

          {d.docs.length > 0 && (
            <Section id="dept-docs" title="Docs" description={`Reference material under docs/, read on demand. ${count(d.docs.length, "file")}.`}>
              <Card list>
                {d.docs.map((f) => (
                  <div key={f} className="dept-cell">
                    <span className="dept-cell__text t-mono">{f}</span>
                  </div>
                ))}
              </Card>
            </Section>
          )}

          <Section id="dept-agents" title="Agents" description={own ? `${agentName(own, agents[own]?.name)} is the department’s own agent, where a transfer lands. ${hired.length ? `${count(hired.length, "agent")} hired into it.` : "No one hired yet."}` : "No agents yet."}>
            <Card list>
              {d.agents.map((a) => {
                const info = agents[a];
                const name = agentName(a, info?.name);
                return (
                  <ListRow
                    key={a}
                    href={hrefFor(`/agents/${a}`)}
                    leading={<LineGlyph slug={a} name={name} size="md" />}
                    title={name}
                    description={<span className="dept-row-desc t-2">{a === d.slug ? "The department’s own agent" : info?.job || "No job given"}</span>}
                    trailing={info?.retired ? <span className="dept-row-meta">retired</span> : undefined}
                  />
                );
              })}
            </Card>
          </Section>

          <Section id="dept-journal" title="Journal" description="What the agents here did, newest first: every change made through Ghost, from the audit. A new hire reads it to catch up.">
            <Card padding="none">
              {journal == null ? (
                <p className="dept-text dept-text--empty">The journal didn’t load.</p>
              ) : journal.length === 0 ? (
                <p className="dept-text dept-text--empty">Nothing done here yet.</p>
              ) : (
                <ol className="dept-journal">
                  {journal.map((e, i) => (
                    <li key={`${e.at}-${i}`} className="dept-journal__row">
                      <span className="dept-journal__when">{formatDateTime(e.at)}</span>
                      <span className="dept-journal__what">
                        <span className="dept-journal__who">{e.agent || title}</span> · {e.what.replace(/^On [^:]+:\s*/, "")} <span className="dept-journal__op">{e.operation}</span>
                      </span>
                    </li>
                  ))}
                </ol>
              )}
            </Card>
          </Section>

          <p className="dept-files t-small t-3">
            The folder: <span className="t-mono">{tildePath(d.path)}</span>. {d.aliases.length ? <>Also answers to {listWords(d.aliases)}.</> : null} <TriangleAlert size={12} aria-hidden style={{ verticalAlign: "-2px" }} /> mcp.json and department.json are yours; Ghost reads them and never writes them.
          </p>
        </>
      )}
      <Dialog
        open={ruling}
        onClose={() => setRuling(false)}
        dismissible={!busy}
        size="md"
        title={`Add a rule to ${title}`}
        description="Appended to AGENTS.md exactly as written and read by every agent here from its next turn. The audit log records it."
        footer={
          <>
            <Button variant="ghost" onClick={() => setRuling(false)} disabled={busy}>
              Cancel
            </Button>
            <Button variant="primary" loading={busy} disabled={!rule.trim() || !online} onClick={() => void addRule()}>
              Add the rule
            </Button>
          </>
        }
      >
        <Field label="Rule" help="In your words. Ghost never paraphrases it.">
          <Textarea rows={3} value={rule} placeholder="Always use UK dates." maxLength={1000} onChange={(e) => setRule(e.target.value)} />
        </Field>
      </Dialog>
    </Page>
  );
}

/** Deliveries counted by outcome or reason, added up. */
function sumOf(counts: Record<string, number>): number {
  return Object.values(counts).reduce((a, b) => a + b, 0);
}

function formatBytes(n: number): string {
  return n < 1024 ? `${n} bytes` : n < 1024 * 1024 ? `${Math.round(n / 1024)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;
}

