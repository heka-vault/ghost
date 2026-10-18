/**
 * Owner: abilities. Installed packages that give Ghost things to read, change, hear or react
 * to (ADR 0012), and the built-in conversations ability (ADR 0014). The list on the left,
 * the selected one on the right. Turning one on or off writes the policy file's enable list
 * and takes effect when Ghost restarts; one that hears idle audio asks first.
 *
 * URL states (query before the hash):
 *   show=<name>          the selected ability (kept in sync as you pick one: reload and links keep it)
 *   allow=1              mock only: open the idle-audio consent for the selected ability
 *   add=1                mock only: open "Add an ability"
 *   fail=abilities|policy|save  mock only: GET /abilities fails, GET /policy fails (an
 *                        unparseable policy.toml answers 422), or saving a toggle fails
 *   abilities=none|restart|blocked  mock fixture: no packages / changes waiting for a restart /
 *                        a passive ability on but not allowed (an edited policy file)
 */
import type { AbilityInfo, PolicyUpdate, PolicyView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ArrowRight, Blocks, Ear, OctagonAlert, Plus } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { hrefFor } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import { Badge, Button, Card, EmptyState, LiveStatus, Notice, Page, SaveBar, Skeleton, SkeletonText, cx, errorText, scrollIntoPanel, useLive, useToast } from "../../ui";
import { Detail } from "./Detail";
import { AddDialog, PassiveDialog } from "./Dialogs";
import { type AbilityState, RESTART_COMMAND, STATE_LABEL, abilityName, andList, split, stateOf, wants } from "./model";
import "./abilities.css";

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

/** A design-state param (`allow=1`, `add=1`) has done its job once its dialog closes: a reload or a copied link shouldn't reopen it. */
const dropParam = (key: string) => writeQuery((q) => q.delete(key));

export default function AbilitiesScreen(_: RouteProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const design = useDesign();
  const toast = useToast();
  const init = useRef(params()).current;
  // Forced failures for design review (?fail=abilities|policy|save): development builds only,
  // so the strings never ship.
  const fail = import.meta.env.DEV && design ? init.get("fail") : null;

  const [items, setItems] = useState<AbilityInfo[] | null>(null);
  const [policy, setPolicy] = useState<PolicyView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** GET /policy failed (422 when policy.toml doesn't parse). The list still shows; the switches don't work. */
  const [policyError, setPolicyError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(init.get("show"));
  /** The switch follows the click at once; cleared when the save settles. */
  const [optimistic, setOptimistic] = useState<Record<string, boolean>>({});
  const [saving, setSaving] = useState<string | null>(null);
  /** The failed save, with the exact call to run again from “Try again”. */
  const [saveError, setSaveError] = useState<{ name: string; text: string; retry: () => void } | null>(null);
  const [asking, setAsking] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [announce, setAnnounce] = useState("");

  const load = useCallback(async () => {
    const [a, p] = await Promise.allSettled([
      import.meta.env.DEV && fail === "abilities" ? Promise.reject(new Error("Ghost didn’t answer GET /abilities (mock).")) : client.abilities(),
      import.meta.env.DEV && fail === "policy"
        ? Promise.reject(new Error("Ghost couldn’t parse ~/.config/ghost/policy.toml: line 14, expected a value after “=” (mock)."))
        : client.policy(),
    ]);
    if (a.status === "fulfilled") {
      setItems(a.value);
      setLoadError(null);
    } else setLoadError(errorText(a.reason));
    if (p.status === "fulfilled") {
      setPolicy(p.value);
      setPolicyError(null);
    } else {
      setPolicy(null);
      setPolicyError(errorText(p.reason));
    }
  }, [client, fail]);

  useEffect(() => {
    void load();
  }, [load]);

  // Design states that open on load (mock only). Their params leave the URL when the dialog closes.
  useEffect(() => {
    if (!design || !items) return;
    if (init.get("add") === "1") setAdding(true);
    if (init.get("allow") === "1" && selected) setAsking(selected);
  }, [!!items]); // once, when the list first arrives
  // A dialog opened from the URL has no opener to hand focus back to: land it where the same
  // dialog is opened by hand, so the next Tab starts from the page, not the top of the document.
  const addButton = useRef<HTMLButtonElement>(null);
  const focusIfLost = (pick: () => HTMLElement | null | undefined) =>
    requestAnimationFrame(() => {
      if (!document.activeElement || document.activeElement === document.body) pick()?.focus();
    });
  const closeAsking = () => {
    setAsking(null);
    dropParam("allow");
    focusIfLost(() => document.querySelector<HTMLElement>(".ab-detail [role=switch]"));
  };
  const closeAdding = () => {
    setAdding(false);
    dropParam("add");
    focusIfLost(() => addButton.current);
  };

  const { installed, builtIn } = useMemo(() => split(items ?? []), [items]);
  const all = useMemo(() => [...installed, ...builtIn], [installed, builtIn]);
  const current = all.find((a) => a.name === selected) ?? all[0] ?? null;

  // Keep the selection in the URL so reload and links land on it; tidy up on the way out.
  useEffect(() => {
    if (current) writeShow(current.name);
  }, [current?.name]);
  useEffect(() => () => writeShow(null), []);

  const wishOf = (a: AbilityInfo) => wants(a, policy, optimistic[a.name]);
  /** Listed under [abilities] passive; null when the policy file couldn't be read. */
  const passiveOf = (a: AbilityInfo): boolean | null => (policy ? policy.passive.includes(a.name) : null);
  const stateFor = (a: AbilityInfo): AbilityState => stateOf(a, wishOf(a), passiveOf(a));

  // A new pick while the panel is scrolled down would land mid-way through the next ability,
  // its name and switch under the compact bar. Bring its header back, level with the list;
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
  const locked = !online || !!policyError;

  const waiting = installed.filter((a) => wishOf(a) !== a.loaded);
  const blocked = installed.filter((a) => stateFor(a) === "blocked");
  /** Who hears the microphone before the wake word right now: loaded passive abilities. */
  const hearingNow = installed.filter((a) => a.passive && a.loaded);

  const save = async (a: AbilityInfo, on: boolean, passive?: string[]) => {
    const enabled = installed.filter((x) => (x.name === a.name ? on : wishOf(x))).map((x) => x.name);
    const update: PolicyUpdate = passive ? { enabled, passive } : { enabled };
    setSaving(a.name);
    setSaveError(null);
    setOptimistic((o) => ({ ...o, [a.name]: on }));
    try {
      if (import.meta.env.DEV && fail === "save") throw new Error("Ghost couldn’t write ~/.config/ghost/policy.toml: permission denied (mock).");
      const r = await client.updatePolicy(update);
      // The mock daemon doesn't store `passive` yet (see shared_requests): keep what we sent, or
      // what we last knew. The real API returns the file as written.
      setPolicy(design ? { ...r.policy, passive: update.passive ?? policy?.passive ?? r.policy.passive } : r.policy);
      const name = abilityName(a.name);
      setAnnounce(on === a.loaded ? `${name} stays ${on ? "on" : "off"}.` : `${name} turns ${on ? "on" : "off"} when Ghost restarts. Restart Ghost to apply.`);
    } catch (e) {
      setSaveError({ name: a.name, text: errorText(e), retry: () => void save(a, on, passive) });
    } finally {
      setOptimistic((o) => {
        const { [a.name]: _, ...rest } = o;
        return rest;
      });
      setSaving(null);
      closeAsking();
    }
    try {
      setItems(await client.abilities());
    } catch {
      /* the save stands; the list refreshes on the next load */
    }
  };

  const toggle = (a: AbilityInfo, on: boolean) => {
    if (saving || locked) return;
    if (on && a.passive && !passiveOf(a)) return setAsking(a.name);
    // Turning a passive ability off takes back its permission to hear idle audio too.
    if (!on && a.passive && passiveOf(a)) return void save(a, false, (policy?.passive ?? []).filter((n) => n !== a.name));
    void save(a, on);
  };

  const allowPassive = (a: AbilityInfo) => save(a, true, [...new Set([...(policy?.passive ?? []), a.name])]);
  const askingAbility = all.find((a) => a.name === asking) ?? null;

  // Blocked: the selected ability's idle-audio card explains it and holds the fixes, so the dock
  // only speaks up from elsewhere on the page, in one line, with a way to get there.
  const blockedElsewhere = blocked.filter((a) => a.name !== current?.name);
  /** The dock's button goes away once the ability is shown: land focus on the card's first fix. */
  const showBlocked = (name: string) => {
    setSelected(name);
    requestAnimationFrame(() => document.querySelector<HTMLElement>(".ab-passive__actions button")?.focus());
  };
  const dock =
    items && blocked.length > 0 ? (
      blockedElsewhere.length > 0 ? (
        <DockBar tone="danger" icon={<OctagonAlert />} title="Ghost won’t start like this" text={`${andList(blocked.map((a) => abilityName(a.name)))} ${blocked.length === 1 ? "is" : "are"} on but not allowed to hear idle audio`}>
          <Button size="sm" variant="secondary" onClick={() => showBlocked(blockedElsewhere[0].name)} trailingIcon={<ArrowRight />}>
            Show {abilityName(blockedElsewhere[0].name)}
          </Button>
        </DockBar>
      ) : undefined
    ) : items && waiting.length > 0 ? (
      // The shared restart pill, as Settings and What Ghost may do show it after a save that
      // owes a restart: the one thing left to do, what changes after it, the command with Copy.
      <SaveBar
        variant="restart"
        title="Restart Ghost to apply"
        detail={restartSentence(waiting, wishOf)}
        command={RESTART_COMMAND}
        actions={
          design ? (
            <span className="ab-dock__proto">
              <Badge size="sm" variant="outline">
                Prototype
              </Badge>
              <Button size="sm" onClick={() => toast({ title: "Prototype", description: "The app can’t restart Ghost yet. It needs a restart command in the desktop shell." })}>
                Restart Ghost
              </Button>
            </span>
          ) : undefined
        }
      />
    ) : undefined;

  return (
    <Page
      width="wide"
      title="Abilities"
      status={<LiveStatus />}
      description="What Ghost can read, change, hear and react to. Most come from packages you install; Ghost loads the ones you turn on when it starts."
      actions={
        <Button ref={addButton} leadingIcon={<Plus />} onClick={() => setAdding(true)}>
          Add an ability
        </Button>
      }
      dock={dock}
    >
      <p className="sr-only" aria-live="polite">
        {announce}
      </p>

      {loadError && (
        <Notice
          tone="danger"
          title={items ? "Couldn’t refresh abilities" : "Couldn’t load abilities"}
          action={
            <Button size="sm" onClick={() => void load()}>
              Try again
            </Button>
          }
          className="ab-notice"
        >
          <p className="ab-notice__p">{loadError}</p>
          <p className="ab-notice__p">
            Check that Ghost is running (<span className="t-mono">systemctl --user status ghostd</span>), then try again.
          </p>
        </Notice>
      )}

      {policyError && items && (
        <Notice
          tone="danger"
          title="Couldn’t read your policy file"
          action={
            <Button size="sm" variant="secondary" href={hrefFor("/policy")} trailingIcon={<ArrowRight />}>
              What Ghost may do
            </Button>
          }
          className="ab-notice"
        >
          <p className="ab-notice__p">{policyError}</p>
          <p className="ab-notice__p">Fix it in What Ghost may do. Until then you can see every ability here, but not turn one on or off.</p>
        </Notice>
      )}

      {!items && !loadError ? (
        <LoadingPanes />
      ) : items ? (
        <div className="ab-panes">
          <AbilityList
            installed={installed}
            builtIn={builtIn}
            current={current?.name ?? null}
            stateFor={stateFor}
            onSelect={setSelected}
            onAdd={() => setAdding(true)}
          />

          <div className="ab-pane">
            {current && (
              <Detail
                key={current.name}
                ability={current}
                state={stateFor(current)}
                policy={policy}
                passiveAllowed={passiveOf(current)}
                locked={locked}
                saving={saving === current.name}
                error={saveError?.name === current.name ? saveError.text : null}
                onToggle={(on) => toggle(current, on)}
                onAllow={() => setAsking(current.name)}
                onRetry={() => saveError?.retry()}
                onDismissError={() => setSaveError(null)}
                headerAnchor={headerAnchor}
              />
            )}
          </div>
        </div>
      ) : null}

      <PassiveDialog
        open={!!askingAbility}
        name={askingAbility ? abilityName(askingAbility.name) : ""}
        pkg={askingAbility?.name ?? ""}
        hearing={hearingNow.filter((a) => a.name !== askingAbility?.name).map((a) => abilityName(a.name))}
        busy={!!saving}
        onAllow={() => askingAbility && void allowPassive(askingAbility)}
        onClose={closeAsking}
      />
      <AddDialog open={adding} onClose={closeAdding} />
    </Page>
  );
}

/** "Slack DM turns on, Calendar turns off" (reads after “Restart Ghost to apply ·”). */
function restartSentence(waiting: AbilityInfo[], wishOf: (a: AbilityInfo) => boolean): string {
  const on = waiting.filter(wishOf).map((a) => abilityName(a.name));
  const off = waiting.filter((a) => !wishOf(a)).map((a) => abilityName(a.name));
  return [on.length ? `${andList(on)} ${on.length === 1 ? "turns" : "turn"} on` : "", off.length ? `${andList(off)} ${off.length === 1 ? "turns" : "turn"} off` : ""].filter(Boolean).join(", ");
}

/** The blocked bar in the page dock (a passive ability on but not allowed): one line, so opening it never moves anything above the fold. A restart owed uses the shared SaveBar. */
function DockBar({ tone = "neutral", icon, title, text, children }: { tone?: "neutral" | "danger"; icon: ReactNode; title: string; text: string; children?: ReactNode }) {
  return (
    <div className={cx("ab-dock", `ab-dock--${tone}`)} role="region" aria-label={title}>
      <span className="ab-dock__icon" aria-hidden>
        {icon}
      </span>
      <p className="ab-dock__text">
        <span className="ab-dock__title">{title}</span>
        <span aria-hidden> · </span>
        <span className="sr-only">: </span>
        {text}
      </p>
      {children && <div className="ab-dock__actions">{children}</div>}
    </div>
  );
}

/**
 * The installed abilities as a listbox: one tab stop (the selected row), arrows and Home/End
 * move and select. Quiet rows on the canvas; the only marks are the ear for one that hears
 * idle audio and a word for anything not simply on.
 */
function AbilityList({
  installed,
  builtIn,
  current,
  stateFor,
  onSelect,
  onAdd,
}: {
  installed: AbilityInfo[];
  builtIn: AbilityInfo[];
  current: string | null;
  stateFor: (a: AbilityInfo) => AbilityState;
  onSelect: (name: string) => void;
  /** Opens “Add an ability”, the same dialog as the header button, from the empty state. */
  onAdd: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const order = [...installed, ...builtIn];

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    const i = order.findIndex((a) => a.name === current);
    if (i < 0) return;
    e.preventDefault();
    const next = e.key === "Home" ? 0 : e.key === "End" ? order.length - 1 : Math.max(0, Math.min(order.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)));
    onSelect(order[next].name);
    ref.current?.querySelectorAll<HTMLElement>("[role=option]")[next]?.focus();
  };

  const option = (a: AbilityInfo) => {
    const s = stateFor(a);
    const sel = a.name === current;
    const quiet = s === "off" || s === "turning-on" || s === "blocked";
    return (
      <div
        key={a.name}
        role="option"
        aria-selected={sel}
        tabIndex={sel ? 0 : -1}
        className={cx("ab-opt", sel && "is-selected")}
        onClick={(e) => {
          onSelect(a.name);
          e.currentTarget.focus();
        }}
      >
        <span className={cx("ab-opt__name", quiet && "is-quiet")}>{abilityName(a.name)}</span>
        {a.passive && (
          <span className={cx("ab-opt__ear", !a.loaded && "is-quiet")}>
            <Ear aria-hidden />
            <span className="sr-only">{a.loaded ? ", hears idle audio" : ", can hear idle audio"}</span>
          </span>
        )}
        {s !== "on" && s !== "built-in" && (
          <span className={cx("ab-opt__state", (s === "turning-on" || s === "turning-off") && "is-pending", s === "blocked" && "is-blocked")}>
            <span className="sr-only">, </span>
            {STATE_LABEL[s]}
          </span>
        )}
      </div>
    );
  };

  const empty = (
    <div className="ab-group">
      <p className="ab-group__label">Installed</p>
      <EmptyState
        size="compact"
        headingLevel="p"
        icon={<Blocks />}
        title="No packages yet"
        className="ab-list__empty"
        description={
          <>
            Install a package, or drop a folder in <span className="t-mono">~/.config/ghost/abilities/</span>.
          </>
        }
        actions={
          <Button size="sm" variant="secondary" onClick={onAdd}>
            How to add one
          </Button>
        }
      />
    </div>
  );

  return (
    <div className="ab-list">
      {installed.length === 0 && empty}
      <div ref={ref} role="listbox" aria-label="Installed abilities" className="ab-listbox" onKeyDown={onKey}>
        {installed.length > 0 && (
          <div role="group" aria-labelledby="ab-group-installed" className="ab-group">
            <p id="ab-group-installed" className="ab-group__label">
              Installed <span className="t-num ab-group__count">{installed.length}</span>
            </p>
            {installed.map(option)}
          </div>
        )}
        {builtIn.length > 0 && (
          <div role="group" aria-labelledby="ab-group-builtin" className="ab-group">
            <p id="ab-group-builtin" className="ab-group__label">
              Built in
            </p>
            {builtIn.map(option)}
          </div>
        )}
      </div>
    </div>
  );
}

function LoadingPanes() {
  return (
    <div className="ab-panes" aria-busy="true" aria-label="Loading abilities">
      <div className="ab-list">
        <div className="ab-listbox">
          <div className="ab-group">
            <span className="ab-group__label">
              <Skeleton width={64} height={12} />
            </span>
            {[120, 96, 108, 84].map((w, i) => (
              <div key={i} className="ab-opt">
                <Skeleton width={w} height={12} />
              </div>
            ))}
          </div>
          <div className="ab-group">
            <span className="ab-group__label">
              <Skeleton width={48} height={12} />
            </span>
            <div className="ab-opt">
              <Skeleton width={100} height={12} />
            </div>
          </div>
        </div>
      </div>
      <div className="ab-pane">
        <div className="ab-detail">
          <div className="ab-detail__header">
            <div className="ab-skel-head">
              <Skeleton width={180} height={24} />
              <Skeleton width={140} height={12} />
            </div>
            <div className="ab-detail__control">
              <Skeleton width={64} height={20} radius="var(--radius-full)" />
            </div>
          </div>
          <SkeletonText lines={2} className="ab-skeleton-desc" />
          <Skeleton width={120} height={16} className="ab-skeleton-gap" />
          <Card list>
            {[0, 1, 2].map((i) => (
              <div key={i} className="ab-item">
                <div className="ab-item__text">
                  <Skeleton width={i === 2 ? "45%" : "62%"} height={14} />
                </div>
              </div>
            ))}
          </Card>
        </div>
      </div>
    </div>
  );
}
