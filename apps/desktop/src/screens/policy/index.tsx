/**
 * Owner: policy. What Ghost may do on its own (ADR 0011): a level per operation, written to
 * ~/.config/ghost/policy.toml through PUT /policy. The operator's file decides; the model
 * never sees this screen or the file.
 *
 * Levels and TOML are two views of one text. The page keeps the file as text, reads the
 * levels out of it, and a level change rewrites only its own line, so switching views keeps
 * every edit and every save keeps the operator’s comments.
 */
import { ApiError, type PolicyView, type SourceInfo } from "@ghost/client-ts";
import { autonomyLabel, useGhost } from "@ghost/ui";
import { Code, SlidersHorizontal } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import {
  Badge,
  Button,
  Card,
  LiveStatus,
  Notice,
  Page,
  SegmentedControl,
  Section,
  Skeleton,
  SkeletonText,
  errorText,
  useConfirm,
  useLive,
  useToast,
} from "../../ui";
import { type BarState, SaveBar, useCopy } from "./Bar";
import { useLeaveGuard } from "./guard";
import { DefaultRow, LevelHeader, OperationRow, Rich, useStuckHeader } from "./Levels";
import {
  type PolicyData,
  SOURCE_COPY,
  changedKeys,
  errorLine,
  highRiskNote,
  inherited,
  isServer,
  levelTable,
  listWords,
  operationText,
  operationTitle,
  parsePolicy,
  setKey,
  sourceText,
  sourceTitle,
  splitSources,
  targetRules,
} from "./model";
import { TomlEditor } from "./TomlEditor";
import "./policy.css";

type Mode = "levels" | "toml";

/**
 * Design states, read from the query before the hash (`?mock=passive&policy=dirty#/policy`).
 * `toml` works anywhere (the Servers screen links to the file this way); the rest only
 * while the mock layer is active.
 */
type DesignState = "toml" | "dirty" | "invalid" | "load-error" | "inline" | null;
function readDesignState(mock: boolean): DesignState {
  const v = new URLSearchParams(window.location.search).get("policy");
  if (v === "toml") return "toml";
  if (!mock) return null;
  return v === "dirty" || v === "invalid" || v === "load-error" || v === "inline" ? v : null;
}

/** A daemon message as a sentence: capitalized, ending in a period, without the “request:” source. */
function sentence(s: string): string {
  const t = s.replace(/^request:\s*/, "").trim();
  if (!t) return t;
  return t[0].toUpperCase() + t.slice(1) + (/[.!?]$/.test(t) ? "" : ".");
}
/** The same, without a leading “Line 4:” where the line is already named. */
const lineless = (s: string) => sentence(s.replace(/^(request:\s*)?line \d+:\s*/i, ""));
const DEFAULT_PATH = "~/.config/ghost/policy.toml";

const fromView = (p: PolicyView): PolicyData => ({ default: p.default ?? null, skills: p.skills, targets: p.targets });
const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

export default function PolicyScreen(_: RouteProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const design = useDesign();
  const [designState] = useState(() => readDesignState(!!design));

  /** Every source the gate knows (ADR 0022): the lines, and each connected server. */
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  const [policy, setPolicy] = useState<PolicyView | null>(null);
  /** The file as the operator is editing it; both views read and write this. */
  const [text, setText] = useState("");
  const [mode, setMode] = useState<Mode>(designState === "toml" || designState === "invalid" ? "toml" : "levels");
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** `saved`: Ghost was asked to write it; otherwise the page found the fault before asking. */
  const [saveError, setSaveError] = useState<{ message: string; line: number | null; saved: boolean } | null>(null);
  /** The row whose last edit only TOML can make (“default”, or “source.op”). */
  const [tomlOnly, setTomlOnly] = useState<string | null>(null);
  /** Keys in the order the operator changed them, most recent first, for the dock’s summary. */
  const editOrder = useRef<string[]>([]);
  /** Bumped when a failed save should take the operator to the line at fault. */
  const [showLine, setShowLine] = useState(0);
  const editor = useRef<HTMLTextAreaElement>(null);

  const adopt = useCallback((p: PolicyView) => {
    setPolicy(p);
    setText(p.text);
  }, []);

  // Only the latest load may land (StrictMode runs the first effect twice).
  const loadSeq = useRef(0);
  const load = useCallback(async () => {
    const seq = ++loadSeq.current;
    setLoadError(null);
    try {
      if (designState === "load-error") {
        await new Promise((r) => setTimeout(r, 200));
        throw new ApiError(422, `${DEFAULT_PATH}: Invalid value (at line 14, column 9)`);
      }
      const [a, p] = await Promise.all([client.sources(), client.policy()]);
      if (seq !== loadSeq.current) return null;
      setSources(a);
      adopt(p);
      return p;
    } catch (e) {
      if (seq === loadSeq.current) setLoadError(errorText(e));
      return null;
    }
  }, [client, adopt, designState]);

  // First load, then the design state on top of it (mock only).
  useEffect(() => {
    let live = true;
    load().then((p) => {
      if (!live || !p) return;
      if (designState === "dirty") {
        let t: string | null = p.text;
        t = t && setKey(t, ["mcp", "github_heka", "levels"], "comment", "quiet");
        t = t && setKey(t, ["mcp", "github_heka", "levels"], "close", "quiet");
        t = t && setKey(t, ["defaults"], "autonomy", "confirm");
        if (t) setText(t);
      } else if (designState === "invalid") {
        const broken = p.text.replace(/^(close = "confirm")$/m, "close = confirm");
        const r = parsePolicy(broken);
        setText(broken);
        if (!r.ok) {
          setSaveError({ message: r.message, line: r.line, saved: false });
          setShowLine((n) => n + 1);
        }
      } else if (designState === "inline") {
        // Hand-written: Everything else as an inline table, which Levels won’t rewrite.
        const lines = p.text.split("\n");
        lines.splice(1, 0, 'defaults = { autonomy = "confirm" }');
        setPolicy({ ...p, text: lines.join("\n") });
        setText(lines.join("\n"));
      }
    });
    return () => {
      live = false;
    };
  }, [load, designState]);

  // ------------------------------------------------------------------------ the file
  const saved = useMemo(() => (policy ? parsePolicy(policy.text) : null), [policy]);
  const draft = useMemo(() => parsePolicy(text), [text]);
  /** What the saved file sets. Ghost’s own reading when this page can’t follow the TOML. */
  const savedData = useMemo<PolicyData | null>(() => (policy ? (saved?.ok ? saved.data : fromView(policy)) : null), [policy, saved]);
  const draftData = draft.ok ? draft.data : null;
  const shown = draftData ?? savedData;
  /** Levels can change the file only while it reads cleanly here. */
  const editable = !!draftData;

  const changed = useMemo(
    () =>
      draftData && savedData ? changedKeys(draftData.skills, savedData.skills, draftData.default ?? "", savedData.default ?? "") : new Set<string>(),
    [draftData, savedData],
  );
  const dirty = !!policy && text !== policy.text;

  /**
   * Apply one level edit to the text. `source` is null for [defaults]; a server's levels
   * live under `[mcp.<name>.levels]`, a built-in's under `[<name>]` (levelTable). Back to
   * the saved levels means back to the saved text.
   */
  const edit = (source: SourceInfo | null, key: string, level: string | null) => {
    if (!policy || !savedData) return;
    const table = source ? levelTable(source) : ["defaults"];
    const id = source ? `${source.name}.${key}` : "default";
    const next = setKey(text, table, key, level);
    const r = next == null ? null : parsePolicy(next);
    // The rewrite must read back as exactly this one change, or it belongs in TOML.
    const landed = !!r?.ok && (source ? r.data.skills[source.name]?.[key] === (level ?? undefined) : (r.data.default ?? null) === (level || null));
    if (next == null || !r || !landed) {
      // Said under the row itself: a toast here would sit over the dock’s Save.
      setTomlOnly(id);
      return;
    }
    setTomlOnly(null);
    editOrder.current = [id, ...editOrder.current.filter((k) => k !== id)];
    const back = r.ok && changedKeys(r.data.skills, savedData.skills, r.data.default ?? "", savedData.default ?? "").size === 0;
    setText(back && saved?.ok ? policy.text : next);
    setSaveError(null);
  };
  const setLevel = (source: SourceInfo, op: string, level: string | null) => edit(source, op, level);
  const setDefaultLevel = (level: string) => edit(null, "autonomy", level || null);

  const discard = () => {
    if (policy) setText(policy.text);
    setSaveError(null);
    setTomlOnly(null);
  };

  const switchMode = async (next: Mode) => {
    if (next === mode) return;
    setTomlOnly(null);
    if (next === "levels" && !draft.ok && draft.certain) {
      setSaveError({ message: draft.message, line: draft.line, saved: false });
      setShowLine((n) => n + 1);
      const ok = await confirm({
        title: `Line ${draft.line} needs a fix first`,
        description: (
          <>
            <Rich text={lineless(draft.message)} /> Levels can show the file once it reads cleanly. Fix it here, or discard your edits to the file.
          </>
        ),
        confirmLabel: "Discard edits",
        tone: "danger",
      });
      if (!ok) return;
      discard();
    }
    setMode(next);
  };

  // ----------------------------------------------------------------------------- save
  /** Levels apply the moment the file is written (ADR 0022); the sources read back the new ones. */
  const finish = (r: { policy: PolicyView }) => {
    adopt(r.policy);
    client.sources().then(setSources).catch(() => {});
  };

  const putBack = async (previous: string) => {
    try {
      const r = await client.updatePolicy({ text: previous });
      finish(r);
      setSaveError(null);
      toast({ title: "Put back the previous levels" });
    } catch (e) {
      setSaveError({ message: errorText(e), line: null, saved: true });
    }
  };

  const save = async () => {
    if (!dirty || saving || !online || !policy) return;
    // Ghost would refuse it too: say which line now, without a round trip.
    if (!draft.ok && draft.certain) {
      setSaveError({ message: draft.message, line: draft.line, saved: true });
      if (mode === "levels") setMode("toml");
      setShowLine((n) => n + 1);
      return;
    }
    setSaving(true);
    setSaveError(null);
    const before = policy;
    try {
      const r = await client.updatePolicy({ text });
      finish(r);
      toast({
        tone: "ok",
        title: "Saved",
        description: "Ghost uses the new levels from its next change.",
        action: before.exists ? { label: "Undo", onClick: () => void putBack(before.text) } : undefined,
      });
    } catch (e) {
      const message = errorText(e);
      const line = errorLine(message);
      setSaveError({ message, line, saved: true });
      if (line != null) {
        if (mode === "levels") setMode("toml");
        setShowLine((n) => n + 1);
      }
    } finally {
      setSaving(false);
    }
  };

  // Ctrl S saves, as everywhere else on Linux.
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
    if (!showLine || mode !== "toml" || !saveError?.line) return;
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
  }, [showLine, mode]);

  // Leaving with unsaved edits asks first.
  useLeaveGuard(dirty && !saving, "/policy", () =>
    confirm({
      title: "Leave without saving?",
      description: changed.size
        ? `${plural(changed.size, "change")} to what Ghost may do ${changed.size === 1 ? "isn’t" : "aren’t"} saved. Leaving discards ${changed.size === 1 ? "it" : "them"}.`
        : "Your edits to the policy file aren’t saved. Leaving discards them.",
      confirmLabel: "Discard changes",
      tone: "danger",
    }),
  );

  // ---------------------------------------------------------------------------- views
  const { writers, readers } = useMemo(() => splitSources(sources ?? []), [sources]);
  const loading = !policy && !loadError;
  const errorId = useId();
  const helpId = useId();
  const header = useRef<HTMLDivElement>(null);
  useStuckHeader(header, [mode, policy, sources]);
  const openToml = () => void switchMode("toml");

  const shownDefault = shown?.default ?? "";
  const levels = shown && (
    <>
      {policy && !policy.exists && !dirty && (
        <p className="policy-lead-note t-body-lg t-2">
          No policy file yet. Every operation follows its risk; saving creates <span className="t-mono">{policy.path}</span>.
        </p>
      )}
      {!editable && (
        <Notice
          tone="warn"
          title="Levels can’t change this file"
          className="policy-notice"
          action={
            <Button size="sm" variant="secondary" leadingIcon={<Code />} onClick={openToml}>
              Edit in TOML
            </Button>
          }
        >
          <span data-user-content>{!draft.ok && <Rich text={sentence(draft.message)} />}</span> The levels below are what Ghost uses now. Change them
          in TOML.
        </Notice>
      )}

      <div className="policy-matrix">
        <LevelHeader headerRef={header} />

        <Card list className="policy-card policy-card--default">
          <DefaultRow
            value={shownDefault}
            edited={changed.has("default")}
            disabled={!editable}
            tomlOnly={tomlOnly === "default"}
            onOpenToml={openToml}
            onChange={setDefaultLevel}
          />
        </Card>

        {writers.map((a) => {
          const title = sourceTitle(a.name);
          const tRules = shown.targets[a.name] ?? {};
          const targetNames = Object.keys(tRules);
          const own = shown.skills[a.name] ?? {};
          // A `*` line the operator wrote gets its own row, so it can be changed here too.
          const wide = own["*"] != null || savedData?.skills[a.name]?.["*"] != null;
          const rowFor = (name: string, risk: string, opTitle: string, description: ReactNode, inh: { level: string; from: string }) => (
            <OperationRow
              key={name}
              source={a.name}
              name={name}
              risk={risk}
              title={opTitle}
              description={description}
              own={own[name]}
              inheritedLevel={inh.level}
              inheritedFrom={inh.from === "source" ? `Set for all of ${title}` : ""}
              edited={changed.has(`${a.name}.${name}`)}
              targets={name === "*" ? [] : targetRules(shown.targets, a.name, name)}
              riskNote={highRiskNote(a.name, name)}
              disabled={!editable}
              tomlOnly={tomlOnly === `${a.name}.${name}`}
              onOpenToml={openToml}
              onChange={(lv) => setLevel(a, name, lv)}
            />
          );
          return (
            <Section
              key={a.name}
              className="policy-source"
              title={
                <span className="policy-source__title">
                  {title}
                  {isServer(a) ? <Badge size="sm">Server</Badge> : <Badge size="sm">Built in</Badge>}
                </span>
              }
              description={<span data-user-content={SOURCE_COPY[a.name] ? undefined : true}>{sourceText(a)}</span>}
              actions={
                // Where these levels live in the file: a server's under its declaration.
                <span className="t-mono t-3 policy-source__id">{isServer(a) ? `mcp.${a.name}.levels` : a.name}</span>
              }
            >
              <Card list className="policy-card">
                {wide &&
                  rowFor(
                    "*",
                    "",
                    `All of ${title}`,
                    `One level for everything ${title} does. An operation with its own level keeps it.`,
                    { level: shownDefault, from: "default" },
                  )}
                {a.operations.map((op) =>
                  rowFor(op.name, op.risk, operationTitle(a.name, op.name), operationText(a.name, op), inherited(shown.skills, shownDefault, a.name, op)),
                )}
                {targetNames.length > 0 && (
                  <div className="policy-card__foot">
                    <p className="t-small t-2">
                      <span className="t-mono">{listWords(targetNames)}</span> {targetNames.length === 1 ? "has its own rules" : "have their own rules"}, and
                      they win over the levels above. Change them in the file.
                    </p>
                    <Button size="sm" variant="ghost" leadingIcon={<Code />} onClick={openToml}>
                      Edit in TOML
                    </Button>
                  </div>
                )}
              </Card>
            </Section>
          );
        })}
      </div>

      {readers.length > 0 && (
        <div className="policy-readers t-small t-3">
          <p>
            {listWords(readers.map((r) => sourceTitle(r.name)))} only {readers.length === 1 ? "reads" : "read"}, so there’s nothing to allow. A write on a
            server is offered once you declare it in the file.
          </p>
        </div>
      )}
    </>
  );

  const toml = policy && (
    <div className="policy-file">
      <div className="policy-file__head">
        <span className="t-mono t-2">{policy.path}</span>
        <span className="t-small t-3">{dirty ? "Edited" : !policy.exists ? "Not created yet" : "Saved"}</span>
      </div>
      {/* The fault is shown on its line and spoken by the dock; this only describes the field. */}
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
        placeholder={'[conversations]\ndelete = "confirm"'}
      />
      <p id={helpId} className="t-small t-3 policy-file__help">
        Ghost checks the file before writing it; if it doesn’t read cleanly, nothing changes. Comments stay as you write them, from here and from
        Levels.
      </p>
    </div>
  );

  // -------------------------------------------------------------------------- the dock
  /** Every key in the order it appears on screen, so edits from TOML still read top to bottom. */
  const screenOrder = useMemo(
    () => ["default", ...writers.flatMap((a) => [`${a.name}.*`, ...a.operations.map((o) => `${a.name}.${o.name}`)])],
    [writers],
  );
  const summary = useMemo(() => {
    if (!draftData) return "";
    const rank = (k: string) => {
      const e = editOrder.current.indexOf(k);
      if (e >= 0) return e;
      const s = screenOrder.indexOf(k);
      return 1000 + (s >= 0 ? s : 999);
    };
    const followers = writers.reduce(
      (n, a) => (draftData.skills[a.name]?.["*"] ? n : n + a.operations.filter((o) => draftData.skills[a.name]?.[o.name] == null).length),
      0,
    );
    const items = [...changed]
      .sort((x, y) => rank(x) - rank(y))
      .map((k) => {
        if (k === "default") {
          const lv = draftData.default ? autonomyLabel(draftData.default).toLowerCase() : "by risk";
          return `Everything else: ${lv}${followers ? ` (${plural(followers, "operation")})` : ""}`;
        }
        const [source, op] = k.split(".");
        const lv = draftData.skills[source]?.[op];
        const name = op === "*" ? `All of ${sourceTitle(source)}` : operationTitle(source, op);
        if (lv) return `${name}: ${autonomyLabel(lv).toLowerCase()}`;
        // Back to the default: say what that lands on.
        const info = writers.find((a) => a.name === source)?.operations.find((o) => o.name === op);
        const inh = info ? inherited(draftData.skills, draftData.default ?? "", source, info).level : draftData.default;
        return `${name}: default${inh ? `, ${autonomyLabel(inh).toLowerCase()}` : ""}`;
      });
    return items.join(" · ") || "Comments and layout only";
  }, [changed, draftData, screenOrder, writers]);

  let bar: BarState | null = null;
  if (dirty) {
    const n = changed.size;
    let title = n ? plural(n, "unsaved change") : "Unsaved edits to the file";
    let error: Extract<BarState, { kind: "dirty" }>["error"] = null;
    if (saveError) {
      // “Couldn’t save” only after Ghost was asked; a fault found on the way to Levels just needs a fix.
      title = saveError.saved ? `Couldn’t save${saveError.line ? ` · line ${saveError.line}` : ""}` : `Line ${saveError.line}`;
      // After the dot the fault reads as the rest of the sentence: no capital, no period.
      const plain = lineless(saveError.message);
      const low = plain ? plain[0].toLowerCase() + plain.slice(1).replace(/\.$/, "") : plain;
      const tail = saveError.saved ? " Nothing changed on disk." : "";
      error = { text: <Rich text={saveError.line ? low : plain + tail} />, plain: plain + tail, line: saveError.line };
    }
    bar = { kind: "dirty", title, summary, online, busy: saving, error };
  }

  return (
    <Page
      width="default"
      title="What Ghost may do"
      description="Choose how much Ghost does on its own for each kind of change. Every change is logged. To reverse one, ask Ghost to change it back."
      status={<LiveStatus />}
      actions={
        // Always there, so the header holds still while the policy loads.
        <SegmentedControl<Mode>
          aria-label="Edit as"
          value={mode}
          disabled={!policy}
          onChange={(m) => void switchMode(m)}
          options={[
            { value: "levels", label: "Levels", icon: <SlidersHorizontal /> },
            { value: "toml", label: "TOML", icon: <Code /> },
          ]}
        />
      }
      dock={bar && <SaveBar state={bar} onSave={() => void save()} onDiscard={discard} />}
    >
      {loadError ? (
        <LoadError message={loadError} onRetry={() => void load()} />
      ) : loading ? (
        <PolicySkeleton mode={mode} />
      ) : mode === "levels" ? (
        levels
      ) : (
        toml
      )}
      {dialog}
    </Page>
  );
}

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

function PolicySkeleton({ mode }: { mode: Mode }) {
  if (mode === "toml") {
    return (
      <div className="policy-skeleton" aria-busy="true" aria-label="Loading the policy file">
        <div className="policy-file__head">
          <Skeleton width={200} height={12} />
          <Skeleton width={40} height={12} />
        </div>
        <div className="policy-skeleton__file">
          {Array.from({ length: 18 }, (_, i) => (
            <Skeleton key={i} width={[0, 6, 12].includes(i) ? "38%" : i % 6 === 5 ? 0 : `${22 + ((i * 7) % 5) * 6}%`} height={10} />
          ))}
        </div>
      </div>
    );
  }
  return (
    <div className="policy-skeleton" aria-busy="true" aria-label="Loading what Ghost may do">
      <div className="policy-cols" aria-hidden>
        <div className="policy-cols__levels">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="policy-cols__level">
              <Skeleton width={14} height={14} />
              <Skeleton width={64} height={10} />
            </div>
          ))}
        </div>
      </div>
      {[1, 4].map((rows, c) => (
        <div key={c} className="policy-skeleton__group">
          {c > 0 && (
            <div className="policy-skeleton__title">
              <Skeleton width={120} height={14} />
              <Skeleton width={320} height={10} />
            </div>
          )}
          <Card list className="policy-card">
            {Array.from({ length: rows }, (_, i) => (
              <div key={i} className="policy-row">
                <div className="policy-row__text">
                  <SkeletonText lines={2} />
                </div>
                <span className="policy-row__gutter" />
                <div className="policy-stops is-empty">
                  <div className="policy-stops__rail">
                    <span className="policy-stops__track" />
                  </div>
                </div>
              </div>
            ))}
          </Card>
        </div>
      ))}
    </div>
  );
}
