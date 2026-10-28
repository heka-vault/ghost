/**
 * Owner: policy. Permissions (ADR 0034): Ghost uses every app it is connected to, and this
 * screen holds the exceptions, in `[permissions]` of ~/.config/ghost/policy.toml. The mode says
 * what a change no rule names does (Open, Ask before changes, Read only: the lockdown); the
 * lists say what is blocked, asks first, runs quietly or is allowed anyway. The most careful
 * rule that matches wins. Ghost's own operations (agents, rules, skills, outputs) are
 * named by the same rules. The model never sees this screen or the file.
 *
 * Rules apply the moment they are set (`PUT /permissions`); the TOML view edits the whole file
 * (`PUT /policy` with text) and saves through the dock, keeping the operator's comments.
 *   #/settings/permissions                 the rules
 *   ?policy=toml#/settings/permissions     the file (works anywhere)
 *   mock only: ?policy=dirty|invalid|load-error
 */
import { ApiError, type AppView, type PermissionsView, type PolicyView, type SourceInfo } from "@ghost/client-ts";
import { permissionLabel, permissionOf, useGhost } from "@ghost/ui";
import { Code, Hand, Megaphone, ShieldOff, SlidersHorizontal, X } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { Link } from "../../app/router";
import { useDesign } from "../../design/context";
import {
  Badge,
  Button,
  Card,
  Disclosure,
  Field,
  IconButton,
  Input,
  Notice,
  Page,
  RadioGroup,
  SegmentedControl,
  Section,
  Select,
  Skeleton,
  Table,
  errorText,
  useConfirm,
  useLive,
  useToast,
} from "../../ui";
import { type BarState, SaveBar, useCopy } from "./Bar";
import { useLeaveGuard } from "./guard";
import { LISTS, type Level, MODES, type Mode, OWN, errorLine, ruleApp, ruleWords, sentence } from "./model";
import { TomlEditor } from "./TomlEditor";
import "./policy.css";

type View = "rules" | "toml";

/** Design states, read from the query before the hash. `toml` works anywhere; the rest only under the mock. */
type DesignState = "toml" | "dirty" | "invalid" | "load-error" | null;
function readDesignState(mock: boolean): DesignState {
  const v = new URLSearchParams(window.location.search).get("policy");
  if (v === "toml") return "toml";
  if (!mock) return null;
  return v === "dirty" || v === "invalid" || v === "load-error" ? v : null;
}

/** The same, without a leading “Line 4:” where the line is already named. */
const lineless = (s: string) => sentence(s.replace(/^(request:\s*)?line \d+:\s*/i, ""));
const DEFAULT_PATH = "~/.config/ghost/policy.toml";
const MODE_ICON: Record<Mode, typeof Hand> = { open: Megaphone, ask: Hand, "read-only": ShieldOff };

export default function PermissionsScreen(_: RouteProps) {
  const { client, appsVersion } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const design = useDesign();
  const [designState] = useState(() => readDesignState(!!design));

  const [policy, setPolicy] = useState<PolicyView | null>(null);
  const [perms, setPerms] = useState<PermissionsView | null>(null);
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  const [apps, setApps] = useState<AppView[] | null>(null);
  /** The file as the operator is editing it in TOML. */
  const [text, setText] = useState("");
  const [view, setView] = useState<View>(designState === "toml" || designState === "invalid" ? "toml" : "rules");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<{ message: string; line: number | null } | null>(null);
  /** The rule (or "mode") being set, so only its control waits. */
  const [setting, setSetting] = useState<string | null>(null);
  const [ruleError, setRuleError] = useState<string | null>(null);
  const [showLine, setShowLine] = useState(0);
  const editor = useRef<HTMLTextAreaElement>(null);

  const dirty = !!policy && text !== policy.text;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;

  const adopt = useCallback((p: PolicyView) => {
    setPolicy(p);
    setPerms(p.permissions);
    setText(p.text);
  }, []);

  const loadSeq = useRef(0);
  const load = useCallback(async () => {
    const seq = ++loadSeq.current;
    setLoadError(null);
    try {
      if (import.meta.env.DEV && designState === "load-error") {
        await new Promise((r) => setTimeout(r, 200));
        throw new ApiError(422, `${DEFAULT_PATH}: Invalid value (at line 4, column 9)`);
      }
      const [p, s, a] = await Promise.all([client.policy(), client.sources(), client.apps().catch(() => null)]);
      if (seq !== loadSeq.current) return null;
      adopt(p);
      setSources(s);
      setApps(a);
      return p;
    } catch (e) {
      if (seq === loadSeq.current) setLoadError(errorText(e));
      return null;
    }
  }, [client, adopt, designState]);

  useEffect(() => {
    let live = true;
    void load().then((p) => {
      if (!live || !p || !import.meta.env.DEV) return;
      if (designState === "dirty") setText(p.text.replace(/^mode = "open"$/m, 'mode = "ask"'));
      else if (designState === "invalid") {
        const broken = p.text.replace(/^mode = "open"$/m, "mode = open");
        setText(broken);
        const line = broken.split("\n").findIndex((l) => l === "mode = open") + 1;
        if (line > 0) {
          setSaveError({ message: `Invalid value (at line ${line}, column 8)`, line });
          setShowLine((n) => n + 1);
        }
      }
    });
    return () => {
      live = false;
    };
  }, [load, designState]);

  // Another window or the voice changed the rules: read them again, unless the file is being edited here.
  const firstApps = useRef(true);
  useEffect(() => {
    if (firstApps.current) {
      firstApps.current = false;
      return;
    }
    if (dirtyRef.current) return;
    client
      .policy()
      .then((p) => !dirtyRef.current && adopt(p))
      .catch(() => {});
    client.sources().then(setSources).catch(() => {});
    client.apps().then(setApps).catch(() => {});
  }, [appsVersion, client, adopt]);

  // ---------------------------------------------------------------------- rules
  /** One change to the permissions, then the file and Ghost's own levels read back. */
  const change = async (key: string, update: Parameters<typeof client.updatePermissions>[0], opts: { what: string; undo?: Parameters<typeof client.updatePermissions>[0]; onError?: (message: string) => void }) => {
    if (!online || setting) return false;
    setSetting(key);
    try {
      const p = await client.updatePermissions(update);
      setPerms(p);
      client.policy().then((v) => !dirtyRef.current && adopt(v)).catch(() => {});
      client.sources().then(setSources).catch(() => {});
      toast({
        tone: "ok",
        title: "Saved",
        description: `${opts.what} Ghost follows it from its next turn.`,
        action: opts.undo ? { label: "Undo", onClick: () => void change(key, opts.undo!, { what: "Put back as it was." }) } : undefined,
      });
      return true;
    } catch (e) {
      const message = sentence(errorText(e));
      if (opts.onError) opts.onError(message);
      else setRuleError(message);
      return false;
    } finally {
      setSetting(null);
    }
  };

  const listOf = (rule: string): Level | null => (perms ? (LISTS.find((l) => perms[l.list].includes(rule))?.list ?? null) : null);

  const setMode = (mode: Mode) => {
    if (!perms || mode === perms.mode) return;
    const label = MODES.find((m) => m.value === mode)!.label;
    void change("mode", { mode }, { what: `Mode: ${label.toLowerCase()}.`, undo: { mode: perms.mode } });
  };

  const removeRule = (rule: string) => {
    const was = listOf(rule);
    void change(rule, { set: { rule, level: null } }, { what: `Removed ${rule}.`, undo: was ? { set: { rule, level: was } } : undefined });
  };

  const titleOf = useCallback(
    (app: string) => apps?.find((a) => a.name === app)?.title ?? OWN[app] ?? app,
    [apps],
  );
  /** A rule that names no app Ghost has, and none of Ghost's own: it waits for one. */
  const waiting = useCallback(
    (rule: string) => {
      const app = ruleApp(rule);
      return !!app && !!apps && !OWN[app] && !apps.some((a) => a.name === app);
    },
    [apps],
  );

  // ----------------------------------------------------------------------- file
  const discard = () => {
    if (policy) setText(policy.text);
    setSaveError(null);
  };

  const switchView = async (next: View) => {
    if (next === view) return;
    if (next === "rules" && dirty) {
      const ok = await confirm({
        title: "Discard your edits to the file?",
        description: "Rules change the file the moment you set them, so unsaved edits can’t wait in the background. Save them first, or discard them.",
        confirmLabel: "Discard edits",
        tone: "danger",
      });
      if (!ok) return;
      discard();
    }
    setView(next);
  };

  const putBack = async (previous: string) => {
    try {
      adopt((await client.updatePolicy({ text: previous })).policy);
      setSaveError(null);
      toast({ title: "Put back the file as it was" });
    } catch (e) {
      setSaveError({ message: errorText(e), line: null });
    }
  };

  const save = async () => {
    if (!dirty || saving || !online || !policy) return;
    setSaving(true);
    setSaveError(null);
    const before = policy;
    try {
      const r = await client.updatePolicy({ text });
      adopt(r.policy);
      client.sources().then(setSources).catch(() => {});
      toast({
        tone: "ok",
        title: "Saved",
        description: "Ghost follows the new permissions from its next turn.",
        action: before.exists ? { label: "Undo", onClick: () => void putBack(before.text) } : undefined,
      });
    } catch (e) {
      const message = errorText(e);
      const line = errorLine(message);
      setSaveError({ message, line });
      if (line != null) setShowLine((n) => n + 1);
    } finally {
      setSaving(false);
    }
  };

  // Ctrl S saves the file, as everywhere else on Linux.
  const saveRef = useRef(save);
  saveRef.current = save;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void saveRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // A failed save takes the operator to the line at fault: selected, centered in the panel.
  useEffect(() => {
    if (!showLine || view !== "toml" || !saveError?.line) return;
    const id = requestAnimationFrame(() => {
      const ta = editor.current;
      const main = document.getElementById("main");
      if (!ta || !main) return;
      const lines = ta.value.split("\n");
      const n = Math.min(saveError.line!, lines.length);
      const start = lines.slice(0, n - 1).reduce((s, l) => s + l.length + 1, 0);
      ta.focus({ preventScroll: true });
      ta.setSelectionRange(start, start + (lines[n - 1]?.length ?? 0));
      const band = ta.parentElement?.querySelector(".policy-toml__band") ?? ta;
      const r = band.getBoundingClientRect();
      const m = main.getBoundingClientRect();
      const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
      main.scrollBy({ top: r.top - m.top - main.clientHeight / 2 + r.height / 2, behavior: reduce ? "auto" : "smooth" });
    });
    return () => cancelAnimationFrame(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showLine, view]);

  useLeaveGuard(dirty && !saving, "/settings/permissions", () =>
    confirm({ title: "Leave without saving?", description: "Your edits to the policy file aren’t saved. Leaving discards them.", confirmLabel: "Discard changes", tone: "danger" }),
  );

  // ---------------------------------------------------------------------- views
  const errorId = useId();
  const helpId = useId();

  const rules = perms && (
    <>
      {policy && !policy.exists && (
        <p className="policy-lead-note t-body-lg t-2">
          No policy file yet, so everything runs and Ghost says what it changed. Setting a rule creates <span className="t-mono">{policy.path}</span>.
        </p>
      )}
      {ruleError && (
        <Notice tone="danger" title="Couldn’t change the permissions" onDismiss={() => setRuleError(null)} className="policy-notice">
          {ruleError}
        </Notice>
      )}

      <Section id="perm-mode" title="When no rule says otherwise" description="What Ghost does with a change no rule below names. Reads always run unless blocked.">
        <RadioGroup<Mode>
          variant="card"
          columns={3}
          aria-label="Mode"
          value={perms.mode}
          disabled={!online || setting === "mode"}
          onChange={setMode}
          options={MODES.map((m) => {
            const Icon = MODE_ICON[m.value];
            return { value: m.value, label: m.label, description: m.help, icon: <Icon size={16} /> };
          })}
        />
      </Section>

      <Section id="perm-add" title="Add a rule" description="A rule names an app, a tool, or a tool with what it’s called with. The most careful rule that matches wins: block, then ask first, then quiet, then allow.">
        <AddRule disabled={!online} busy={!!setting && setting.startsWith("add:")} existing={(r) => listOf(r)} onAdd={(rule, list, onError) => change(`add:${rule}`, { set: { rule, level: list } }, { what: `${rule}: ${permissionLabel(list).toLowerCase()}.`, undo: { set: { rule, level: listOf(rule) } }, onError })} />
      </Section>

      {LISTS.filter((l) => l.list !== "allow" || perms.mode !== "open" || perms.allow.length > 0).map((l) => (
        <Section
          key={l.list}
          id={`perm-${l.list}`}
          title={
            <>
              {l.title} <span className="policy-count t-num">{perms[l.list].length}</span>
            </>
          }
          description={l.list === "allow" && perms.mode === "open" ? "In Open every change runs already. These matter once the mode asks or blocks." : l.help}
        >
          <Card list>
            {perms[l.list].length === 0 ? (
              <p className="policy-empty">{l.empty}</p>
            ) : (
              perms[l.list].map((rule) => (
                <div key={rule} className="policy-rule">
                  <div className="policy-rule__text">
                    <span className="policy-rule__code">{rule}</span>
                    <span className="policy-rule__words">
                      {ruleWords(rule, titleOf)}
                      {waiting(rule) && <span className="t-3"> · no app called {ruleApp(rule)} is connected yet</span>}
                    </span>
                  </div>
                  <IconButton size="sm" icon={<X />} label={`Remove ${rule}`} loading={setting === rule} disabled={!online || (!!setting && setting !== rule)} onClick={() => removeRule(rule)} />
                </div>
              ))
            )}
          </Card>
        </Section>
      ))}

      <OwnOperations sources={sources} />

      <p className="policy-foot t-small t-3">
        These apply to Ghost and every agent alike. Each app’s tools, with these rules applied, are on its page under <Link to="/settings/apps">Apps</Link>.
      </p>
    </>
  );

  const toml = policy && (
    <div className="policy-file">
      <div className="policy-file__head">
        <span className="t-mono t-2">{policy.path}</span>
        <span className="t-small t-3">{dirty ? "Edited" : !policy.exists ? "Not created yet" : "Saved"}</span>
      </div>
      {saveError && (
        <span id={errorId} className="sr-only">
          {saveError.line ? `Line ${saveError.line}: ` : ""}
          {lineless(saveError.message)}
        </span>
      )}
      <TomlEditor
        inputRef={editor}
        value={text}
        onChange={(t) => {
          setText(t);
          if (saveError) setSaveError(null);
        }}
        label="Policy file"
        errorLine={saveError?.line ?? null}
        describedBy={saveError ? `${errorId} ${helpId}` : helpId}
        placeholder={'[permissions]\nmode = "open"\nblock = ["github.delete_*"]'}
      />
      <p id={helpId} className="t-small t-3 policy-file__help">
        Ghost checks the file before writing it; if it doesn’t read cleanly, nothing changes. Comments stay as you write them. Rules set on the Rules view rewrite only <span className="t-mono">[permissions]</span>.
      </p>
    </div>
  );

  let bar: BarState | null = null;
  if (dirty && view === "toml") {
    let title = "Unsaved edits to the file";
    let error: Extract<BarState, { kind: "dirty" }>["error"] = null;
    if (saveError) {
      title = `Couldn’t save${saveError.line ? ` · line ${saveError.line}` : ""}`;
      const plain = lineless(saveError.message);
      const low = plain ? plain[0].toLowerCase() + plain.slice(1).replace(/\.$/, "") : plain;
      error = { text: saveError.line ? low : `${plain} Nothing changed on disk.`, plain: `${plain} Nothing changed on disk.`, line: saveError.line };
    }
    bar = { kind: "dirty", title, summary: "", online, busy: saving, error };
  }

  return (
    <Page
      width="default"
      title="Permissions"
      description="Everything runs: Ghost uses every app you connect and tells you what it changed. This is where you block, or have Ghost ask you first about, anything you want handled differently. Every change is in the audit log; to reverse one, ask Ghost to change it back."
      actions={
        <SegmentedControl<View>
          aria-label="Edit as"
          value={view}
          disabled={!policy}
          onChange={(v) => void switchView(v)}
          options={[
            { value: "rules", label: "Rules", icon: <SlidersHorizontal /> },
            { value: "toml", label: "TOML", icon: <Code /> },
          ]}
        />
      }
      dock={bar && <SaveBar state={bar} onSave={() => void save()} onDiscard={discard} />}
    >
      {loadError ? <LoadError message={loadError} onRetry={() => void load()} /> : !policy ? <PermissionsSkeleton view={view} /> : view === "rules" ? rules : toml}
      {dialog}
    </Page>
  );
}

/* ------------------------------------------------------------------ add a rule */

const EXAMPLES: { rule: string; says: string }[] = [
  { rule: "gmail", says: "everything in an app" },
  { rule: "github.delete_*", says: "tools by name" },
  { rule: "github.issue_write(state=closed)", says: "one kind of call" },
  { rule: "github.*(repo!=site)", says: "everything outside one repository" },
];

function AddRule({ disabled, busy, existing, onAdd }: { disabled: boolean; busy: boolean; existing: (rule: string) => Level | null; onAdd: (rule: string, list: Level, onError: (message: string) => void) => Promise<boolean> }) {
  const [rule, setRule] = useState("");
  const [list, setList] = useState<Level>("block");
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const text = rule.trim();
    if (!text) {
      setError("Write a rule first, like github.delete_*.");
      return;
    }
    if (existing(text) === list) {
      setError(`${text} is already in that list.`);
      return;
    }
    setError(null);
    if (await onAdd(text, list, setError)) setRule("");
  };

  return (
    <form className="policy-add" onSubmit={(e) => void submit(e)} noValidate>
      <div className="policy-add__row">
        <Field label="Rule" error={error ?? undefined} className="policy-add__rule">
          <Input
            mono
            autoComplete="off"
            spellCheck={false}
            placeholder="github.delete_*"
            value={rule}
            onChange={(e) => {
              setRule(e.target.value);
              setError(null);
            }}
          />
        </Field>
        <Field label="What Ghost does" className="policy-add__list">
          <Select
            value={list}
            onChange={(e) => setList(e.target.value as Level)}
            options={LISTS.map((l) => ({ value: l.list, label: permissionLabel(l.list) }))}
          />
        </Field>
        <Button type="submit" variant="primary" className="policy-add__go" loading={busy} disabled={disabled}>
          Add the rule
        </Button>
      </div>
      <ul className="policy-examples" aria-label="Examples">
        {EXAMPLES.map((x) => (
          <li key={x.rule}>
            <button type="button" className="policy-example" onClick={() => setRule(x.rule)}>
              <span className="t-mono">{x.rule}</span>
            </button>
            <span className="t-3"> {x.says}</span>
          </li>
        ))}
      </ul>
    </form>
  );
}

/* --------------------------------------------------- Ghost's own operations */

/** Ghost's own operations, each at the level the rules give it by name. Read-only: rules change them. */
function OwnOperations({ sources }: { sources: SourceInfo[] | null }) {
  const own = useMemo(() => (sources ?? []).filter((s) => s.kind !== "app"), [sources]);
  if (!sources) return <Skeleton width={180} height={14} />;
  if (own.length === 0) return null;
  const total = own.reduce((n, s) => n + s.operations.length, 0);
  return (
    <Disclosure summary={`What Ghost does itself: ${total} operations on your agents, rules and files`} className="policy-own">
      <p className="t-small t-2 policy-own__lead">
        Hiring and retiring agents, rules, memory and what Ghost makes: named by the same rules, such as <span className="t-mono">conversations.delete</span>.
      </p>
      <Table density="compact">
        <thead>
          <tr>
            <th>Operation</th>
            <th>What it does</th>
            <th className="is-shrink">Now</th>
          </tr>
        </thead>
        <tbody>
          {own.flatMap((s) =>
            s.operations.map((op) => {
              const level = permissionOf(op.autonomy);
              return (
                <tr key={`${s.name}.${op.name}`}>
                  <td className="is-mono is-nowrap">
                    {s.name}.{op.name}
                  </td>
                  <td className="is-muted">{op.description.split(/(?<=\.)\s/)[0]}</td>
                  <td className="is-shrink">
                    <Badge size="sm" tone={level === "block" ? "danger" : level === "ask" ? "warn" : "neutral"} variant={level === "allow" ? "outline" : "soft"}>
                      {permissionLabel(level)}
                    </Badge>
                  </td>
                </tr>
              );
            }),
          )}
        </tbody>
      </Table>
    </Disclosure>
  );
}

/* -------------------------------------------------------------------- states */

/** “~/.config/ghost/policy.toml: Invalid value (at line 14, column 9)” → where, what, and the path apart. */
function splitLoadError(message: string): { where: string; what: string; path: string | null } {
  const path = message.match(/(~?\/[^\s:]+\.toml)/)?.[1] ?? null;
  let rest = message.replace(/^request:\s*/, "");
  if (path) rest = rest.replace(path, "").replace(/^\s*:\s*/, "");
  const at = rest.match(/\s*\(at line (\d+)(?:, column (\d+))?\)\s*/i);
  if (at) rest = rest.replace(at[0], " ").trim();
  const where = at ? `Line ${at[1]}${at[2] ? `, column ${at[2]}` : ""}: ` : "";
  const what = rest ? (where ? rest[0].toLowerCase() + rest.slice(1) : rest[0].toUpperCase() + rest.slice(1)) : "";
  return { where, what: what && !/[.!?]$/.test(what) ? `${what}.` : what, path };
}

function LoadError({ message, onRetry }: { message: string; onRetry: () => void }) {
  const [copied, copy] = useCopy();
  const { where, what, path } = splitLoadError(message);
  return (
    <Notice
      tone="danger"
      title="Couldn’t read the policy"
      action={
        <div className="policy-actions">
          <Button size="sm" variant="ghost" onClick={() => copy(path ?? DEFAULT_PATH)}>
            {copied ? "Copied" : "Copy path"}
          </Button>
          <Button size="sm" variant="secondary" onClick={onRetry}>
            Try again
          </Button>
        </div>
      }
    >
      <span data-user-content>
        {where}
        {what}
      </span>{" "}
      If the file has a typo, fix it in a text editor, then try again.
      <span className="policy-load__path t-mono">{path ?? DEFAULT_PATH}</span>
    </Notice>
  );
}

function PermissionsSkeleton({ view }: { view: View }) {
  if (view === "toml") {
    return (
      <div className="policy-skeleton" aria-busy="true" aria-label="Loading the policy file">
        <div className="policy-file__head">
          <Skeleton width={200} height={12} />
          <Skeleton width={40} height={12} />
        </div>
        <div className="policy-skeleton__file">
          {Array.from({ length: 12 }, (_, i) => (
            <Skeleton key={i} width={[0, 6].includes(i) ? "38%" : `${22 + ((i * 7) % 5) * 6}%`} height={10} />
          ))}
        </div>
      </div>
    );
  }
  return (
    <div className="policy-skeleton" aria-busy="true" aria-label="Loading the permissions">
      <Skeleton width={220} height={14} />
      <div className="policy-skeleton__modes">
        {[0, 1, 2].map((i) => (
          <Skeleton key={i} height={88} radius="var(--radius-lg)" />
        ))}
      </div>
      {[2, 3].map((rows, c) => (
        <div key={c} className="policy-skeleton__group">
          <Skeleton width={120} height={14} />
          <Card list>
            {Array.from({ length: rows }, (_, i) => (
              <div key={i} className="policy-rule">
                <div className="policy-rule__text">
                  <Skeleton width="40%" height={12} />
                  <Skeleton width="65%" height={10} />
                </div>
              </div>
            ))}
          </Card>
        </div>
      ))}
    </div>
  );
}
