/**
 * Owner: skills. The SKILL.md folders Ghost reads (ADR 0023): its own, under
 * ~/.config/ghost/skills, which the operator adds by dropping a folder on the list or naming
 * its path, and removes with a click; and the operator's own in ~/.agents/skills, read in
 * place, never copied or removed here (ADR 0035). A skill tells the model how to do a kind
 * of task; it never changes what Ghost may do, so there are no levels on this page. Adding
 * and removing go through the gate as skills.add and skills.remove, audited like any write.
 *
 * Find skills (#35) searches skills.sh through Ghost; a result, or a GitHub address, opens a
 * look at the skill (its SKILL.md and its scripts) before skills.install copies it in. A skill
 * installed that way says where it came from, and the page asks Ghost once per visit whether
 * its source has moved on, offering Update when it has.
 *
 * URL states (query before the hash):
 *   show=<name>    the selected skill (kept in sync as you pick one: reload and links keep it)
 *   fail=skills    mock only: GET /skills fails
 */
import type { SkillBody, SkillInfo, SkillUpdateView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { BookOpen, FolderDown, FolderPlus, Search } from "lucide-react";
import { type DragEvent, type KeyboardEvent, type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import { inTauri } from "../../transport";
import { Button, Card, EmptyState, ListRow, Notice, Page, Skeleton, SkeletonText, cx, errorText, scrollIntoPanel, useConfirm, useLive, useToast } from "../../ui";
import { AddDialog } from "./AddDialog";
import { Detail } from "./Detail";
import { FindSkills } from "./Find";
import { PreviewDialog, type PreviewStart } from "./PreviewDialog";
import { GHOST_SKILLS_DIR, humanize, needsWords, saysInstalled, sourceWord, tildePath } from "./model";
import "./skills.css";

const params = () => new URLSearchParams(window.location.search);

/** Change the query (before the hash) in place, without adding a history entry. */
function writeQuery(mutate: (q: URLSearchParams) => void) {
  const q = params();
  mutate(q);
  const search = q.toString();
  window.history.replaceState(window.history.state, "", `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`);
}

/** Keep `show=<name>` in step with the selection. */
const writeShow = (name: string | null) => writeQuery((q) => (name ? q.set("show", name) : q.delete("show")));

/** A folder Ghost refused: its path and the daemon's own sentence. */
interface Refusal {
  path: string;
  message: string;
}

/** The selected skill's SKILL.md: loading, read, or refused. */
interface BodyState {
  name: string;
  data: SkillBody | null;
  error: string | null;
}

export default function SkillsScreen(_: RouteProps) {
  const { client, status } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog: confirmDialog } = useConfirm();
  const design = useDesign();
  const init = useRef(params()).current;
  // A forced failure for design review (?fail=skills): development builds only, so the string never ships.
  const fail = import.meta.env.DEV && design ? init.get("fail") : null;

  const [skills, setSkills] = useState<SkillInfo[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(init.get("show"));
  const [body, setBody] = useState<BodyState | null>(null);
  /** SKILL.md texts already read this visit, by name; add and remove forget theirs. */
  const bodies = useRef(new Map<string, SkillBody>());
  const [addOpen, setAddOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  /** A dropped folder Ghost refused, shown over the list until dismissed or the next drop. */
  const [refusal, setRefusal] = useState<Refusal | null>(null);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);
  /** A folder is being dragged over the window: the list shows where it will land. */
  const [over, setOver] = useState(false);
  /** The look at a skill before installing it: from a search result, an address, or an update. */
  const [look, setLook] = useState<PreviewStart | null>(null);
  /** Whether each skill installed from GitHub has a newer version, asked once per visit. */
  const [updates, setUpdates] = useState<Record<string, SkillUpdateView>>({});
  const askedUpdates = useRef(false);
  const [findVersion, setFindVersion] = useState(0);

  const load = useCallback(async () => {
    try {
      const list = import.meta.env.DEV && fail === "skills" ? await Promise.reject(new Error("Ghost didn’t answer GET /skills (mock).")) : await client.skills();
      setSkills(list);
      setLoadError(null);
    } catch (e) {
      setLoadError(errorText(e));
    }
  }, [client, fail]);

  // What a skill is missing depends on which apps are connected, so re-read when that set changes.
  const connectedKey = (status?.apps ?? []).join("\n");
  useEffect(() => {
    void load();
  }, [load, connectedKey]);

  const current = skills?.find((s) => s.name === selected) ?? skills?.[0] ?? null;

  // Ask once per visit whether the skills Ghost installed from GitHub have moved on: Ghost asks
  // GitHub now, never in the background or on a turn. A failure just leaves Update off.
  useEffect(() => {
    if (askedUpdates.current || !skills?.some((s) => s.origin)) return;
    askedUpdates.current = true;
    client
      .skillUpdates()
      .then((list) => setUpdates(Object.fromEntries(list.map((u) => [u.name, u]))))
      .catch(() => {});
  }, [client, skills]);

  // The selected skill's SKILL.md, read once per visit.
  useEffect(() => {
    const name = current?.name ?? null;
    if (!name) return setBody(null);
    const hit = bodies.current.get(name);
    if (hit) return setBody({ name, data: hit, error: null });
    setBody({ name, data: null, error: null });
    let live = true;
    client
      .skill(name)
      .then((d) => {
        bodies.current.set(name, d);
        if (live) setBody({ name, data: d, error: null });
      })
      .catch((e) => live && setBody({ name, data: null, error: errorText(e) }));
    return () => {
      live = false;
    };
  }, [client, current?.name]);

  // Keep the selection in the URL so reload and links land on it; tidy up on the way out.
  useEffect(() => {
    if (current) writeShow(current.name);
  }, [current?.name]);
  useEffect(() => () => writeShow(null), []);

  // A new pick while the panel is scrolled down would land mid-way through the next skill's
  // instructions, its name under the compact bar. Bring its header back, level with the list;
  // never move when the header is already in view.
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

  /* ------------------------------------------------------------------ writes */
  /** Copy one folder into Ghost's skills. Throws the daemon's refusal; the caller shows it where it happened. */
  const add = useCallback(
    async (path: string, replace = false): Promise<SkillInfo> => {
      setAdding(true);
      try {
        const info = await client.addSkill({ path, replace });
        bodies.current.delete(info.name);
        setRefusal(null);
        setSelected(info.name);
        await load();
        toast({ title: replace ? "Replaced" : "Added", description: `Ghost reads ${humanize(info.name)} from its next session.`, tone: "ok" });
        return info;
      } finally {
        setAdding(false);
      }
    },
    [client, load, toast],
  );

  /** Folders dropped on the window: each in turn; the first refusal stops the rest and shows. */
  const addDropped = useCallback(
    async (paths: string[], replace = false) => {
      for (const path of paths) {
        try {
          await add(path, replace);
        } catch (e) {
          setRefusal({ path, message: errorText(e) });
          return;
        }
      }
    },
    [add],
  );

  // In the app the webview reports a native drag (Tauri 2): paths come absolute, so a drop
  // adds straight away. A browser never gives a path, so there the drop opens the dialog.
  useEffect(() => {
    if (!inTauri) return;
    let unlisten: (() => void) | null = null;
    let gone = false;
    import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((e) => {
          const p = e.payload;
          if (p.type === "enter" || p.type === "over") setOver(true);
          else if (p.type === "leave") setOver(false);
          else {
            setOver(false);
            void addDropped(p.paths);
          }
        }),
      )
      .then((u) => {
        if (gone) u();
        else unlisten = u;
      })
      .catch(() => {});
    return () => {
      gone = true;
      unlisten?.();
    };
  }, [addDropped]);
  const browserDrag = inTauri
    ? {}
    : {
        onDragOver: (e: DragEvent<HTMLElement>) => {
          e.preventDefault();
          setOver(true);
        },
        onDragLeave: (e: DragEvent<HTMLElement>) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
        },
        onDrop: (e: DragEvent<HTMLElement>) => {
          e.preventDefault();
          setOver(false);
          if (online) setAddOpen(true);
        },
      };

  const remove = async (s: SkillInfo) => {
    const name = humanize(s.name);
    const ok = await confirm({
      title: `Remove ${name}?`,
      description: (
        <>
          Ghost stops reading it and deletes <span className="t-mono">{tildePath(s.path)}</span>. A copy you kept elsewhere stays.
        </>
      ),
      confirmLabel: `Remove ${name}`,
      tone: "danger",
    });
    if (!ok) return;
    setRemoving(true);
    setRemoveError(null);
    try {
      await client.removeSkill(s.name);
      bodies.current.delete(s.name);
      const i = skills?.findIndex((x) => x.name === s.name) ?? -1;
      const next = skills?.[i + 1] ?? skills?.[i - 1] ?? null;
      setSelected(next?.name ?? null);
      await load();
      toast({ title: "Removed", description: `Ghost no longer reads ${name}.` });
    } catch (e) {
      setRemoveError(errorText(e));
    } finally {
      setRemoving(false);
    }
  };

  /** A skill came in from GitHub: select it, re-read the list, and mark its update done. */
  const installed = async (info: SkillInfo, updated: boolean) => {
    bodies.current.delete(info.name);
    setSelected(info.name);
    if (updated) setUpdates(({ [info.name]: _, ...rest }) => rest);
    setFindVersion((v) => v + 1);
    await load();
    toast({ title: updated ? "Updated" : "Installed", description: `Ghost reads ${humanize(info.name)} from its next session.`, tone: "ok" });
  };

  /** Find skills sits under the panes, past a long SKILL.md: the header's button brings it up and puts the cursor in it. */
  const findSkills = () => {
    const input = document.getElementById("skl-find-search");
    if (!input) return;
    scrollIntoPanel(document.getElementById("skl-find"), { block: "start" });
    input.focus({ preventScroll: true });
  };

  /* ------------------------------------------------------------------ render */
  const refusedInstalled = !!refusal && saysInstalled(refusal.message);
  return (
    <Page
      width="wide"
      title="Skills"
      description="Folders with a SKILL.md that tell Ghost how to do a kind of task; it reads one when a request calls for it. A skill shapes what Ghost does, never what it may do."
      actions={
        <>
          <Button variant="secondary" leadingIcon={<Search />} onClick={findSkills} disabled={!skills}>
            Find skills
          </Button>
          <Button variant="secondary" leadingIcon={<FolderPlus />} onClick={() => setAddOpen(true)} disabled={!online}>
            Add from folder…
          </Button>
        </>
      }
    >
      {loadError && (
        <Notice
          tone="danger"
          title={skills ? "Couldn’t refresh skills" : "Couldn’t load skills"}
          action={
            <Button size="sm" onClick={() => void load()}>
              Try again
            </Button>
          }
          className="skl-notice"
        >
          <p className="skl-notice__p">{loadError}</p>
          <p className="skl-notice__p">
            Check that Ghost is running (<span className="t-mono">systemctl --user status ghostd</span>), then try again.
          </p>
        </Notice>
      )}

      {refusal && (
        <Notice
          tone="danger"
          title={refusedInstalled ? "Already installed" : "Couldn’t add that folder"}
          className="skl-notice"
          onDismiss={() => setRefusal(null)}
          action={
            refusedInstalled ? (
              <Button size="sm" loading={adding} disabled={!online} onClick={() => void addDropped([refusal.path], true)}>
                Replace
              </Button>
            ) : undefined
          }
        >
          <p className="skl-notice__p" data-user-content>
            {refusal.message}
          </p>
          <p className="skl-notice__p">{refusedInstalled ? "Replace it to use this folder’s copy instead; the one Ghost has is deleted." : "Drop the folder that holds SKILL.md, not a file or a folder of folders."}</p>
        </Notice>
      )}

      {!skills && !loadError ? (
        <LoadingPanes />
      ) : skills && skills.length === 0 ? (
        <DropZone over={over} {...browserDrag}>
          <Card className="skl-empty">
            <EmptyState
              icon={<BookOpen />}
              title="No skills yet"
              description={
                <>
                  Put a folder with a <span className="t-mono">SKILL.md</span> at <span className="t-mono">{GHOST_SKILLS_DIR}/&lt;name&gt;/</span>,{" "}
                  {inTauri ? "drop one here, use Add from folder…, or find one below." : "use Add from folder…, or find one below."} Yours in{" "}
                  <span className="t-mono">~/.agents/skills</span> are read in place.
                </>
              }
            />
          </Card>
        </DropZone>
      ) : skills ? (
        <div className="skl-panes">
          <div className="skl-list">
            <DropZone over={over} {...browserDrag}>
              <SkillList skills={skills} current={current?.name ?? null} onSelect={setSelected} updates={updates} />
            </DropZone>
            <p className="skl-hint">{inTauri ? "Drop a skill’s folder on the list to add it." : "Add from folder… copies a folder on this machine into Ghost’s skills."}</p>
          </div>
          <div className="skl-pane">
            {current && (
              <Detail
                key={current.name}
                skill={current}
                body={body?.name === current.name ? body.data : null}
                bodyError={body?.name === current.name ? body.error : null}
                error={removeError}
                online={online}
                removing={removing}
                onRemove={() => void remove(current)}
                update={updates[current.name] ?? null}
                onUpdate={() => current.origin && setLook({ mode: "update", name: current.name, from: current.origin.commit })}
                headerAnchor={headerAnchor}
              />
            )}
          </div>
        </div>
      ) : null}

      {skills && <FindSkills version={findVersion} onPick={(listing) => setLook({ mode: "listing", listing })} onAddress={() => setLook({ mode: "address" })} />}

      <AddDialog open={addOpen} busy={adding} onClose={() => setAddOpen(false)} onAdd={add} />
      <PreviewDialog start={look} onClose={() => setLook(null)} onInstalled={(info, updated) => void installed(info, updated)} />
      {confirmDialog}
    </Page>
  );
}

/**
 * Where a dropped folder lands: the list, or the empty card. While a drag is over the window a
 * dashed accent frame lies over it and says what will happen; it takes no pointer events, so
 * the drop still reaches the webview.
 */
function DropZone({ over, children, ...drag }: { over: boolean; children: ReactNode; onDragOver?: (e: DragEvent<HTMLElement>) => void; onDragLeave?: (e: DragEvent<HTMLElement>) => void; onDrop?: (e: DragEvent<HTMLElement>) => void }) {
  return (
    <div className={cx("skl-zone", over && "is-over")} {...drag}>
      {children}
      <div className="skl-zone__over" aria-hidden>
        <FolderDown />
        Drop to add it
      </div>
    </div>
  );
}

/**
 * The skills as a listbox of rows: one tab stop (the selected row), arrows and Home/End move
 * and select. A row says what the skill is for, where it comes from, and the one thing it is
 * missing.
 */
function SkillList({ skills, current, onSelect, updates }: { skills: SkillInfo[]; current: string | null; onSelect: (name: string) => void; updates: Record<string, SkillUpdateView> }) {
  const ref = useRef<HTMLDivElement>(null);
  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    const i = skills.findIndex((s) => s.name === current);
    if (i < 0) return;
    e.preventDefault();
    const next = e.key === "Home" ? 0 : e.key === "End" ? skills.length - 1 : Math.max(0, Math.min(skills.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)));
    onSelect(skills[next].name);
    ref.current?.querySelectorAll<HTMLElement>("[role=option]")[next]?.focus();
  };
  return (
    <Card list>
      <div ref={ref} role="listbox" aria-label="Skills Ghost reads" onKeyDown={onKey}>
        {skills.map((s) => {
          const sel = s.name === current;
          return (
            <ListRow
              key={s.name}
              role="option"
              aria-selected={sel}
              tabIndex={sel ? 0 : -1}
              selected={sel}
              align="start"
              onClick={() => onSelect(s.name)}
              title={humanize(s.name)}
              description={
                <>
                  <span className="skl-list__sum" data-user-content>
                    {s.description}
                  </span>
                  <span className="skl-list__from">
                    {sourceWord(s)}
                    {updates[s.name]?.update && " · update available"}
                    {s.problem ? (
                      <>
                        {" · "}
                        <span className="skl-list__needs">not offered</span>
                      </>
                    ) : (
                      s.missing.length > 0 && (
                        <>
                          {" · "}
                          <span className="skl-list__needs">{needsWords(s.missing)}</span>
                        </>
                      )
                    )}
                  </span>
                </>
              }
            />
          );
        })}
      </div>
    </Card>
  );
}

function LoadingPanes() {
  return (
    <div className="skl-panes" aria-busy="true" aria-label="Loading skills">
      <div className="skl-list">
        <Card list>
          {[120, 84, 96].map((w, i) => (
            <div key={i} className="skl-skel-row">
              <Skeleton width={w} height={12} />
              <Skeleton width={w + 60} height={10} />
            </div>
          ))}
        </Card>
      </div>
      <div className="skl-pane">
        <div className="skl-detail">
          <div className="skl-detail__header">
            <div className="skl-skel-head">
              <Skeleton width={180} height={24} />
              <Skeleton width={140} height={12} />
            </div>
            <Skeleton width={72} height={20} radius="var(--radius-full)" />
          </div>
          <SkeletonText lines={2} className="skl-skeleton-desc" />
          <Skeleton width={120} height={16} className="skl-skeleton-gap" />
          <Card tone="sunken">
            <div className="skl-skel-body">
              <SkeletonText lines={5} />
            </div>
          </Card>
        </div>
      </div>
    </div>
  );
}
