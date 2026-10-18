/** Owner: abilities. The selected ability: what it is, whether it's on, and everything it adds to Ghost. */
import type { AbilityInfo, PolicyView } from "@ghost/client-ts";
import { autonomyLabel } from "@ghost/ui";
import { ArrowRight, Ear, Hand, Megaphone, OctagonAlert, ShieldOff, VolumeX } from "lucide-react";
import { type ReactNode, type Ref, useEffect, useRef, useState } from "react";

import { hrefFor } from "../../app/router";
import { Badge, Button, Card, Disclosure, Notice, Switch, cx } from "../../ui";
import { CommandLine } from "./CommandLine";
import { type AbilityState, SLOT_LABEL, abilityName, addsSummary, hasPlainLabel, isWished, labelSaysId, plainLabel, policyHref, targetRules } from "./model";

const LEVEL_ICON: Record<string, ReactNode> = { confirm: <Hand />, announce: <Megaphone />, quiet: <VolumeX />, deny: <ShieldOff /> };

export interface DetailProps {
  ability: AbilityInfo;
  state: AbilityState;
  policy: PolicyView | null;
  /** Listed under [abilities] passive in the policy file; null when the policy file couldn't be read. */
  passiveAllowed: boolean | null;
  /** The switch can't be used: Ghost isn't answering, or the policy file can't be read. */
  locked: boolean;
  saving: boolean;
  error: string | null;
  onToggle: (on: boolean) => void;
  /** Opens the idle-audio consent for this ability (the blocked state's fix). */
  onAllow: () => void;
  /** Runs the save that failed again, unchanged. */
  onRetry: () => void;
  onDismissError: () => void;
  /** Placed just above the header, offset by the compact page bar; the screen scrolls to it on a new pick. */
  headerAnchor?: Ref<HTMLSpanElement>;
}

export function Detail({ ability: a, state, policy, passiveAllowed, locked, saving, error, onToggle, onAllow, onRetry, onDismissError, headerAnchor }: DetailProps) {
  const name = abilityName(a.name);
  const builtIn = state === "built-in";
  const wish = isWished(state);
  const loaded = a.loaded;
  const description = builtIn && a.name === "conversations" ? "Named lines with their own persona, voice and memory, like Accounting or Legal. Part of Ghost, so it’s always on." : a.description;
  const plugs = a.interceptors.length + a.slots.length + a.listeners + a.commands.length > 0;
  /** The model's own words, for rows whose label was written by hand. Others show theirs in the row. */
  const told = [
    ...a.read_tools.filter((t) => hasPlainLabel(a.name, t.name)).map((t) => ({ id: t.name, text: t.description })),
    ...a.operations.filter((o) => hasPlainLabel(a.name, o.name)).map((o) => ({ id: o.name, text: o.description })),
  ].filter((t) => t.text);
  /**
   * One quiet line under the description: what a restart changes, or what this one doesn't add.
   * With the switch word it is the only place the detail states a pending state (the list and
   * the dock say it for the page). None when blocked: the idle-audio card says why and holds
   * the two ways out.
   */
  const note =
    state === "blocked"
      ? null
      : state === "turning-off"
        ? "Still loaded until Ghost restarts."
        : !loaded
          ? wish
            ? "Restart Ghost to load it."
            : "Turn it on and restart Ghost to see what it adds."
          : addsSummary(a);
  const hasSections = loaded && (a.read_tools.length > 0 || a.operations.length > 0 || plugs);
  const blocked = state === "blocked";
  // Once a fix clears the blocked state its buttons are gone, and the dialog's opener with them:
  // hand focus to the switch instead of letting it fall to the page.
  const article = useRef<HTMLElement>(null);
  const wasBlocked = useRef(blocked);
  useEffect(() => {
    const cleared = wasBlocked.current && !blocked;
    wasBlocked.current = blocked;
    if (!cleared) return;
    // A frame later: the consent dialog closes in the same commit and returns focus to an opener that's gone.
    const id = requestAnimationFrame(() => {
      const lost = !document.activeElement || document.activeElement === document.body;
      if (lost) article.current?.querySelector<HTMLElement>("[role=switch]")?.focus();
    });
    return () => cancelAnimationFrame(id);
  }, [blocked]);

  // The name and its switch stay with their content: the header sticks under the compact page
  // bar, and shows a hairline only while it is stuck (the sentinel above it has scrolled under the bar).
  const sentinel = useRef<HTMLDivElement>(null);
  const [stuck, setStuck] = useState(false);
  useEffect(() => {
    const el = sentinel.current;
    const root = el?.closest<HTMLElement>(".shell__main");
    if (!el || !root) return;
    const bar = parseFloat(getComputedStyle(root).getPropertyValue("--page-bar-height")) || 48;
    const io = new IntersectionObserver(([e]) => setStuck(!e.isIntersecting && e.boundingClientRect.top < root.getBoundingClientRect().top + bar + 1), {
      root,
      rootMargin: `-${bar + 1}px 0px 0px 0px`,
      threshold: 0,
    });
    io.observe(el);
    return () => io.disconnect();
  }, []);

  return (
    <article ref={article} className="ab-detail" aria-labelledby="ab-detail-title">
      <div ref={sentinel} className="ab-detail__sentinel" aria-hidden />
      <header className={cx("ab-detail__header", stuck && "is-stuck")}>
        {/* Where a scroll lands so the header sits under the compact page bar, level with the list. */}
        <span ref={headerAnchor} className="ab-detail__anchor" aria-hidden />
        <div className="ab-detail__titles">
          <h2 id="ab-detail-title" className="t-title ab-detail__title">
            {name}
          </h2>
          <p className="ab-detail__meta">
            <span className="t-mono">{a.name}</span>
            <span aria-hidden>·</span>
            <span>{builtIn ? "Built in" : `Version ${a.version}`}</span>
          </p>
        </div>
        <div className="ab-detail__control">
          {builtIn ? (
            <Badge>Always on</Badge>
          ) : (
            <Switch
              checked={wish}
              onChange={(on) => !saving && onToggle(on)}
              disabled={locked}
              aria-label={`Use ${name}`}
              label={<span className={cx("ab-detail__switch-word", locked && "is-locked")}>{wish ? "On" : "Off"}</span>}
            />
          )}
        </div>
      </header>

      <p className="ab-detail__description t-body-lg" data-user-content>
        {description}
      </p>
      {note && <p className="ab-detail__note">{note}</p>}

      {builtIn && a.name === "conversations" && (
        <div className="ab-detail__links">
          <Button size="sm" variant="secondary" href={hrefFor("/conversations")} trailingIcon={<ArrowRight />}>
            Open Conversations
          </Button>
        </div>
      )}

      {error && (
        <Notice
          tone="danger"
          title="Couldn’t save"
          onDismiss={onDismissError}
          action={
            <Button size="sm" onClick={onRetry} disabled={locked || saving}>
              Try again
            </Button>
          }
          className="ab-detail__notice"
        >
          <p className="ab-notice__p">{error}</p>
          <p className="ab-notice__p">
            Your choice wasn’t saved. Check that Ghost is running and <span className="t-mono">~/.config/ghost/policy.toml</span> is writable, then try again.
          </p>
        </Notice>
      )}

      {a.passive && (
        // Amber means one thing: it hears idle audio right now. Red means Ghost won't start: the
        // list's word, the dock from elsewhere, and this card, which is where the fix lives.
        <Card tone={blocked ? "danger" : loaded ? "warn" : "default"} padding="md" className={cx("ab-passive", blocked && "is-blocked")}>
          <span className={cx("ab-passive__icon", !loaded && !blocked && "is-quiet")} aria-hidden>
            {blocked ? <OctagonAlert /> : <Ear />}
          </span>
          <div className="ab-passive__body">
            <h3 className="ab-passive__title">{blocked ? "Ghost won’t start like this" : "Hears idle audio"}</h3>
            {/* The status leads: it is what differs between abilities. The one-line reminder of what that means follows, quietly. */}
            <p className="ab-passive__status">
              <PassiveStatus loaded={loaded} wish={wish} allowed={passiveAllowed} />
            </p>
            <p className="ab-passive__text">Idle audio is everything the microphone picks up before you say the wake word, inside Ghost on this computer.</p>
            {blocked && (
              <div className="ab-passive__actions">
                <Button size="sm" variant="secondary" onClick={onAllow} disabled={locked || saving}>
                  Allow…
                </Button>
                <Button size="sm" variant="ghost" onClick={() => onToggle(false)} disabled={locked || saving}>
                  Turn it off
                </Button>
              </div>
            )}
          </div>
        </Card>
      )}

      {hasSections && (
        <div className="ab-sections">
          {(a.read_tools.length > 0 || a.operations.length > 0) && (
            <div className="ab-group-rc">
              {a.read_tools.length > 0 && (
                <Block
                  title="What it can read"
                  count={a.read_tools.length}
                  // A read-only ability already says so in the line under its description.
                  description={a.operations.length > 0 ? "Ghost looks these up on its own. They never change anything." : undefined}
                >
                  <Card list>
                    {a.read_tools.map((t) => (
                      <Row key={t.name} ability={a.name} id={t.name} description={t.description} />
                    ))}
                  </Card>
                </Block>
              )}

              {a.operations.length > 0 && (
                <Block
                  title="What it can change"
                  count={a.operations.length}
                  description="Every change is logged. Say “undo that” within ten minutes to reverse the last one, if it can be."
                  actions={
                    <Button size="sm" variant="ghost" href={policyHref(a.name)} trailingIcon={<ArrowRight />}>
                      Change levels
                    </Button>
                  }
                >
                  <Card list>
                    {a.operations.map((op) => {
                      const rules = targetRules(policy, a.name, op.name).filter(([, level]) => level !== op.autonomy);
                      return (
                        <Row
                          key={op.name}
                          ability={a.name}
                          id={op.name}
                          description={op.description}
                          extra={
                            rules.length > 0 && (
                              <ul className="ab-item__rules" aria-label="Repository rules">
                                {rules.map(([target, level]) => (
                                  <li key={target} className="ab-item__rule">
                                    {LEVEL_ICON[level]}
                                    <span>
                                      {autonomyLabel(level)} in <span className="t-mono">{target}</span>
                                    </span>
                                  </li>
                                ))}
                              </ul>
                            )
                          }
                        >
                          <span className={`ab-level ab-level--${op.autonomy}`}>
                            {LEVEL_ICON[op.autonomy]}
                            {autonomyLabel(op.autonomy)}
                          </span>
                        </Row>
                      );
                    })}
                  </Card>
                </Block>
              )}

              {told.length > 0 && (
                <Disclosure summary="What Ghost is told" className="ab-told">
                  <p className="ab-told__intro">How the ability describes each one to the model, word for word.</p>
                  <dl className="ab-told__list">
                    {told.map((t) => (
                      <div key={t.id} className="ab-told__item">
                        <dt className="t-mono">{t.id}</dt>
                        <dd data-user-content>{t.text}</dd>
                      </div>
                    ))}
                  </dl>
                </Disclosure>
              )}
            </div>
          )}

          {plugs && (
            <Block title="How it plugs in">
              <Card list>
                {a.interceptors.length > 0 && (
                  <Hook label="Hears you first" help="Sees what you say before the model does. It can answer, cancel, undo or put Ghost to sleep.">
                    {a.interceptors.map((i) => (
                      <span key={i} className="ab-hook__slot">
                        <span>{abilityName(i)}</span>
                        <span className="t-mono t-3">{i}</span>
                      </span>
                    ))}
                  </Hook>
                )}
                {a.slots.length > 0 && (
                  <Hook
                    label="In the voice pipeline"
                    help={
                      a.passive && a.slots.includes("audio_in")
                        ? "Runs on the microphone at all times, before the wake word."
                        : "Runs inside every session, on the audio or words passing through."
                    }
                  >
                    {a.slots.map((s) => (
                      <span key={s} className="ab-hook__slot">
                        <span>{SLOT_LABEL[s] ?? s}</span>
                        <span className="t-mono t-3">{s}</span>
                      </span>
                    ))}
                  </Hook>
                )}
                {a.listeners > 0 && (
                  // Ghost reports only how many; which events each one listens to isn't known.
                  <Hook label="Reacts to events" help="Told after wakes, session starts and ends, and each change. It can’t hold Ghost up.">
                    <span>
                      {a.listeners} {a.listeners === 1 ? "listener" : "listeners"}
                    </span>
                  </Hook>
                )}
                {a.commands.length > 0 && (
                  <Hook label="Command line" help="Run these in a terminal.">
                    {a.commands.map((c) => (
                      <CommandLine key={c} command={`ghostd ${a.name} ${c}`} />
                    ))}
                  </Hook>
                )}
              </Card>
            </Block>
          )}
        </div>
      )}
    </article>
  );
}

/**
 * One thing it can read or change. A hand-written label stands alone on one line (the model's
 * words wait under “What Ghost is told”). Otherwise the humanized id leads and the ability's own
 * description follows, wrapping to two lines; the exact id shows only when the label doesn't
 * already say it.
 */
function Row({ ability, id, description, extra, children }: { ability: string; id: string; description: string; extra?: ReactNode; children?: ReactNode }) {
  const hand = hasPlainLabel(ability, id);
  return (
    <div className="ab-item">
      <div className="ab-item__text">
        <p className="ab-item__title">
          {plainLabel(ability, id)}
          {!hand && !labelSaysId(id) && <span className="ab-item__id">{id}</span>}
        </p>
        {!hand && description && (
          <p className="ab-item__desc" data-user-content>
            {description}
          </p>
        )}
        {extra}
      </div>
      {children}
    </div>
  );
}

/**
 * Whether it hears the microphone right now, and what changes after a restart. What is loaded
 * is the fact (it hears idle audio until Ghost restarts, whatever the file says now); the
 * policy file's passive list is the permission for the next start.
 */
function PassiveStatus({ loaded, wish, allowed }: { loaded: boolean; wish: boolean; allowed: boolean | null }) {
  const list = <code>[abilities] passive</code>;
  if (loaded) {
    if (!wish) return <>Still hearing idle audio until Ghost restarts. After that it hears nothing.</>;
    if (allowed === false) return <>Hearing idle audio now, but your policy file no longer allows it, so Ghost won’t start again with it on.</>;
    if (allowed === null) return <>Hearing idle audio now.</>;
    return <>Hearing idle audio now. You allowed it under {list}.</>;
  }
  if (allowed === null) return <>Ghost couldn’t read your policy file, so it can’t tell whether this one is allowed.</>;
  // Blocked: the card's title already says Ghost won't start; this says why and the two ways out.
  if (wish) return allowed ? <>Allowed under {list}. It starts hearing idle audio when Ghost restarts.</> : <>It’s on, but your policy file doesn’t list it under {list}. Allow it to hear idle audio, or turn it off.</>;
  return allowed ? <>Off, so it hears nothing. Your policy file still allows it under {list}.</> : <>Off, so it hears nothing. Turning it on asks for your okay first.</>;
}

/**
 * A titled block. Like Section, but its action sits on the heading's line rather than the
 * description's last line (Section bottom-aligns actions; see shared_requests).
 */
function Block({ title, count, description, actions, children }: { title: string; count?: number; description?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="ab-block">
      <header className="ab-block__header">
        <div className="ab-block__head">
          <h3 className="ab-block__title">
            {title}
            {count !== undefined && <span className="ab-block__count t-num">{count}</span>}
          </h3>
          {actions && <div className="ab-block__actions">{actions}</div>}
        </div>
        {description && <p className="ab-block__description">{description}</p>}
      </header>
      {children}
    </section>
  );
}

function Hook({ label, help, children }: { label: string; help: string; children: ReactNode }) {
  return (
    <div className="ab-hook">
      <div className="ab-hook__label">
        <h4 className="ab-hook__title">{label}</h4>
        <p className="ab-hook__help">{help}</p>
      </div>
      <div className="ab-hook__value">{children}</div>
    </div>
  );
}
