/**
 * Owner: settings. Settings › Sound › Dictation (ADR 0077 as amended 2026-11-25): hold a key to
 * dictate, and Ghost types what you say where your cursor is. The switch asks GNOME for the key
 * and for typing (its own two dialogs, so turning it on can take a while and shows it is
 * waiting); once GNOME has given a key it shows in GNOME's words with Change key, which opens
 * GNOME's own picker. "Tidy with" chooses where the words go first (Off, the operator's Claude
 * Code with one of its models, or OpenRouter with its key, ADR 0078 as amended) and says where
 * and about how long. Each change applies at once, like the rest of this page's sections below
 * the form.
 */
import type { DictationKeyBody, DictationKeyView, PasswordInfo, PasswordsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState, type ReactNode } from "react";

import { Button, Card, Field, Input, Kbd, ListRow, Notice, Section, Select, Spinner, Switch, errorText, useLive, useToast } from "../../ui";
import { ANOTHER_OPENROUTER, BINDING, HOLD_OR_TAP, TYPES_WHERE, keyBusy, keyName, keyNotes, keyShown, tidyBody, tidyModelProblem, tidyOptions, tidyValue, tidyWhere } from "./dictation";
import { PasswordRow } from "./Passwords";
import { Row } from "./parts";
import { SECTION_TITLE, sectionDomId } from "./model";

export function DictationSection() {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [view, setView] = useState<DictationKeyView | null>(null);
  const [pending, setPending] = useState<DictationPending>(null);

  useEffect(() => {
    let live = true;
    client
      .dictationKey()
      .then((v) => live && setView(v))
      .catch(() => live && setView(null));
    return () => {
      live = false;
    };
  }, [client]);

  // A Ghost without the key (an older daemon) has nothing to show here.
  if (!view) return null;

  const turn = async (on: boolean) => {
    setPending(on ? "on" : "off");
    try {
      setView(await client.setDictationKey({ on }));
    } catch (e) {
      toast({ tone: "danger", title: on ? "Couldn’t turn the key on" : "Couldn’t turn the key off", description: errorText(e) });
    } finally {
      setPending(null);
    }
  };
  const tidy = async (body: DictationKeyBody) => {
    setPending("tidy");
    try {
      setView(await client.setDictationKey(body));
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t change it", description: errorText(e) });
    } finally {
      setPending(null);
    }
  };
  const change = async () => {
    setPending("change");
    try {
      setView(await client.changeDictationKey());
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t change the key", description: errorText(e) });
    } finally {
      setPending(null);
    }
  };

  return (
    <DictationRows
      view={view}
      pending={pending}
      online={online}
      onTurn={(on) => void turn(on)}
      onTidy={(b) => void tidy(b)}
      onChange={() => void change()}
      keyControl={<OpenRouterKey view={view} online={online} onView={setView} />}
    />
  );
}

const OPENROUTER_KEY = "OPENROUTER_API_KEY";

/**
 * OpenRouter's key for the tidy (ADR 0078), kept in Ghost's vault as on the Passwords page: the
 * value goes in and never comes back out. Shown right under "Tidy with" while it isn't set, so
 * OpenRouter's options can be turned on there, and while OpenRouter tidies.
 */
function OpenRouterKey({ view, online, onView }: { view: DictationKeyView; online: boolean; onView: (v: DictationKeyView) => void }) {
  const { client } = useGhost();
  const toast = useToast();
  const [vault, setVault] = useState<PasswordsView | null>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    client.passwords().then(setVault, () => {});
  }, [client]);
  const row: PasswordInfo = vault?.passwords.find((p) => p.name === OPENROUTER_KEY) ?? {
    name: OPENROUTER_KEY,
    set: !!view.openrouter,
    ghost: true,
    used_by: ["Dictation’s tidy"],
    from_environment: false,
    offered: false,
    added: null,
    updated: null,
    last_used: null,
  };
  const reread = () => {
    client.dictationKey().then(onView, () => {});
  };
  const forget = async (p: PasswordInfo) => {
    try {
      setVault((await client.deletePassword(p.name)).passwords);
      reread();
    } catch (e) {
      toast({ title: "Couldn’t delete it", description: errorText(e), tone: "danger" });
    }
  };
  const closed = !!vault && vault.vault.state !== "open";
  return (
    <div className="set-model-key">
      <PasswordRow
        p={row}
        open={open}
        onOpen={() => setOpen(true)}
        onClose={() => setOpen(false)}
        onSaved={(v) => {
          setVault(v);
          reread();
        }}
        onDelete={(p) => void forget(p)}
        waiting={false}
        disabled={!online || closed}
      />
    </div>
  );
}

export type DictationPending = "on" | "off" | "tidy" | "change" | null;

/** The section as drawn from the key's view and what is in flight; the calls are the caller's. */
export function DictationRows({
  view,
  pending,
  online,
  onTurn,
  onTidy,
  onChange,
  keyControl,
}: {
  view: DictationKeyView;
  pending: DictationPending;
  online: boolean;
  onTurn: (on: boolean) => void;
  onTidy: (body: DictationKeyBody) => void;
  onChange: () => void;
  /** OpenRouter's key, right under "Tidy with" while it isn't set or OpenRouter tidies. */
  keyControl?: ReactNode;
}) {
  const busy = keyBusy(view, pending === "on" || pending === "off");
  const asking = pending === "on" || view.state === "binding";
  const name = keyName(view.key);
  const notes = keyNotes(view);
  const [another, setAnother] = useState<string | null>(null);
  const current = tidyValue(view);
  const value = another != null ? ANOTHER_OPENROUTER : current;
  const choose = (v: string) => {
    if (v === ANOTHER_OPENROUTER) {
      // The id is typed first; nothing changes until it is saved.
      setAnother(current === ANOTHER_OPENROUTER ? (view.tidy_model ?? "") : "");
      return;
    }
    setAnother(null);
    onTidy(tidyBody(v));
  };
  const typed = another ?? (current === ANOTHER_OPENROUTER ? (view.tidy_model ?? "") : null);
  const problem = typed != null ? tidyModelProblem(typed) : null;
  const saveAnother = () => {
    if (typed == null || problem || !typed.trim()) return;
    if (typed.trim() === (view.tidy_model ?? "") && current === ANOTHER_OPENROUTER) return setAnother(null);
    onTidy(tidyBody(ANOTHER_OPENROUTER, typed));
    setAnother(null);
  };
  const showKey = !!keyControl && (!view.openrouter || view.tidy_source === "openrouter");

  return (
    <Section id={sectionDomId("dictation")} title={SECTION_TITLE.dictation} description="Speak, and Ghost types it for you. Your voice is read on this computer." className="set-section">
      <Card list>
        <ListRow
          title="Hold a key to dictate"
          description={
            <span aria-live="polite">
              {asking ? (
                <span className="set-dictation__asking">
                  <Spinner size={12} /> {BINDING}
                </span>
              ) : (
                TYPES_WHERE
              )}
            </span>
          }
          trailing={
            <Switch
              checked={pending === "on" ? true : pending === "off" ? false : view.on}
              onChange={onTurn}
              disabled={!online || busy || (view.state === "unavailable" && !view.on)}
              aria-label="Hold a key to dictate"
            />
          }
        />
        {keyShown(view) && (
          <ListRow
            title="Key"
            description={HOLD_OR_TAP}
            meta={name ? <Kbd>{name}</Kbd> : undefined}
            trailing={
              <Button size="sm" variant="secondary" loading={pending === "change"} disabled={!online || !!pending} onClick={onChange}>
                Change key
              </Button>
            }
          />
        )}
        <ListRow
          title="Tidy with"
          description={<span aria-live="polite">{tidyWhere(view)}</span>}
          trailing={
            <Select size="sm" value={value} onChange={(e) => choose(e.target.value)} disabled={!online || !!pending} aria-label="Tidy with">
              {tidyOptions(view).map((g) =>
                g.label ? (
                  <optgroup key={g.label} label={g.label}>
                    {g.options.map((o) => (
                      <option key={o.value} value={o.value} disabled={o.disabled}>
                        {o.label}
                      </option>
                    ))}
                  </optgroup>
                ) : (
                  g.options.map((o) => (
                    <option key={o.value} value={o.value} disabled={o.disabled}>
                      {o.label}
                    </option>
                  ))
                ),
              )}
            </Select>
          }
        />
        {typed != null && (
          <Row>
            <Field layout="inline" id="dictation-tidy-model" label="Model on OpenRouter" help="Any model id OpenRouter lists, like anthropic/claude-haiku-5.5." error={problem ?? undefined}>
              <Input
                mono
                value={typed}
                placeholder={view.tidy_model_default || ""}
                spellCheck={false}
                autoComplete="off"
                disabled={!online || !!pending}
                onChange={(e) => setAnother(e.target.value)}
                onBlur={saveAnother}
                onKeyDown={(e) => {
                  if (e.key === "Enter") saveAnother();
                }}
              />
            </Field>
          </Row>
        )}
        {showKey && keyControl}
      </Card>
      {notes.map((n) => (
        <Notice key={n.text} tone={n.tone}>
          {n.command
            ? n.text.split(n.command).flatMap((part, i) => (i === 0 ? [part] : [<code key={i} className="set-value is-mono">{n.command}</code>, part]))
            : n.text}
        </Notice>
      ))}
    </Section>
  );
}
