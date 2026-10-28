/**
 * Owner: settings. The Model page (ADR 0037): which provider Ghost thinks with, picked like an
 * app, then its key, its model from the provider's own list, and Test connection. The address,
 * extra headers, the timeout and the Password's name are folded away under Advanced.
 *
 * The settings are part of the screen's one draft and save with its save bar through
 * PUT /settings. The key is a password in Ghost's vault (ADR 0036): its row saves on its own,
 * as on the Passwords page, and OpenRouter's sign-in keeps the key it returns there, never in
 * this window. A setup from the older settings (GHOST_LLM_ENDPOINT) shows as the provider it
 * resolves to, and is written out as that provider on its first edit.
 */
import type { ModelConnectionView, ModelDraft, ModelTestResult, PasswordInfo, PasswordsView, SettingsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { type ReactNode, useEffect, useRef, useState } from "react";

import { Badge, Button, Card, Disclosure, errorText, Field, Input, Notice, RadioGroup, Section, SegmentedControl, useConfirm, useToast } from "../../ui";
import { openSignIn } from "../apps/signin";
import {
  CONNECTION_KEYS,
  choiceOf,
  msWords,
  OPENAI_COMPATIBLE,
  type ProviderChoice,
  sectionDomId,
  type Values,
} from "./model";
import { Row } from "./parts";
import { PasswordRow, passwordTitle } from "./Passwords";

const CHOICES: { value: ProviderChoice; label: string; description: string }[] = [
  { value: "anthropic", label: "Anthropic", description: "Claude, with an API key from the Anthropic Console." },
  { value: "openrouter", label: "OpenRouter", description: "Sign in, then Claude or any other model OpenRouter offers." },
  { value: "cliproxy", label: "Claude subscription", description: "Your Claude plan, through CLIProxy running on your network." },
  { value: "openai-compatible", label: "OpenAI or a local model", description: "OpenAI with an API key, or Ollama or LM Studio on this computer." },
  { value: "custom", label: "Your own address", description: "Any address that speaks Anthropic’s or OpenAI’s API." },
];
const OPENAI_COMPATIBLE_CHOICES = [
  { value: "openai", label: "OpenAI" },
  { value: "ollama", label: "Ollama" },
  { value: "lmstudio", label: "LM Studio" },
];
const PROTOCOLS = [
  { value: "anthropic", label: "Anthropic" },
  { value: "openai", label: "OpenAI" },
];
/** The page's form, rendered by the settings screen: the fields name it so Enter saves, while
 *  the key's row stays a form of its own (forms can't nest). */
export const MODEL_FORM_ID = "set-model-form";
const SIGN_IN_POLL_MS = 1500;
const SIGN_IN_POLLS = 400; // ten minutes, as long as OpenRouter's code lasts

export interface ModelSectionProps {
  s: SettingsView;
  conn: ModelConnectionView | null;
  values: Values;
  online: boolean;
  mock: boolean;
  setValues: (patch: Record<string, string>) => void;
  help: (k: string, text: ReactNode) => ReactNode;
  err: (k: string) => ReactNode;
  fieldId: (k: string) => string;
  /** Names saved but not yet in use (a restart owed), as the Passwords page shows them. */
  waiting: Set<string>;
  /** A password changed (saved here, or by OpenRouter's sign-in): read the settings and the
   *  connection again; `restart` names one Ghost reads only at its next start. */
  onPassword: (restart: string[]) => void;
}

export function ModelSection({ s, conn, values, online, mock, setValues, help, err, fieldId, waiting, onPassword }: ModelSectionProps) {
  const { client } = useGhost();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();

  // The draft as a connection: explicit once it names a provider, else the older settings it
  // resolves to (what GET /model reports), with any edits on top.
  const explicit = !!(values.GHOST_LLM_PROVIDER ?? "").trim();
  const eff: Values = explicit ? values : { ...(conn?.values ?? {}), ...nonEmpty(values) };
  const provider = (eff.GHOST_LLM_PROVIDER || conn?.provider || "anthropic").trim();
  const spec = conn?.providers.find((p) => p.id === provider);
  const protocol = provider === "custom" ? eff.GHOST_LLM_PROTOCOL || "anthropic" : spec?.protocol ?? "anthropic";
  const password = (eff.GHOST_LLM_PASSWORD || (!explicit ? conn?.password : spec?.password) || "").trim();
  const keyNeeded = !!password && !(provider === "ollama" || provider === "lmstudio");

  /* --------------------------------------------- its password, in Ghost's vault (ADR 0036) */
  const [vault, setVault] = useState<PasswordsView | null>(null);
  const [keyOpen, setKeyOpen] = useState(false);
  const readVault = () => {
    client.passwords().then(setVault, () => {});
  };
  useEffect(readVault, [client]);
  const row: PasswordInfo | null = !password
    ? null
    : vault?.passwords.find((p) => p.name === password) ?? {
        name: password,
        set: false,
        ghost: true,
        used_by: ["The model connection"],
        added: null,
        updated: null,
        last_used: null,
      };
  const vaultClosed = !!vault && vault.vault.state !== "open";
  const keySet = !!row?.set || (password === conn?.password && !!conn?.password_set);
  const keySaved = (v: PasswordsView, restart: boolean, names: string[]) => {
    setVault(v);
    onPassword(restart ? names : []);
  };
  const forgetKey = async (p: PasswordInfo) => {
    const title = passwordTitle(p);
    if (!(await confirm({ title: `Delete the ${title} password?`, description: "Ghost can’t reach the model with it any more until you add it again.", confirmLabel: `Delete ${title}`, tone: "danger" }))) return;
    try {
      const r = await client.deletePassword(p.name);
      keySaved(r.passwords, r.restart_required, [p.name]);
      toast({ title: "Deleted", description: `${title} is gone from the vault.` });
    } catch (e) {
      toast({ title: "Couldn’t delete it", description: errorText(e), tone: "danger" });
    }
  };

  /** An edit: from the older settings, the provider they resolve to is written out first. */
  const edit = (patch: Record<string, string>) => {
    const base: Record<string, string> = {};
    if (!explicit) for (const k of CONNECTION_KEYS) base[k] = conn?.values[k] ?? "";
    setValues({ ...base, ...patch });
  };
  const savedProvider = (s.values.GHOST_LLM_PROVIDER || conn?.provider || "").trim();
  const pick = (id: string) => {
    if (id === provider) return;
    if (id === savedProvider) {
      // Back to what's saved: nothing to save.
      setValues(Object.fromEntries(CONNECTION_KEYS.map((k) => [k, s.values[k] ?? ""])));
      return;
    }
    setValues({
      ...Object.fromEntries(CONNECTION_KEYS.map((k) => [k, ""])),
      GHOST_LLM_PROVIDER: id,
      ...(id === "custom" ? { GHOST_LLM_PROTOCOL: "anthropic" } : {}),
    });
  };
  const pickChoice = (c: ProviderChoice) => pick(c === "openai-compatible" ? ((OPENAI_COMPATIBLE as readonly string[]).includes(provider) ? provider : "openai") : c);

  /** What the daemon resolves the list and the test from: the draft over what's saved. */
  const draftRef = useRef<ModelDraft>({ values: {} });
  draftRef.current = {
    values: Object.fromEntries(CONNECTION_KEYS.map((k) => [k, (values[k] ?? "").trim() || null])),
  };

  /* -------------------------------------------------------------- the model list */
  const listKey = JSON.stringify([provider, protocol, eff.GHOST_LLM_BASE_URL ?? "", password, keySet, row?.updated ?? "", eff.GHOST_LLM_HEADERS ?? ""]);
  const [list, setList] = useState<{ key: string; models: { id: string; name?: string | null }[]; error: string | null; fallback: string | null } | null>(null);
  const [listing, setListing] = useState(false);
  useEffect(() => {
    if (!online) return;
    let live = true;
    setListing(true);
    const t = window.setTimeout(() => {
      client
        .modelList(draftRef.current)
        .then((r) => live && setList({ key: listKey, models: r.models, error: r.error ?? null, fallback: r.default ?? null }))
        .catch((e) => live && setList({ key: listKey, models: [], error: errorText(e), fallback: null }))
        .finally(() => live && setListing(false));
    }, 350);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [client, listKey, online]);
  const models = list?.key === listKey ? list : null;
  const defaultModel = spec?.default_model ?? (protocol === "anthropic" ? "claude-sonnet-5-5" : models?.fallback ?? "");
  const listId = "set-model-list";

  /* ---------------------------------------------------------- test connection */
  const testKey = JSON.stringify([listKey, eff.GHOST_LLM_MODEL ?? "", eff.GHOST_LLM_TIMEOUT_SECS ?? ""]);
  const [test, setTest] = useState<{ key: string; r?: ModelTestResult; error?: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const runTest = async () => {
    setTesting(true);
    try {
      setTest({ key: testKey, r: await client.testModel(draftRef.current) });
    } catch (e) {
      setTest({ key: testKey, error: errorText(e) });
    } finally {
      setTesting(false);
    }
  };
  const shownTest = test?.key === testKey ? test : null;

  /* ---------------------------------------------------------- OpenRouter's sign-in */
  const [signing, setSigning] = useState(false);
  const [signUrl, setSignUrl] = useState<string | null>(null);
  const [signError, setSignError] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(
    () => () => {
      alive.current = false;
    },
    [],
  );
  const signIn = async () => {
    setSigning(true);
    setSignError(null);
    setSignUrl(null);
    try {
      const r = await client.modelSignIn();
      const opened = await openSignIn(r.url, { again: true, mock });
      if (!opened) setSignUrl(r.url);
      for (let i = 0; i < SIGN_IN_POLLS && alive.current; i++) {
        await new Promise((ok) => window.setTimeout(ok, SIGN_IN_POLL_MS));
        const c = await client.modelConnection();
        if (c.sign_in_pending) continue;
        if (c.sign_in_error) setSignError(c.sign_in_error);
        else toast({ title: "Signed in to OpenRouter", description: "Ghost keeps the key as a password in its vault. It never comes back to this window.", tone: "ok" });
        readVault();
        onPassword(c.sign_in_error ? [] : ["OPENROUTER_API_KEY"]);
        break;
      }
    } catch (e) {
      setSignError(errorText(e));
    } finally {
      if (alive.current) {
        setSigning(false);
        setSignUrl(null);
      }
    }
  };

  /* -------------------------------------------------------------------- render */
  const editing = CONNECTION_KEYS.some((k) => (values[k] ?? "") !== (s.values[k] ?? "")) || keyOpen;
  const field = (k: string) => fieldId(k);
  const baseHere = provider === "cliproxy" || provider === "custom";
  const baseField = (
    <Field
      layout="inline"
      id={field("GHOST_LLM_BASE_URL")}
      label="Address"
      detail="GHOST_LLM_BASE_URL"
      optional={!baseHere || provider === "cliproxy"}
      help={help("GHOST_LLM_BASE_URL", baseHere && provider === "custom" ? (protocol === "openai" ? "Up to /v1, as OpenAI’s clients take it." : "Where /v1/messages answers.") : `Empty means ${spec?.base_url || "the provider’s own"}.`)}
      error={err("GHOST_LLM_BASE_URL")}
    >
      <Input
        form={MODEL_FORM_ID}
        value={eff.GHOST_LLM_BASE_URL ?? ""}
        placeholder={spec?.base_url || (protocol === "openai" ? "http://gpu-box:8000/v1" : "https://llm.example.com")}
        inputMode="url"
        spellCheck={false}
        onChange={(e) => edit({ GHOST_LLM_BASE_URL: e.target.value })}
      />
    </Field>
  );

  return (
    <>
      <Section id={sectionDomId("model")} className="set-section">
        {dialog}
        {conn?.legacy && !explicit && (
          <p className="set-help is-readable set-model-legacy">
            From your older settings (<span className="set-detail">{conn.legacy}</span>). Any change here saves it as the provider below.
          </p>
        )}
        <RadioGroup<ProviderChoice> variant="card" aria-label="Provider" value={choiceOf(provider)} onChange={pickChoice} options={CHOICES} disabled={!online} />
      </Section>

      <Section title={spec?.title ?? "Connection"} className="set-section">
        {conn && !conn.ready && !editing && (
          <Notice tone="warn" title="Ghost can’t reach a model yet">
            {sentence(conn.problems[0] ?? "")}
          </Notice>
        )}
        <Card list>
          {choiceOf(provider) === "openai-compatible" && (
            <Row>
              <Field layout="inline" label="Provider" help="OpenAI’s own, or a model on this computer." controlWidth="auto">
                <SegmentedControl value={provider} onChange={pick} options={OPENAI_COMPATIBLE_CHOICES} disabled={!online} />
              </Field>
            </Row>
          )}
          {provider === "openrouter" ? (
            <Row>
              <Field
                layout="inline"
                label="OpenRouter account"
                detail={password}
                controlWidth="auto"
                help={
                  signing ? (
                    signUrl ? (
                      <>Open this address to sign in: <span className="set-detail">{signUrl}</span></>
                    ) : (
                      "Waiting for you to sign in in your browser…"
                    )
                  ) : keySet ? (
                    "Signed in. Ghost keeps the key OpenRouter gave it as a password in its vault."
                  ) : vaultClosed ? (
                    "Unlock Ghost’s passwords on the Passwords page first, so it can keep the key."
                  ) : (
                    "Sign in and Ghost gets its own key. There’s nothing to copy."
                  )
                }
                error={signError ?? undefined}
                aside={keySet ? <Badge tone="ok" size="sm" dot>Signed in</Badge> : <Badge size="sm">Not signed in</Badge>}
              >
                <Button size="sm" variant={keySet ? "ghost" : "secondary"} loading={signing} disabled={!online || vaultClosed} onClick={() => void signIn()}>
                  {keySet ? "Sign in again…" : "Sign in…"}
                </Button>
              </Field>
            </Row>
          ) : (
            keyNeeded &&
            row && (
              <div className="set-model-key">
              <PasswordRow
                p={row}
                open={keyOpen}
                onOpen={() => setKeyOpen(true)}
                onClose={() => setKeyOpen(false)}
                onSaved={keySaved}
                onDelete={(p) => void forgetKey(p)}
                waiting={waiting.has(row.name)}
                disabled={!online || vaultClosed}
              />
              </div>
            )
          )}
          {baseHere && <Row>{baseField}</Row>}
          {provider === "custom" && (
            <Row>
              <Field layout="inline" label="Speaks" detail="GHOST_LLM_PROTOCOL" help="The API your address answers with." controlWidth="auto">
                <SegmentedControl value={protocol} onChange={(p) => edit({ GHOST_LLM_PROTOCOL: p })} options={PROTOCOLS} disabled={!online} />
              </Field>
            </Row>
          )}
          <Row>
            <Field
              layout="inline"
              id={field("GHOST_LLM_MODEL")}
              label="Model"
              detail="GHOST_LLM_MODEL"
              help={help("GHOST_LLM_MODEL", modelHelp(models, listing, defaultModel, spec?.title ?? "the provider"))}
              error={err("GHOST_LLM_MODEL")}
            >
              <Input
                form={MODEL_FORM_ID}
                mono
                list={listId}
                value={eff.GHOST_LLM_MODEL ?? ""}
                placeholder={defaultModel || "Pick from the list"}
                spellCheck={false}
                autoComplete="off"
                onChange={(e) => edit({ GHOST_LLM_MODEL: e.target.value })}
              />
            </Field>
            <datalist id={listId}>
              {(models?.models ?? []).map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name ?? undefined}
                </option>
              ))}
            </datalist>
          </Row>
          <Row className="set-model-test">
            <div className="set-model-test__main">
              <p className="set-label">Time to first word</p>
              <p className="set-help" aria-live="polite">
                {shownTest ? <TestWords t={shownTest} /> : "Sends one short message and times the first word back, the way Ghost’s latency check does."}
              </p>
            </div>
            <Button size="sm" variant="secondary" loading={testing} disabled={!online} onClick={() => void runTest()}>
              Test connection
            </Button>
          </Row>
        </Card>
      </Section>

      <Section className="set-section">
        <Disclosure summary="Advanced" defaultOpen={["GHOST_LLM_HEADERS", "GHOST_LLM_TIMEOUT_SECS", "GHOST_LLM_PASSWORD", ...(baseHere ? [] : ["GHOST_LLM_BASE_URL"])].some((k) => !!err(k) || !!(eff[k] ?? "").trim())}>
          <Card list>
            {!baseHere && <Row>{baseField}</Row>}
            <Row>
              <Field
                layout="inline"
                id={field("GHOST_LLM_PASSWORD")}
                label="Password name"
                detail="GHOST_LLM_PASSWORD"
                optional
                help={help("GHOST_LLM_PASSWORD", `The password that holds the key. Empty means ${spec?.password || "none"}.`)}
                error={err("GHOST_LLM_PASSWORD")}
              >
                <Input form={MODEL_FORM_ID} mono value={eff.GHOST_LLM_PASSWORD ?? ""} placeholder={spec?.password || "MY_MODEL_KEY"} spellCheck={false} onChange={(e) => edit({ GHOST_LLM_PASSWORD: e.target.value.toUpperCase() })} />
              </Field>
            </Row>
            <Row>
              <Field
                layout="inline"
                id={field("GHOST_LLM_HEADERS")}
                label="Extra headers"
                detail="GHOST_LLM_HEADERS"
                optional
                help={help("GHOST_LLM_HEADERS", "Sent with every call, as written. Name: value, separated by semicolons.")}
                error={err("GHOST_LLM_HEADERS")}
              >
                <Input form={MODEL_FORM_ID} mono value={eff.GHOST_LLM_HEADERS ?? ""} placeholder="X-Title: Ghost" spellCheck={false} onChange={(e) => edit({ GHOST_LLM_HEADERS: e.target.value })} />
              </Field>
            </Row>
            <Row>
              <Field
                layout="inline"
                id={field("GHOST_LLM_TIMEOUT_SECS")}
                label="Timeout"
                detail="GHOST_LLM_TIMEOUT_SECS"
                optional
                help={help("GHOST_LLM_TIMEOUT_SECS", "Seconds a call may take before it fails. Empty means the client’s own.")}
                error={err("GHOST_LLM_TIMEOUT_SECS")}
                controlWidth={120}
              >
                <Input form={MODEL_FORM_ID} value={eff.GHOST_LLM_TIMEOUT_SECS ?? ""} placeholder="60" inputMode="decimal" onChange={(e) => edit({ GHOST_LLM_TIMEOUT_SECS: e.target.value })} />
              </Field>
            </Row>
          </Card>
        </Disclosure>
      </Section>
    </>
  );
}

function TestWords({ t }: { t: { r?: ModelTestResult; error?: string } }) {
  if (t.error) return <span className="t-danger">{t.error}</span>;
  const r = t.r!;
  if (!r.ok || r.ttft_ms == null) return <span className="t-danger">{sentence(r.error ?? "No answer came back.")}</span>;
  return (
    <>
      <span className="set-live">First word in {msWords(r.ttft_ms)}</span>
      {r.total_ms != null && <>, the whole answer in {msWords(r.total_ms)}</>}. <span className="set-detail">{r.answered_by || r.model}</span>
    </>
  );
}

function modelHelp(models: { models: unknown[]; error: string | null } | null, listing: boolean, fallback: string, title: string): string {
  const empty = fallback ? ` Empty means ${fallback}.` : "";
  if (!models) return listing ? `Reading ${title}’s models…` : `Type a model id.${empty}`;
  if (models.error) return `${sentence(models.error)} Type a model id instead.${empty}`;
  const n = models.models.length;
  return n ? `${n} model${n === 1 ? "" : "s"} from ${title}; start typing to pick one.${empty}` : `${title} listed no models.${empty}`;
}

/** "couldn’t reach it" → "Couldn’t reach it." */
function sentence(t: string): string {
  const s = t.trim();
  if (!s) return s;
  return s.charAt(0).toUpperCase() + s.slice(1) + (/[.!?]$/.test(s) ? "" : ".");
}

function nonEmpty(v: Values): Values {
  return Object.fromEntries(Object.entries(v).filter(([, x]) => (x ?? "").trim() !== ""));
}
