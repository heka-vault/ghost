/**
 * Owner: toolkits. The designer for one toolkit (ADR 0023): its description, one card per
 * granted server with what of it the toolkit lets through (reads, writes at a level no looser
 * than the policy's, scope items), and the skills it carries. Everything here edits a draft;
 * the page's save bar writes it to the policy file with PUT /toolkits/{name}, and the
 * problems the daemon reports against the file come back inline, on the control they are about.
 */
import type { GrantView, McpServerInfo, SkillInfo } from "@ghost/client-ts";
import { autonomyLabel } from "@ghost/ui";
import { Hand, Megaphone, ShieldOff, VolumeX } from "lucide-react";
import { type ReactNode, type Ref, useId } from "react";

import { Badge, Button, Card, Checkbox, Field, Input, Notice, SegmentedControl, Select, cx } from "../../ui";
import { type Draft, allowedLevels, addable, cleanGrant, count, humanize, kindWords, levelProblem, missingSkills, problemText, readsOf, serverProblems, summary, withinCeiling } from "./model";

const LEVEL_ICON: Record<string, ReactNode> = { confirm: <Hand />, announce: <Megaphone />, quiet: <VolumeX />, deny: <ShieldOff /> };

export interface DesignerProps {
  name: string;
  draft: Draft;
  onChange: (d: Draft) => void;
  /** The servers the policy file declares (GET /mcp); null while they load. */
  servers: McpServerInfo[] | null;
  /** The skills Ghost reads (GET /skills); null while they load. */
  skills: SkillInfo[] | null;
  /** What the daemon found wrong with the saved toolkit, in its own words. */
  problems: string[];
  /** Not in the policy file yet: Save puts it there. */
  isNew: boolean;
  online: boolean;
  deleting: boolean;
  onDelete: () => void;
  /** Why the last save or delete didn't go through, if it didn't. */
  error: { title: string; text: string } | null;
  /** Placed just above the header, offset by the compact page bar; the screen scrolls to it on a new pick. */
  headerAnchor?: Ref<HTMLSpanElement>;
}

export function Designer({ name, draft, onChange, servers, skills, problems, isNew, online, deleting, onDelete, error, headerAnchor }: DesignerProps) {
  const title = humanize(name);
  const disabled = !online;
  const pickId = useId();
  const canAdd = servers ? addable(servers, draft) : [];
  const setGrant = (i: number, g: GrantView) => onChange({ ...draft, grants: draft.grants.map((x, k) => (k === i ? g : x)) });
  const addGrant = (server: string) => {
    if (!server || draft.grants.some((g) => g.server === server)) return;
    onChange({ ...draft, grants: [...draft.grants, { server, reads: null, writes: null, items: null, levels: {} }] });
  };
  const missing = missingSkills(problems);
  // Skills the toolkit names that Ghost doesn't have stay visible until unchecked.
  const extraSkills = draft.skills.filter((s) => !(skills ?? []).some((k) => k.name === s));

  return (
    <article className="tk-detail" aria-labelledby="tk-detail-title">
      <header className="tk-detail__header">
        <span ref={headerAnchor} className="tk-detail__anchor" aria-hidden />
        <div className="tk-detail__titles">
          <h2 id="tk-detail-title" className="t-title tk-detail__title">
            {title}
          </h2>
          <p className="tk-detail__meta">
            <span className="t-mono">{name}</span>
            <span aria-hidden>·</span>
            <span>{summary(draft)}</span>
          </p>
        </div>
        <div className="tk-detail__control">
          {isNew ? (
            <Badge tone="info" dot>
              Not saved yet
            </Badge>
          ) : problems.length ? (
            <Badge tone="warn" dot>
              {count(problems.length, "problem")}
            </Badge>
          ) : (
            <Badge tone="ok" dot>
              In order
            </Badge>
          )}
          <Button size="sm" variant="danger-ghost" onClick={onDelete} loading={deleting} disabled={disabled || isNew}>
            Delete
          </Button>
        </div>
      </header>

      {error && (
        <Notice tone="danger" title={error.title} className="tk-detail__notice">
          <p className="tk-notice__p">{error.text}</p>
        </Notice>
      )}

      {problems.length > 0 && (
        <Notice tone="warn" title={problems.length === 1 ? "One thing Ghost can’t honor" : `${problems.length} things Ghost can’t honor`} className="tk-detail__notice">
          <ul className="tk-problems" data-user-content>
            {problems.map((p) => (
              <li key={p}>{problemText(p)}</li>
            ))}
          </ul>
          <p className="tk-notice__p">A toolkit only narrows your policy file: a level can come down, never up, and a server or skill has to exist first. Fix them here and save.</p>
        </Notice>
      )}

      <div className="tk-fields">
        <Field label="Description" help="One line in your words: what a department with this toolkit is for.">
          <Input value={draft.description} placeholder="The books: billing issues, invoices and the month close." maxLength={200} disabled={disabled} onChange={(e) => onChange({ ...draft, description: e.target.value })} />
        </Field>
      </div>

      <div className="tk-sections">
        <Block
          title="Servers"
          count={draft.grants.length}
          description="What a department with this toolkit may reach on each server. Everything the server offers unless you narrow it; a level may be as cautious as your policy’s, or more, never looser."
          actions={
            servers && servers.length > 0 ? (
              <Select
                id={pickId}
                size="sm"
                aria-label="Add a server"
                value=""
                disabled={disabled || canAdd.length === 0}
                onChange={(e) => addGrant(e.target.value)}
                options={[{ value: "", label: canAdd.length ? "Add a server…" : "Every server is in", disabled: true }, ...canAdd.map((s) => ({ value: s.name, label: `${humanize(s.name)}${s.connected ? "" : " (not connected)"}` }))]}
              />
            ) : undefined
          }
        >
          {draft.grants.length === 0 ? (
            <p className="tk-none">
              {servers === null
                ? "Reading your servers…"
                : servers.length === 0
                  ? "No servers to grant: none is declared in your policy file yet. A toolkit with skills alone still works."
                  : "No servers yet. A department with this toolkit can’t look anything up or change anything until you add one."}
            </p>
          ) : (
            <div className="tk-grants">
              {draft.grants.map((g, i) => (
                <GrantCard
                  key={g.server}
                  grant={g}
                  server={servers?.find((s) => s.name === g.server) ?? null}
                  loading={servers === null}
                  problems={problems}
                  disabled={disabled}
                  onChange={(next) => setGrant(i, next)}
                  onRemove={() => onChange({ ...draft, grants: draft.grants.filter((_, k) => k !== i) })}
                />
              ))}
            </div>
          )}
        </Block>

        <Block title="Skills" count={draft.skills.length} description="The skills a department with this toolkit reads. A skill shapes what Ghost does, never what it may do: it can’t name a server this toolkit doesn’t grant.">
          {skills === null ? (
            <p className="tk-none">Reading your skills…</p>
          ) : skills.length === 0 && extraSkills.length === 0 ? (
            <p className="tk-none">No skills installed yet. Add one on the Skills screen, then pick it here.</p>
          ) : (
            <Card list>
              {skills.map((s) => (
                <div key={s.name} className="tk-item">
                  <Checkbox
                    id={`tk-skill-${slugId(s.name)}`}
                    checked={draft.skills.includes(s.name)}
                    disabled={disabled}
                    onChange={(on) => onChange({ ...draft, skills: on ? [...draft.skills.filter((x) => x !== s.name), s.name] : draft.skills.filter((x) => x !== s.name) })}
                    label={
                      <>
                        {humanize(s.name)} <span className="tk-item__id">{s.name}</span>
                      </>
                    }
                    description={<span data-user-content>{s.description}</span>}
                  />
                </div>
              ))}
              {extraSkills.map((s) => (
                <div key={s} className="tk-item">
                  <Checkbox
                    id={`tk-skill-${slugId(s)}`}
                    checked
                    disabled={disabled}
                    onChange={() => onChange({ ...draft, skills: draft.skills.filter((x) => x !== s) })}
                    label={
                      <>
                        {humanize(s)} <span className="tk-item__id">{s}</span>
                      </>
                    }
                    description={missing.has(s) ? "Not installed: Ghost can’t read it. Uncheck it to drop it, or add the skill on the Skills screen." : "Not installed here. Uncheck it to drop it, or add the skill on the Skills screen."}
                  />
                </div>
              ))}
            </Card>
          )}
        </Block>
      </div>
    </article>
  );
}

/* ------------------------------------------------------------------------ grants */

interface GrantCardProps {
  grant: GrantView;
  /** The declared server, or null when the file doesn't declare it (anymore). */
  server: McpServerInfo | null;
  loading: boolean;
  problems: string[];
  disabled: boolean;
  onChange: (g: GrantView) => void;
  onRemove: () => void;
}

/** One granted server: its reads, its writes with their levels, and its scope items. */
function GrantCard({ grant, server, loading, problems, disabled, onChange, onRemove }: GrantCardProps) {
  const id = useId();
  const g = cleanGrant(grant);
  const name = humanize(g.server);
  const own = serverProblems(problems, g.server);
  const reads = server ? readsOf(server) : [];
  const writes = server?.writes ?? [];
  const scope = server?.scope ?? null;
  const words = scope ? kindWords(scope.kind) : null;
  const facts = server ? [server.connected ? "Connected" : "Not connected", count(reads.length, "read"), count(writes.length, "write"), scope ? count(scope.items.length, words!.one, words!.many) : null].filter(Boolean).join(" · ") : loading ? "Reading the server…" : "Not in your policy file";
  const setLevel = (op: string, level: string) => {
    const levels = { ...g.levels };
    if (level) levels[op] = level;
    else delete levels[op];
    onChange({ ...g, levels });
  };
  const granted = (op: string) => g.writes == null || g.writes.includes(op);
  return (
    <Card className="tk-grant" padding="none">
      <div className="tk-grant__head">
        <div className="tk-grant__titles">
          <p className="tk-grant__title">
            {name} <span className="tk-item__id">{g.server}</span>
          </p>
          <p className={cx("tk-grant__facts", server && !server.connected && "is-off", !server && !loading && "is-off")}>{facts}</p>
        </div>
        <Button size="sm" variant="ghost" className="tk-grant__remove" disabled={disabled} onClick={onRemove}>
          Remove
        </Button>
      </div>
      {own.length > 0 && (
        <ul className="tk-grant__problems" data-user-content>
          {own.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      )}

      {!server && !loading ? (
        <div className="tk-grant__body">
          <p className="tk-none">
            Declare <span className="t-mono">[mcp.{g.server}]</span> in the policy file again, or remove it from this toolkit. What the toolkit named stays until you do:{" "}
            {[g.reads && `reads ${g.reads.join(", ")}`, g.writes && `writes ${g.writes.join(", ")}`, g.items && `items ${g.items.join(", ")}`].filter(Boolean).join("; ") || "everything"}.
          </p>
        </div>
      ) : (
        <div className="tk-grant__body">
          <Chooser
            id={`${id}-reads`}
            title="Reads"
            allLabel="Everything it offers"
            someLabel="Only these"
            noun="read"
            items={reads.map((t) => ({ id: t.name, label: humanize(t.name), detail: t.name, description: t.description }))}
            value={g.reads ?? null}
            disabled={disabled}
            onChange={(reads) => onChange({ ...g, reads })}
            emptyText="This server offers nothing to look up."
          />
          <Chooser
            id={`${id}-writes`}
            title="Writes"
            allLabel="Every declared write"
            someLabel="Only these"
            noun="write"
            items={writes.map((w) => ({ id: w.operation, label: humanize(w.operation), detail: `${g.server}.${w.operation}`, description: `Asked for by ${w.asked_by.join(", ")}.` }))}
            value={g.writes ?? null}
            disabled={disabled}
            onChange={(ws) => {
              // A write that is no longer granted loses its level too.
              const levels = Object.fromEntries(Object.entries(g.levels).filter(([op]) => ws == null || ws.includes(op)));
              onChange({ ...g, writes: ws, levels });
            }}
            emptyText="No writes are declared for this server, so nothing here can change anything."
            showAll
            extra={(op) =>
              granted(op) ? (
                <LevelSelect
                  id={`${id}-level-${slugId(op)}`}
                  op={op}
                  ceiling={writes.find((w) => w.operation === op)?.autonomy ?? "confirm"}
                  value={g.levels[op] ?? ""}
                  problem={levelProblem(problems, g.server, op)}
                  disabled={disabled}
                  onChange={(lv) => setLevel(op, lv)}
                />
              ) : null
            }
          />
          {scope ? (
            <Chooser
              id={`${id}-items`}
              title={words!.many[0].toUpperCase() + words!.many.slice(1)}
              allLabel={`Every ${words!.one}`}
              someLabel="Only these"
              noun={words!.one}
              nounMany={words!.many}
              items={scope.items.map((i) => ({ id: i, label: <span className="t-mono">{i}</span> }))}
              value={g.items ?? null}
              disabled={disabled}
              onChange={(items) => onChange({ ...g, items })}
              emptyText={`No ${words!.many} are allowed yet. Add them to the server’s scope in the policy file.`}
            />
          ) : (
            <div className="tk-chooser">
              <div className="tk-chooser__head">
                <p className="tk-chooser__title">Items</p>
              </div>
              <p className="tk-chooser__empty">Nothing to narrow: this server declares no scope, so a department with it reaches all of it.</p>
            </div>
          )}
        </div>
      )}
    </Card>
  );
}

/* ------------------------------------------------------------------------ choosers */

interface ChooserItem {
  id: string;
  label: ReactNode;
  detail?: string;
  description?: ReactNode;
}

interface ChooserProps {
  id: string;
  title: string;
  allLabel: string;
  someLabel: string;
  noun: string;
  nounMany?: string;
  items: ChooserItem[];
  /** null: everything the server has. */
  value: string[] | null;
  disabled: boolean;
  onChange: (value: string[] | null) => void;
  emptyText: string;
  /** Per item, a control shown beside it (a write's level); shown for every item while everything is granted. */
  extra?: (id: string) => ReactNode;
  /** Keep the rows on screen while everything is granted (the writes, for their levels). */
  showAll?: boolean;
}

/**
 * Everything, or only some: a segmented choice, then the rows. With everything on, the rows
 * read as a plain list (no checkboxes to tick) unless `showAll` keeps them for their extra
 * control. Items the grant names that the server no longer has stay visible until unchecked.
 */
function Chooser({ id, title, allLabel, someLabel, noun, nounMany = `${noun}s`, items, value, disabled, onChange, emptyText, extra, showAll }: ChooserProps) {
  const all = value == null;
  const picked = value ?? [];
  const gone = picked.filter((v) => !items.some((i) => i.id === v));
  const toggle = (item: string, on: boolean) => onChange(on ? [...picked.filter((i) => i !== item), item] : picked.filter((i) => i !== item));
  const headId = `${id}-title`;
  return (
    <div className="tk-chooser" role="group" aria-labelledby={headId}>
      <div className="tk-chooser__head">
        <p className="tk-chooser__title" id={headId}>
          {title}
          <span className="tk-chooser__count t-num">{items.length === 0 ? "" : all ? `all ${items.length}` : `${picked.length} of ${items.length}`}</span>
        </p>
        {items.length > 0 && (
          <SegmentedControl
            aria-label={`${title}: everything or only some`}
            size="sm"
            value={all ? "all" : "some"}
            onChange={(v) => onChange(v === "all" ? null : items.map((i) => i.id))}
            options={[
              { value: "all", label: allLabel },
              { value: "some", label: someLabel },
            ]}
          />
        )}
      </div>
      {items.length === 0 && gone.length === 0 ? (
        <p className="tk-chooser__empty">{emptyText}</p>
      ) : all && !showAll ? (
        <p className="tk-chooser__all">
          {items.map((i, k) => (
            <span key={i.id} className="tk-chooser__all-item">
              {i.detail ? <span className="t-mono">{i.detail}</span> : i.label}
              {k < items.length - 1 && <span aria-hidden>, </span>}
            </span>
          ))}
        </p>
      ) : (
        <ul className="tk-chooser__list" role="list">
          {items.map((i) => {
            const on = all || picked.includes(i.id);
            return (
              <li key={i.id} className={cx("tk-chooser__item", !on && "is-off")}>
                {all ? (
                  <span className="tk-chooser__plain">
                    <span className="tk-chooser__label">
                      {i.label}
                      {i.detail && <span className="tk-item__id">{i.detail}</span>}
                    </span>
                    {i.description && <span className="tk-chooser__desc">{i.description}</span>}
                  </span>
                ) : (
                  <Checkbox
                    id={`${id}-${slugId(i.id)}`}
                    checked={on}
                    disabled={disabled}
                    onChange={(v) => toggle(i.id, v)}
                    label={
                      <>
                        {i.label}
                        {i.detail && <span className="tk-item__id">{i.detail}</span>}
                      </>
                    }
                    description={i.description}
                  />
                )}
                {extra && on && <div className="tk-chooser__extra">{extra(i.id)}</div>}
              </li>
            );
          })}
          {gone.map((v) => (
            <li key={v} className="tk-chooser__item">
              <Checkbox id={`${id}-${slugId(v)}`} checked disabled={disabled} onChange={() => toggle(v, false)} label={<span className="t-mono">{v}</span>} description={`Not a ${noun} this server has anymore. Uncheck it to drop it.`} />
            </li>
          ))}
        </ul>
      )}
      {!all && items.length > 0 && picked.length === 0 && <p className="tk-chooser__help">Nothing checked: a department with this toolkit gets no {nounMany} here.</p>}
    </div>
  );
}

/** A write's level inside the toolkit: the policy's level, or anything more cautious. */
function LevelSelect({ id, op, ceiling, value, problem, disabled, onChange }: { id: string; op: string; ceiling: string; value: string; problem: string | null; disabled: boolean; onChange: (level: string) => void }) {
  const choices = allowedLevels(ceiling);
  const options = [
    { value: "", label: `Policy’s: ${autonomyLabel(ceiling)}` },
    ...choices.filter((l) => l !== ceiling).map((l) => ({ value: l, label: autonomyLabel(l) })),
    // A saved level the policy no longer allows stays selectable so the select shows what the file says, with the problem under it.
    ...(value && !withinCeiling(value, ceiling) ? [{ value, label: `${autonomyLabel(value)} (looser than the policy)` }] : []),
  ];
  const shown = value || ceiling;
  const helpId = `${id}-help`;
  return (
    <div className={cx("tk-level", problem && "is-problem")}>
      <span className="tk-level__icon" aria-hidden>
        {LEVEL_ICON[shown]}
      </span>
      <Select id={id} size="sm" aria-label={`Level for ${humanize(op)}`} aria-describedby={helpId} value={value} invalid={!!problem} disabled={disabled || choices.length <= 1} options={options} onChange={(e) => onChange(e.target.value)} />
      <span id={helpId} className={cx("tk-level__help", problem && "is-problem")}>
        {problem ? problem : choices.length <= 1 ? `Not allowed in your policy, so it can’t be here either.` : `Ceiling: ${autonomyLabel(ceiling)}`}
      </span>
    </div>
  );
}

/* ------------------------------------------------------------------------ blocks */

/** A titled block, like Section, with its count beside the heading and an action on its line (as Servers and Skills do). */
function Block({ title, count: n, description, actions, children }: { title: string; count?: number; description?: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="tk-block">
      <header className="tk-block__header">
        <div className="tk-block__head">
          <h3 className="tk-block__title">
            {title}
            {n !== undefined && (
              <span className="tk-block__count t-num" aria-label={count(n, "item")}>
                {n}
              </span>
            )}
          </h3>
          {actions && <div className="tk-block__actions">{actions}</div>}
        </div>
        {description && <p className="tk-block__description">{description}</p>}
      </header>
      {children}
    </section>
  );
}

const slugId = (s: string) => s.replace(/[^A-Za-z0-9_-]+/g, "-");
