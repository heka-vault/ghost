/**
 * Owner: settings. Settings › Sound › Dictation (ADR 0077 as amended 2026-11-25): hold a key to
 * dictate, and Ghost types what you say where your cursor is. The switch asks GNOME for the key
 * and for typing (its own two dialogs, so turning it on can take a while and shows it is
 * waiting); once GNOME has given a key it shows in GNOME's words with Change key, which opens
 * GNOME's own picker. The tidy sends the words to the model first, and says which one. Each
 * change applies at once, like the rest of this page's sections below the form.
 */
import type { DictationKeyView, PasswordInfo, PasswordsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

import { Button, Card, Field, Input, Kbd, ListRow, Notice, Section, Spinner, Switch, errorText, useLive, useToast } from "../../ui";
import { BINDING, HOLD_OR_TAP, OPENROUTER_TITLE, TYPES_WHERE, keyBusy, keyName, keyNotes, keyShown, openRouterLine, tidyLine, tidyModelProblem } from "./dictation";
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
  const tidy = async (on: boolean) => {
    setPending("tidy");
    try {
      setView(await client.setDictationKey({ tidy: on }));
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
    <>
      <DictationRows view={view} pending={pending} online={online} onTurn={(on) => void turn(on)} onTidy={(on) => void tidy(on)} onChange={() => void change()} />
      <OpenRouterTidy view={view} online={online} onView={setView} />
    </>
  );
}

const OPENROUTER_KEY = "OPENROUTER_API_KEY";

/**
 * Tidy with OpenRouter (ADR 0078): its password, kept in Ghost's vault as on the Passwords page
 * (the value goes in and never comes back out), and the model asked there. With the password
 * set, the tidy goes to OpenRouter instead of the operator's model.
 */
function OpenRouterTidy({ view, online, onView }: { view: DictationKeyView; online: boolean; onView: (v: DictationKeyView) => void }) {
  const { client } = useGhost();
  const toast = useToast();
  const [vault, setVault] = useState<PasswordsView | null>(null);
  const [open, setOpen] = useState(false);
  const [model, setModel] = useState(view.tidy_model ?? "");
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    client.passwords().then(setVault, () => {});
  }, [client]);
  useEffect(() => setModel(view.tidy_model ?? ""), [view.tidy_model]);
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
  const problem = tidyModelProblem(model);
  const saveModel = async () => {
    if (problem || model.trim() === (view.tidy_model ?? "")) return;
    setSaving(true);
    try {
      onView(await client.setDictationKey({ tidy_model: model.trim() }));
    } catch (e) {
      toast({ title: "Couldn’t change the model", description: errorText(e), tone: "danger" });
    } finally {
      setSaving(false);
    }
  };
  const closed = !!vault && vault.vault.state !== "open";
  return (
    <Section title={OPENROUTER_TITLE} description={openRouterLine(view)} className="set-section">
      <Card list>
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
        <Row>
          <Field
            layout="inline"
            id="dictation-tidy-model"
            label="Model on OpenRouter"
            help={`Empty means ${view.tidy_model_default || "OpenRouter’s quick default"}. Any model id OpenRouter lists works.`}
            error={problem ?? undefined}
          >
            <Input
              mono
              value={model}
              placeholder={view.tidy_model_default || ""}
              spellCheck={false}
              autoComplete="off"
              disabled={!online || saving}
              onChange={(e) => setModel(e.target.value)}
              onBlur={() => void saveModel()}
              onKeyDown={(e) => {
                if (e.key === "Enter") void saveModel();
              }}
            />
          </Field>
        </Row>
      </Card>
    </Section>
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
}: {
  view: DictationKeyView;
  pending: DictationPending;
  online: boolean;
  onTurn: (on: boolean) => void;
  onTidy: (on: boolean) => void;
  onChange: () => void;
}) {
  const busy = keyBusy(view, pending === "on" || pending === "off");
  const asking = pending === "on" || view.state === "binding";
  const name = keyName(view.key);
  const notes = keyNotes(view);

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
          title="Tidy with the model"
          description={tidyLine(view.tidy_with)}
          trailing={<Switch checked={view.tidy} onChange={onTidy} disabled={!online || !!pending} aria-label="Tidy with the model" />}
        />
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
