/**
 * Owner: toolkits. The `[toolkit.<name>]` tables in the policy file (ADR 0023): what a
 * department may reach, as a named bundle: servers (optionally only some reads, writes and
 * scope items, at levels no looser than the policy's) and skills. The list on the left, the
 * designer on the right; edits collect in a draft and go into the file with Save, and what
 * the daemon can't honor comes back as problems on the controls they are about.
 *
 * URL states (query before the hash):
 *   show=<name>     the selected toolkit (kept in sync as you pick one)
 *   open=new        the New toolkit dialog
 *   fail=toolkits   mock only: GET /toolkits fails
 */
import type { McpServerInfo, SkillInfo, ToolkitView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Briefcase, Plus } from "lucide-react";
import { type KeyboardEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import { Badge, Button, Card, EmptyState, ListRow, LiveStatus, Notice, Page, SaveBar, Skeleton, SkeletonText, errorText, scrollIntoPanel, useConfirm, useLive, useToast } from "../../ui";
import { Designer } from "./Designer";
import { type Draft, count, emptyDraft, humanize, policyFileHref, sameDraft, summary, toDraft, toUpdate } from "./model";
import { NewToolkitDialog } from "./NewToolkitDialog";
import "./toolkits.css";

const params = () => new URLSearchParams(window.location.search);

/** Change the query (before the hash) in place, without adding a history entry. */
function writeQuery(mutate: (q: URLSearchParams) => void) {
  const q = params();
  mutate(q);
  const search = q.toString();
  window.history.replaceState(window.history.state, "", `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`);
}
const writeShow = (name: string | null) => writeQuery((q) => (name ? q.set("show", name) : q.delete("show")));

/** An unsaved copy of a toolkit: `base` is what the file has, null for one not in it yet. */
interface Working {
  draft: Draft;
  base: Draft | null;
}

/** A row in the list: a saved toolkit, or a new one waiting to be saved. */
interface Entry {
  name: string;
  view: ToolkitView | null;
  draft: Draft;
  isNew: boolean;
  dirty: boolean;
}

export default function ToolkitsScreen(_: RouteProps) {
  const { client, status } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog: confirmDialog } = useConfirm();
  const design = useDesign();
  const init = useRef(params()).current;
  // A forced failure for design review (?fail=toolkits): development builds only, so the string never ships.
  const fail = import.meta.env.DEV && design ? init.get("fail") : null;

  const [toolkits, setToolkits] = useState<ToolkitView[] | null>(null);
  const [servers, setServers] = useState<McpServerInfo[] | null>(null);
  const [skills, setSkills] = useState<SkillInfo[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(init.get("show"));
  /** Edits by name, kept for the page's life so moving between toolkits never drops them. */
  const working = useRef(new Map<string, Working>());
  const [, rerender] = useState(0);
  const touch = () => rerender((n) => n + 1);
  const [newOpen, setNewOpen] = useState(init.get("open") === "new");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<{ title: string; text: string } | null>(null);

  const load = useCallback(async () => {
    const [t, m, s] = await Promise.allSettled([
      import.meta.env.DEV && fail === "toolkits" ? Promise.reject(new Error("Ghost didn’t answer GET /toolkits (mock).")) : client.toolkits(),
      client.mcp(),
      client.skills(),
    ]);
    if (t.status === "fulfilled") {
      setToolkits(t.value);
      setLoadError(null);
    } else setLoadError(errorText(t.reason));
    if (m.status === "fulfilled") setServers(m.value);
    if (s.status === "fulfilled") setSkills(s.value);
  }, [client, fail]);

  // Problems depend on which servers are declared and connected and which skills exist, so re-read when that set changes.
  const connectedKey = (status?.servers ?? []).join("\n");
  useEffect(() => {
    void load();
  }, [load, connectedKey]);

  // The list: the file's toolkits, then any new ones not saved yet, each with its draft.
  const entries: Entry[] = [
    ...(toolkits ?? []).map((v) => {
      const w = working.current.get(v.name);
      const draft = w?.draft ?? toDraft(v);
      return { name: v.name, view: v, draft, isNew: false, dirty: !!w && !sameDraft(draft, toDraft(v)) };
    }),
    ...[...working.current.entries()].filter(([name, w]) => w.base === null && !(toolkits ?? []).some((v) => v.name === name)).map(([name, w]) => ({ name, view: null, draft: w.draft, isNew: true, dirty: true })),
  ];
  const current = entries.find((e) => e.name === selected) ?? entries[0] ?? null;
  const dirty = !!current?.dirty;

  useEffect(() => {
    if (current) writeShow(current.name);
  }, [current?.name]);
  useEffect(() => () => writeShow(null), []);

  // A new pick while the panel is scrolled down would land mid-way through the next designer,
  // its name under the compact bar. Bring its header back, level with the list.
  const headerAnchor = useRef<HTMLSpanElement>(null);
  const shown = useRef<string | null>(null);
  useLayoutEffect(() => {
    const name = current?.name ?? null;
    const previous = shown.current;
    shown.current = name;
    if (!previous || !name || previous === name) return;
    const anchor = headerAnchor.current;
    const scroller = anchor?.closest(".shell__main");
    if (!anchor || !scroller) return;
    if (anchor.getBoundingClientRect().top < scroller.getBoundingClientRect().top - 1) scrollIntoPanel(anchor, { block: "start", smooth: false });
  }, [current?.name]);

  /* ------------------------------------------------------------------ edits */
  const change = (name: string, draft: Draft, view: ToolkitView | null) => {
    const base = view ? toDraft(view) : null;
    if (base && sameDraft(draft, base)) working.current.delete(name);
    else working.current.set(name, { draft, base });
    touch();
  };

  const create = (name: string, description: string) => {
    working.current.set(name, { draft: emptyDraft(description), base: null });
    setNewOpen(false);
    writeQuery((q) => q.delete("open"));
    setSelected(name);
    setError(null);
    touch();
  };

  const discard = () => {
    if (!current) return;
    const next = current.isNew ? (entries[entries.indexOf(current) + 1] ?? entries[entries.indexOf(current) - 1] ?? null) : current;
    working.current.delete(current.name);
    setSelected(next?.name ?? null);
    touch();
  };

  const save = async () => {
    if (!current || !current.dirty || saving || !online) return;
    setSaving(true);
    setError(null);
    try {
      const view = await client.putToolkit(current.name, toUpdate(current.draft));
      working.current.delete(current.name);
      setToolkits((list) => {
        const rest = (list ?? []).filter((v) => v.name !== view.name);
        return [...rest, view].sort((a, b) => a.name.localeCompare(b.name));
      });
      setSelected(view.name);
      toast({
        title: "Saved",
        description: view.problems.length ? `${humanize(view.name)} is in the policy file; Ghost skips the ${count(view.problems.length, "part")} it can’t honor until you fix ${view.problems.length === 1 ? "it" : "them"}.` : `Departments with ${humanize(view.name)} reach the new set from their next session.`,
        tone: view.problems.length ? "warn" : "ok",
      });
    } catch (e) {
      setError({ title: "Couldn’t save", text: errorText(e) });
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!current || current.isNew) return;
    const name = humanize(current.name);
    const ok = await confirm({
      title: `Delete ${name}?`,
      description: <>Its table leaves the policy file. A department that has it reaches less from its next session, and Ghost names the missing toolkit when it starts.</>,
      confirmLabel: `Delete ${name}`,
      tone: "danger",
    });
    if (!ok) return;
    setDeleting(true);
    setError(null);
    try {
      await client.deleteToolkit(current.name);
      const i = entries.indexOf(current);
      const next = entries[i + 1] ?? entries[i - 1] ?? null;
      working.current.delete(current.name);
      setToolkits((list) => (list ?? []).filter((v) => v.name !== current.name));
      setSelected(next?.name ?? null);
      toast({ title: `Deleted ${name}` });
    } catch (e) {
      setError({ title: `Couldn’t delete ${name}`, text: errorText(e) });
    } finally {
      setDeleting(false);
    }
  };

  // Ctrl S saves, from anywhere on the page.
  const saveRef = useRef(save);
  saveRef.current = save;
  useEffect(() => {
    const on = (e: KeyboardEvent | globalThis.KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void saveRef.current();
      }
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, []);

  /* ------------------------------------------------------------------ render */
  const dock = dirty ? (
    <SaveBar
      variant={!online ? "offline" : error?.title === "Couldn’t save" ? "error" : "dirty"}
      title={current?.isNew ? `New toolkit · ${current.name}` : "Unsaved changes"}
      detail={!online ? "they wait here until Ghost is back" : current?.isNew ? "not in the policy file yet" : "they go in the policy file on Save"}
      description={error?.title === "Couldn’t save" ? error.text : undefined}
      busy={saving}
      onDiscard={discard}
      onSave={() => void save()}
    />
  ) : undefined;

  const newButton = (variant: "primary" | "secondary") => (
    <Button variant={variant} leadingIcon={<Plus />} onClick={() => setNewOpen(true)} disabled={!online}>
      New toolkit
    </Button>
  );
  const empty = toolkits != null && entries.length === 0;

  return (
    <Page
      width="wide"
      title="Toolkits"
      status={<LiveStatus />}
      description="What a department may reach: which servers, which of their tools and items, at what level, and which skills. A toolkit only narrows your policy file; it can never loosen it."
      actions={empty ? undefined : newButton(dirty ? "secondary" : "primary")}
      dock={dock}
      dockFirst
    >
      {loadError && (
        <Notice
          tone="danger"
          title={toolkits ? "Couldn’t refresh toolkits" : "Couldn’t load toolkits"}
          action={
            <Button size="sm" onClick={() => void load()}>
              Try again
            </Button>
          }
          className="tk-notice"
        >
          <p className="tk-notice__p">{loadError}</p>
          <p className="tk-notice__p">
            Check that Ghost is running (<span className="t-mono">systemctl --user status ghostd</span>), then try again.
          </p>
        </Notice>
      )}

      {!toolkits && !loadError ? (
        <LoadingPanes />
      ) : empty ? (
        <Card>
          <EmptyState
            icon={<Briefcase />}
            title="No toolkits yet"
            description={
              <>
                A toolkit is what a department may reach: which servers, which of their tools and items, at what level, and which skills. It only narrows what your policy file already allows.
                Start one here, give it to a department on its line, and the file keeps it as <span className="t-mono">[toolkit.&lt;name&gt;]</span> (<a href={policyFileHref()} className="tk-link">see the file</a>).
              </>
            }
            actions={newButton("primary")}
          />
        </Card>
      ) : toolkits || entries.length ? (
        <div className="tk-panes">
          <ToolkitList entries={entries} current={current?.name ?? null} onSelect={setSelected} />
          <div className="tk-pane">
            {current && (
              <Designer
                key={current.name}
                name={current.name}
                draft={current.draft}
                onChange={(d) => change(current.name, d, current.view)}
                servers={servers}
                skills={skills}
                problems={current.view?.problems ?? []}
                isNew={current.isNew}
                online={online}
                deleting={deleting}
                onDelete={() => void remove()}
                error={error && error.title !== "Couldn’t save" ? error : null}
                headerAnchor={headerAnchor}
              />
            )}
          </div>
        </div>
      ) : null}

      <NewToolkitDialog
        open={newOpen}
        onClose={() => {
          setNewOpen(false);
          writeQuery((q) => q.delete("open"));
        }}
        taken={entries.map((e) => e.name)}
        onCreate={create}
      />
      {confirmDialog}
    </Page>
  );
}

/**
 * The toolkits as a listbox of rows: one tab stop (the selected row), arrows and Home/End move
 * and select. A row says what it reaches and, when the file has something wrong with it, so.
 */
function ToolkitList({ entries, current, onSelect }: { entries: Entry[]; current: string | null; onSelect: (name: string) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    const i = entries.findIndex((t) => t.name === current);
    if (i < 0) return;
    e.preventDefault();
    const next = e.key === "Home" ? 0 : e.key === "End" ? entries.length - 1 : Math.max(0, Math.min(entries.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)));
    onSelect(entries[next].name);
    ref.current?.querySelectorAll<HTMLElement>("[role=option]")[next]?.focus();
  };
  return (
    <div className="tk-list">
      <Card list>
        <div ref={ref} role="listbox" aria-label="Toolkits in the policy file" onKeyDown={onKey}>
          {entries.map((t) => {
            const sel = t.name === current;
            const problems = t.view?.problems.length ?? 0;
            return (
              <ListRow
                key={t.name}
                role="option"
                aria-selected={sel}
                tabIndex={sel ? 0 : -1}
                selected={sel}
                align="start"
                onClick={() => onSelect(t.name)}
                title={humanize(t.name)}
                description={
                  <>
                    {t.draft.description && (
                      <span className="tk-list__sum" data-user-content>
                        {t.draft.description}
                      </span>
                    )}
                    <span className="tk-list__from">{summary(t.draft)}</span>
                  </>
                }
                meta={
                  t.isNew ? (
                    <Badge size="sm" tone="info" dot>
                      New
                    </Badge>
                  ) : t.dirty ? (
                    <span className="tk-list__unsaved">Unsaved</span>
                  ) : problems ? (
                    <Badge size="sm" tone="warn" dot title={count(problems, "problem")}>
                      {problems}
                    </Badge>
                  ) : undefined
                }
              />
            );
          })}
        </div>
      </Card>
    </div>
  );
}

function LoadingPanes() {
  return (
    <div className="tk-panes" aria-busy="true" aria-label="Loading toolkits">
      <div className="tk-list">
        <Card list>
          {[120, 84, 96].map((w, i) => (
            <div key={i} className="tk-skel-row">
              <Skeleton width={w} height={12} />
              <Skeleton width={w + 60} height={10} />
            </div>
          ))}
        </Card>
      </div>
      <div className="tk-pane">
        <div className="tk-detail">
          <div className="tk-detail__header">
            <div className="tk-skel-head">
              <Skeleton width={180} height={24} />
              <Skeleton width={140} height={12} />
            </div>
            <Skeleton width={72} height={20} radius="var(--radius-full)" />
          </div>
          <SkeletonText lines={1} className="tk-skeleton-desc" />
          <Skeleton width={120} height={16} className="tk-skeleton-gap" />
          <Card>
            <SkeletonText lines={4} />
          </Card>
        </div>
      </div>
    </div>
  );
}
