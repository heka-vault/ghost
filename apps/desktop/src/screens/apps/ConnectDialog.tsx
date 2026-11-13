/**
 * Owner: apps. Connect an app from the catalog (POST /apps with catalog and option): pick a
 * way to connect when the entry offers several, fill its settings (a secret is a password
 * field, sent once and never shown again), and, under Advanced, the name rules use. An app
 * that signs in opens the browser; the dialog then waits for the operator to finish there.
 */
import { ApiError, type AppView, type CatalogEntryView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Check, ExternalLink } from "lucide-react";
import { type FormEvent, useEffect, useId, useState } from "react";

import { navigate } from "../../app/router";
import { useDesign } from "../../design/context";
import { Button, Dialog, Disclosure, Field, Input, Notice, RadioGroup, Select, Spinner, errorText, useLive, useToast } from "../../ui";
import { AboutApp } from "./AboutApp";
import { AppIcon } from "./AppIcon";
import { count, optionHelp, sentence } from "./model";
import { abilityOf } from "./plain";
import { expectSignIn, openSignIn } from "./signin";

/**
 * What a connect came back with. POST /apps answers once the app is saved, even when it then
 * didn't answer: the first app that failed (with its problem), else whether one is still
 * connecting.
 */
export function connectOutcome(apps: AppView[]): { failed: AppView | null; connecting: boolean } {
  const failed = apps.find((a) => a.state === "failed" || a.state === "stopped") ?? null;
  return { failed, connecting: !failed && apps.some((a) => a.state === "connecting") };
}

/** The dialog's notice text for an app that was saved and then didn't connect. */
export const failedText = (a: AppView) => `${a.problem ? sentence(a.problem) : "It didn’t answer."} Check the settings and try again.`;

export function ConnectDialog({ entry: e, onClose, onConnected, onOpen }: { entry: CatalogEntryView; onClose: () => void; onConnected: () => void; onOpen?: (name: string) => void }) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const design = useDesign();
  const mock = import.meta.env.DEV && !!design;
  const formId = useId();
  const [optionId, setOptionId] = useState(e.options[0]?.id ?? "");
  const option = e.options.find((o) => o.id === optionId) ?? e.options[0] ?? null;
  const [values, setValues] = useState<Record<string, string>>(() => Object.fromEntries(e.options.flatMap((o) => o.settings.map((s) => [`${o.id}:${s.key}`, s.default ?? ""]))));
  const [name, setName] = useState(e.suggested_name);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ title: string; text: string; taken: boolean; failed?: boolean } | null>(null);
  /** Saved by an earlier try that then didn't connect: the next try replaces it. */
  const [saved, setSaved] = useState(false);
  const [missing, setMissing] = useState<Set<string>>(new Set());
  /** Connected, or waiting for it to be: the last pane of the flow. */
  const [done, setDone] = useState<{ name: string; app: AppView | null } | null>(null);
  /** After a connect that signs in: the page Ghost opened, and whether it could open it. */
  const [signIn, setSignIn] = useState<{ url: string; opened: boolean; name: string } | null>(null);

  const value = (key: string) => values[`${option?.id}:${key}`] ?? "";
  const set = (key: string, v: string) => {
    setValues((x) => ({ ...x, [`${option?.id}:${key}`]: v }));
    if (missing.has(key)) setMissing((m) => new Set([...m].filter((k) => k !== key)));
  };

  const connect = async (replace = false) => {
    if (!option || busy || !online) return;
    const empty = option.settings.filter((s) => s.required && !value(s.key).trim());
    if (empty.length) {
      setMissing(new Set(empty.map((s) => s.key)));
      return;
    }
    setBusy(true);
    setError(null);
    const appName = name.trim() || e.suggested_name;
    expectSignIn(appName);
    try {
      const vals = Object.fromEntries(option.settings.map((s) => [s.key, value(s.key).trim()]).filter(([, v]) => v));
      const r = await client.connectApp({ catalog: e.name, option: option.id, name: appName, values: vals, replace: replace || saved || undefined });
      onConnected();
      const { failed, connecting } = connectOutcome(r.apps);
      if (failed) {
        setSaved(true);
        setError({ title: `Ghost couldn’t connect to ${e.title}`, text: failedText(failed), taken: false, failed: true });
      } else if (r.sign_in_url) {
        const opened = await openSignIn(r.sign_in_url, { again: true, mock });
        setSignIn({ url: r.sign_in_url, opened, name: r.apps[0]?.title ?? e.title });
        setDone({ name: appName, app: null });
      } else if (connecting) {
        // Still reaching it: the dialog waits with it and shows the tools once it answers.
        setDone({ name: appName, app: null });
      } else {
        setDone({ name: appName, app: r.apps.find((a) => a.name === appName) ?? r.apps[0] ?? null });
      }
    } catch (err) {
      const taken = err instanceof ApiError && err.status === 409;
      setError({ title: taken ? "Already connected under that name" : `Couldn’t connect ${e.title}`, text: sentence(errorText(err)), taken });
    } finally {
      setBusy(false);
    }
  };

  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    void connect();
  };

  // Waiting on a sign-in or a first answer: look again until the app is connected, then show what it can do.
  useEffect(() => {
    if (!done || done.app?.state === "connected") return;
    let on = true;
    const t = window.setInterval(() => {
      client
        .app(done.name)
        .then((a) => {
          if (!on) return;
          if (a.state === "connected") {
            setDone({ name: done.name, app: a });
            setSignIn(null);
            onConnected();
          } else if (a.state === "failed") {
            setDone(null);
            setSignIn(null);
            setSaved(true);
            setError({ title: `Ghost couldn’t connect to ${e.title}`, text: failedText(a), taken: false, failed: true });
          }
        })
        .catch(() => {});
    }, 1500);
    return () => {
      on = false;
      window.clearInterval(t);
    };
  }, [client, done, e.title, onConnected]);

  if (done?.app?.state === "connected") return <Connected app={done.app} onClose={onClose} onOpen={onOpen} />;
  if (done && !signIn) {
    return (
      <Dialog open onClose={onClose} size="md" title={`Connecting to ${e.title}…`} description="Ghost is reaching it. Its tools show here once it answers; you can close this and it carries on." footer={<Button variant="secondary" onClick={onClose}>Close</Button>}>
        <div className="apps-wait" aria-busy="true">
          <Spinner size={16} label="" /> <span className="t-2">Waiting for {e.title} to answer…</span>
        </div>
      </Dialog>
    );
  }
  if (signIn) {
    return (
      <Dialog
        open
        onClose={onClose}
        size="md"
        title={`Finish signing in to ${signIn.name}`}
        description={signIn.opened ? `Ghost opened ${signIn.name}’s sign-in page in your browser. Sign in there; ${signIn.name} shows as connected under Apps once you’re done.` : "Ghost couldn’t open your browser. Open the sign-in page yourself; the app shows as connected under Apps once you’re done."}
        footer={
          <>
            <Button variant="ghost" leadingIcon={<ExternalLink />} onClick={() => void openSignIn(signIn.url, { again: true, mock })}>
              Open the sign-in page again
            </Button>
            <Button variant="primary" onClick={onClose}>
              Done
            </Button>
          </>
        }
      >
        <p className="apps-signin-url t-mono t-small t-3">{new URL(signIn.url).host}</p>
        <p className="apps-wait" aria-live="polite">
          <Spinner size={14} label="" /> <span className="t-2">Waiting for you to sign in. This moves on by itself.</span>
        </p>
      </Dialog>
    );
  }

  return (
    <Dialog
      open
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title={`Connect ${e.title}`}
      description={<span data-user-content>{e.description}</span>}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={formId} loading={busy} disabled={!online || !option}>
            {option?.kind === "sign_in" ? "Connect and sign in" : `Connect ${e.title}`}
          </Button>
        </>
      }
    >
      <form id={formId} className="apps-form" onSubmit={submit} noValidate>
        <div className="apps-form__who">
          <AppIcon title={e.title} icon={e.icon} size="lg" />
          <span className="apps-form__who-text">
            <span className="apps-form__who-title" data-user-content>
              {e.title}
            </span>
            {e.website && /^https:\/\//.test(e.website) && (
              <span className="t-small t-3 t-mono" data-user-content>
                {new URL(e.website).host}
              </span>
            )}
          </span>
        </div>

        <AboutApp entry={e} onOpen={onOpen} />

        {e.connected.length > 0 && !error && (
          <Notice tone="info" className="apps-form__notice">
            Already connected as <span className="t-mono">{e.connected.join(", ")}</span>. Connecting again adds a second one, with a name of its own.
          </Notice>
        )}

        {e.options.length > 1 ? (
          <RadioGroup
            variant="card"
            label="How Ghost connects"
            value={option?.id ?? null}
            onChange={(id) => {
              setOptionId(id);
              setMissing(new Set());
              setError(null);
            }}
            options={e.options.map((o) => ({ value: o.id, label: <span data-user-content>{o.label}</span>, description: optionHelp(o.kind) }))}
          />
        ) : option ? (
          <p className="t-body t-2 apps-form__how">{optionHelp(option.kind)}</p>
        ) : (
          <Notice tone="warn">The catalog lists no way to connect {e.title}. Add it yourself by its address instead.</Notice>
        )}

        {option?.settings.map((s) => (
          <Field
            key={`${option.id}:${s.key}`}
            label={<span data-user-content>{s.label}</span>}
            detail={s.key}
            optional={!s.required}
            help={s.secret ? "A password: kept in Ghost’s vault, and never shown again." : undefined}
            error={missing.has(s.key) ? `${s.label} is needed to connect.` : undefined}
          >
            {s.choices.length > 0 ? (
              <Select value={value(s.key) || s.choices[0]} options={s.choices.map((c) => ({ value: c, label: c }))} onChange={(ev) => set(s.key, ev.target.value)} />
            ) : (
              <Input
                type={s.secret ? "password" : "text"}
                autoComplete="off"
                spellCheck={false}
                mono={!s.secret}
                value={value(s.key)}
                placeholder={s.secret ? "Paste it here" : s.default || "Type it here"}
                onChange={(ev) => set(s.key, ev.target.value)}
              />
            )}
          </Field>
        ))}

        {option?.kind === "local" && <p className="t-small t-3">Ghost needs the tool it runs (npx, uvx or docker) installed on this computer.</p>}

        <Disclosure summary="Advanced" defaultOpen={!!error?.taken}>
          <Field label="Name in Ghost" detail={`${name.trim() || e.suggested_name}.*`} help="Rules and the audit log call the app by this name. Lower-case letters and dashes.">
            <Input mono value={name} placeholder={e.suggested_name} onChange={(ev) => setName(ev.target.value.toLowerCase())} />
          </Field>
        </Disclosure>

        {error && (
          <Notice
            tone="danger"
            title={error.title}
            className="apps-form__notice"
            action={
              error.taken || error.failed ? (
                <Button size="sm" variant="secondary" loading={busy} onClick={() => void connect(true)}>
                  {error.failed ? "Try again" : "Replace it"}
                </Button>
              ) : undefined
            }
          >
            {error.text} {error.taken ? "Pick another name under Advanced, or replace the one you have." : ""}
          </Notice>
        )}
        {/* Enter in a field submits; the visible Connect button sits in the dialog's footer. */}
        <button type="submit" hidden aria-hidden tabIndex={-1} />
      </form>
    </Dialog>
  );
}

/** The flow's last pane: connected, and what it lets Ghost do, in plain words. */
export function Connected({ app: a, onClose, onOpen }: { app: AppView; onClose: () => void; onOpen?: (name: string) => void }) {
  const ab = abilityOf(a.actions);
  return (
    <Dialog
      open
      onClose={onClose}
      size="md"
      title={`${a.title} is connected`}
      description="Every agent can use it from its next turn, under your permissions."
      footer={
        <>
          <Button variant="ghost" onClick={() => (onOpen ? onOpen(a.name) : navigate(`/apps/${a.name}`))}>
            Its permissions and limits
          </Button>
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        </>
      }
    >
      <div className="apps-can apps-can--dialog">
        <p className="apps-can__lead">
          <Check size={16} aria-hidden className="t-ok" /> Ghost can look up {count(ab.reads.length, "thing")} and change {count(ab.changes.length, "thing")} in {a.title}.
        </p>
        {ab.readLine && <p className="apps-can__line">{ab.readLine}</p>}
        {ab.changeLine && <p className="apps-can__line">{ab.changeLine}</p>}
        <p className="apps-can__line apps-can__rules">{ab.changes.length ? "Each change runs and Ghost tells you, unless your permissions say to ask first or block it." : "It only looks things up; nothing it does changes anything."}</p>
      </div>
    </Dialog>
  );
}
