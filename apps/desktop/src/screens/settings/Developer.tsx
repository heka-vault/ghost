/**
 * Owner: settings. Settings › Developer (ADR 0073, ADR 0074): shown once the version on About
 * was clicked seven times (app/developer.ts). Not a lock, and the page says so: everything
 * here the operator can already do with a file or a terminal.
 *   Debug mode         GHOST_DEBUG_AUDIO for an hour, in Ghost's memory only (ADR 0074)
 *   Advanced pages     Passwords, Skills, Outside tools, the Audit log, notification routes,
 *                      Permissions' Advanced, the settings file
 *   Experimental       a closed list of settings that are environment variables, each with
 *                      its real name, what it does, its value and where that comes from,
 *                      written to settings.env (GET/PUT /developer/switches)
 *   Tests              the model check, Send a test error report (`POST /reports/test`, what
 *                      was sent shown exactly) and Run Ghost's checks (`POST /developer/checks`,
 *                      `ghostd check`'s lines, problems first), each with its command to copy;
 *                      the first run again, the floating position, the speech models, the
 *                      version line; "Record a test session" opens the Hearing test
 *   Live views         the timing overlay's switch and Copy, and the microphone meter
 *                      (DeveloperLive.tsx, #/settings/developer/live; ADR 0073 as amended)
 *
 * States by URL (mock): `?debug=on` (in the scenario `debugmode`) shows debug mode on.
 */
import type { ChecksView, DebugAudioView, ReportTestView, SwitchView, SwitchesView } from "@ghost/client-ts";
import { useGhost, versionLine } from "@ghost/ui";
import { Copy } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { setDeveloper } from "../../app/developer";
import { useFloating } from "../../app/floating";
import { hrefFor, navigate } from "../../app/router";
import { settingsPath } from "../../app/routes";
import { Badge, Button, Card, ListRow, Notice, Section, Select, Input, Switch, errorText, useCopy, useToast } from "../../ui";
import { SECTION_TITLE, sectionDomId, tildePath } from "./model";
import { LiveViews } from "./DeveloperLive";
import { timeLeft } from "./plain";

export function useNow(every = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), every);
    return () => window.clearInterval(t);
  }, [every]);
  return now;
}

/* ------------------------------------------------------------------ debug mode */

function DebugMode() {
  const { client, status } = useGhost();
  const toast = useToast();
  const [view, setView] = useState<DebugAudioView | null>(null);
  const [busy, setBusy] = useState(false);
  const now = useNow();
  const mark = status?.debug_audio ?? null;
  useEffect(() => {
    client
      .debugAudio()
      .then(setView)
      .catch(() => setView(null));
  }, [client, mark?.until]);
  const set = async (on: boolean) => {
    setBusy(true);
    try {
      setView(await client.setDebugAudio(on));
    } catch (e) {
      toast({ title: on ? "Couldn’t turn debug mode on" : "Couldn’t turn debug mode off", description: errorText(e), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };
  const on = !!view?.on;
  const left = timeLeft(view?.until, now);
  const listeningOff = !!status?.not_listening;
  return (
    <Section
      id={sectionDomId("debug")}
      title={SECTION_TITLE.debug}
      description="Logs what Ghost hears and its audio decisions (wake matches, turns, barge-ins, your yes) to Ghost’s own log on this computer, for an hour. Nothing is sent anywhere."
      className="set-section"
    >
      {on && (
        <Notice
          tone="warn"
          title={view?.from_environment && !view.until ? "Debug mode is on for this run of Ghost" : `Debug mode is on${left ? `: ${left} left` : ""}`}
          action={
            view?.until ? (
              <Button size="sm" onClick={() => void set(false)} loading={busy}>
                Turn off now
              </Button>
            ) : undefined
          }
        >
          {view?.from_environment && !view.until
            ? "Ghost was started with GHOST_DEBUG_AUDIO=1, so it logs until that Ghost stops."
            : "Ghost is writing what it hears, word for word, to its log, even what wasn’t meant for it. It turns itself off when the hour is up, and whenever Ghost restarts."}
          {listeningOff ? " Listening is off, so nothing is heard and nothing is logged until you turn it back on." : ""}
        </Notice>
      )}
      <Card list>
        <ListRow
          title="Debug mode"
          description={
            <>
              The same as starting Ghost with <code>GHOST_DEBUG_AUDIO=1</code>, for one hour, then off by itself. A restart turns it off too: it is kept in Ghost’s memory, never in a file. While it is on, the rail and the floating Ghost say so. To give the log to someone, save a diagnostic report on About with “Add the debug log” before it ends.
            </>
          }
          meta={on ? <Badge size="sm" tone="warn" dot>{left ?? "On"}</Badge> : undefined}
          trailing={<Switch aria-label="Debug mode" checked={on} disabled={busy || !view || (on && !view.until)} onChange={(v: boolean) => void set(v)} />}
        />
      </Card>
    </Section>
  );
}

/* ------------------------------------------------------------------ advanced pages */

function AdvancedPages({ settingsPathText }: { settingsPathText: string | null }) {
  const row = (id: string, title: string, description: string) => <ListRow key={id} title={title} description={description} href={hrefFor(settingsPath(id))} chevron />;
  return (
    <Section id={sectionDomId("advanced")} title={SECTION_TITLE.advanced} description="Pages that were in the main list. Each is also linked from where it is used." className="set-section">
      <Card list>
        {row("passwords", "Passwords", "Every password in Ghost’s vault by name, what uses each and when it was last used. Never a value.")}
        {row("skills", "Skills", "What Ghost’s agents know how to do, from Ghost and from your ~/.agents/skills.")}
        {row("tools", "Outside tools", "Other AI tools on this computer that reach Ghost as named clients.")}
        {row("audit", "Audit log", "Every change Ghost proposed and what came of it; Today shows the same as a timeline.")}
        {row("notifications", "Notification routes", "Every way Ghost can reach you, how long it waits before the next, quiet hours, ntfy.")}
        <ListRow title="Permissions: Advanced" description="The mode, every rule, and the policy file itself." href={hrefFor("/settings/permissions/advanced")} chevron />
        <ListRow title="Settings file" description="Where Settings saves. Passwords are in Ghost’s vault, never in this file." meta={<span className="set-value is-mono">{settingsPathText ?? "~/.config/ghost/settings.env"}</span>} />
      </Card>
    </Section>
  );
}

/* ------------------------------------------------------------------ experimental switches */

const SOURCE: Record<SwitchView["source"], string> = {
  file: "from settings.env",
  environment: "from the environment Ghost was started in",
  default: "Ghost’s default",
};

function SwitchControl({ s, busy, onSave }: { s: SwitchView; busy: boolean; onSave: (v: string | null) => void }) {
  const [text, setText] = useState(s.file_value ?? "");
  useEffect(() => setText(s.file_value ?? ""), [s.file_value]);
  const label = `${s.title} (${s.name})`;
  if (s.kind === "flag") {
    const now = (s.value || s.default) === "on" || ["true", "1", "yes"].includes((s.value || "").toLowerCase());
    return <Switch aria-label={label} checked={now} disabled={busy} onChange={(on: boolean) => onSave(on === (s.default === "on") ? null : on ? "on" : "off")} />;
  }
  if (s.kind === "choice") {
    return (
      <Select
        size="sm"
        aria-label={label}
        value={s.value || s.default}
        disabled={busy}
        options={s.choices.map((c) => ({ value: c, label: c === s.default ? `${c} (default)` : c }))}
        onChange={(e) => onSave(e.target.value === s.default ? null : e.target.value)}
      />
    );
  }
  return (
    <form
      className="set-switch-number"
      onSubmit={(e) => {
        e.preventDefault();
        onSave(text.trim() === "" ? null : text.trim());
      }}
    >
      <Input size="sm" aria-label={label} inputMode="decimal" placeholder={s.default || "auto"} value={text} disabled={busy} onChange={(e) => setText(e.target.value)} />
      <Button size="sm" variant="ghost" type="submit" disabled={busy || text === (s.file_value ?? "")}>
        Save
      </Button>
    </form>
  );
}

function Experimental() {
  const { client } = useGhost();
  const toast = useToast();
  const [view, setView] = useState<SwitchesView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [waiting, setWaiting] = useState<string[]>([]);
  const load = useCallback(() => {
    client
      .developerSwitches()
      .then((v) => {
        setView(v);
        setError(null);
      })
      .catch((e) => setError(errorText(e)));
  }, [client]);
  useEffect(load, [load]);
  const save = async (name: string, value: string | null) => {
    setBusy(name);
    try {
      const r = await client.setDeveloperSwitches({ [name]: value });
      setView(r.switches);
      if (r.restart_required) setWaiting((w) => Array.from(new Set([...w, name])));
      toast({ title: `Saved ${name}`, description: r.applied.includes(name) ? "Ghost uses it now." : "Ghost uses it after its next restart.", tone: "ok" });
    } catch (e) {
      toast({ title: `Couldn’t save ${name}`, description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };
  return (
    <Section
      id={sectionDomId("experimental")}
      title={SECTION_TITLE.experimental}
      description={`Settings that are environment variables, by their real names. Each is saved to ${view?.settings_path ?? "settings.env"} and takes effect when Ghost next restarts. A value from the environment Ghost was started in wins over the file.`}
      className="set-section"
    >
      {error && <Notice tone="danger" title="Couldn’t read the switches">{error}</Notice>}
      {view && (
        <Card list>
          {view.switches.map((s) => (
            <ListRow
              key={s.name}
              align="start"
              title={
                <>
                  {s.title} <code className="set-switch-name">{s.name}</code>
                </>
              }
              description={
                <>
                  {s.sentence}{" "}
                  <span className="set-switch-source">
                    Now: <span className="is-mono">{s.value || s.default || "worked out by Ghost"}</span>, {SOURCE[s.source]}.
                    {s.source === "environment" && s.file_value != null ? " The file’s value is overridden." : ""}
                    {waiting.includes(s.name) ? " Waits for a restart." : ""}
                  </span>
                </>
              }
              trailing={<SwitchControl s={s} busy={busy !== null} onSave={(v) => void save(s.name, v)} />}
            />
          ))}
        </Card>
      )}
    </Section>
  );
}

/* ------------------------------------------------------------------ tests */

function CommandButton({ command }: { command: string }) {
  const [copied, copy] = useCopy();
  return (
    <Button size="sm" variant="ghost" leadingIcon={<Copy />} onClick={() => copy(command)} aria-label={`Copy ${command}`}>
      {copied ? "Copied" : "Copy the command"}
    </Button>
  );
}

/** Send a test error report now, and show exactly what went (or why nothing did). */
function TestReport() {
  const { client } = useGhost();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ReportTestView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const send = async () => {
    setBusy(true);
    setError(null);
    try {
      setResult(await client.sendTestReport());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <ListRow
        title="Send a test error report"
        description="Sends one test report now, as ghostd reports test does, and shows exactly what went. A build from the checkout has no key and sends nothing."
        trailing={
          <span className="set-row-actions">
            <CommandButton command="ghostd reports test" />
            <Button size="sm" variant="secondary" loading={busy} onClick={() => void send()}>
              Send
            </Button>
          </span>
        }
      />
      {(result || error) && (
        <div className="dev-result" data-testid="test-report-result">
          {error ? (
            <Notice tone="danger" title="Couldn’t send a test report">
              {error}
            </Notice>
          ) : result && result.sent ? (
            <>
              <p className="dev-result__head">{result.message}</p>
              <pre>{JSON.stringify(result.body, null, 2)}</pre>
            </>
          ) : (
            <Notice tone={result?.available ? "warn" : "info"} title={result?.available ? "Nothing was sent" : "This build sends no error reports"}>
              {result?.message}
            </Notice>
          )}
        </div>
      )}
    </>
  );
}

/** `ghostd check`'s lines as a list, problems first. */
export function checksSummary(v: ChecksView): string {
  if (v.ok) return `All clear in ${v.secs.toFixed(1)} s.`;
  return `${v.problems} problem${v.problems === 1 ? "" : "s"}, in ${v.secs.toFixed(1)} s.`;
}

/** Run `ghostd check` and show its lines, problems first. */
function RunChecks() {
  const { client } = useGhost();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ChecksView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      setResult(await client.runChecks());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <ListRow
        title="Run Ghost’s checks"
        description="What ghostd check checks: the settings, the vault, the folder, the speech models, the microphone and each app (it connects each once, as the command does). Takes a few seconds."
        trailing={
          <span className="set-row-actions">
            <CommandButton command="ghostd check" />
            <Button size="sm" variant="secondary" loading={busy} onClick={() => void run()}>
              Run
            </Button>
          </span>
        }
      />
      {(result || error) && (
        <div className="dev-result" data-testid="checks-result">
          {error ? (
            <Notice tone="danger" title="Couldn’t run the checks">
              {error}
            </Notice>
          ) : (
            result && (
              <>
                <p className="dev-result__head">
                  <Badge size="sm" tone={result.ok ? "ok" : "danger"} dot>
                    {result.ok ? "OK" : "Problems"}
                  </Badge>{" "}
                  {checksSummary(result)}
                </p>
                <ul>
                  {result.lines.map((l, i) => (
                    <li key={i} className={l.problem ? "is-problem" : l.warning ? "is-warning" : undefined}>
                      {l.problem ? "✕ " : l.warning ? "! " : ""}
                      {l.text}
                    </li>
                  ))}
                </ul>
              </>
            )
          )}
        </div>
      )}
    </>
  );
}

function Tests() {
  const { client, status } = useGhost();
  const toast = useToast();
  const floating = useFloating();
  const [checking, setChecking] = useState(false);
  const [copiedVersion, copyVersion] = useCopy();
  const runCheck = async () => {
    setChecking(true);
    try {
      const r = await client.checkModel();
      const words = r.models.map((m) => `${m.role}: ${m.reachable ? `first word ${m.first_word.median_ms != null ? (m.first_word.median_ms / 1000).toFixed(2) : "?"} s` : (m.error ?? "unreachable")}`).join("; ");
      toast({ title: "Model check done", description: words || "Nothing to check.", tone: r.models.every((m) => m.reachable) ? "ok" : "danger" });
    } catch (e) {
      toast({ title: "Couldn’t check the model", description: errorText(e), tone: "danger" });
    } finally {
      setChecking(false);
    }
  };
  const speech = async () => {
    try {
      const v = await client.speechModels();
      toast({ title: v.missing.length ? `${v.missing.length} speech model file${v.missing.length === 1 ? "" : "s"} missing` : "The speech models are all here", description: v.missing.length ? "Download them from Settings › Sound." : "Each matches its pinned size and hash.", tone: v.missing.length ? "danger" : "ok" });
    } catch (e) {
      toast({ title: "Couldn’t check the speech models", description: errorText(e), tone: "danger" });
    }
  };
  const resetFloating = async () => {
    if (!floating.status) return;
    await floating.set({ ...floating.status.settings, placement: "beside", monitor: "other" });
    toast({ title: "Floating position reset", description: "Beside your work, on the other monitor.", tone: "ok" });
  };
  const line = status?.version ? versionLine(status.version, status.build) : null;
  return (
    <Section id={sectionDomId("tests")} title={SECTION_TITLE.tests} description="Checks to run while testing a build, and what to copy into a bug report." className="set-section">
      <Card list>
        <ListRow title="Record a test session" description="The Hearing test’s recorder: talk to Ghost as usual and keep the raw audio on this computer to score how well it hears." trailing={<Button size="sm" variant="secondary" href={hrefFor("/hear")}>Open the recorder</Button>} />
        <ListRow title="Check the model" description="A few short calls to the model connection: how soon the first word comes, and what works." trailing={<Button size="sm" variant="secondary" loading={checking} onClick={() => void runCheck()}>Run the check</Button>} />
        <TestReport />
        <RunChecks />
        <ListRow title="Check the speech models" description="Whether Parakeet’s and Kokoro’s files are all here." trailing={<Button size="sm" variant="secondary" onClick={() => void speech()}>Check</Button>} />
        <ListRow title="Replay the first run" description="Set up Ghost again from the start; it keeps what’s done." trailing={<Button size="sm" variant="secondary" href={hrefFor("/welcome")}>Open setup</Button>} />
        {floating.status && <ListRow title="Reset the floating position" description="Back to beside your work, on the other monitor." trailing={<Button size="sm" variant="secondary" onClick={() => void resetFloating()}>Reset</Button>} />}
        <ListRow
          title="Copy the version line"
          description={line ?? "Ghost isn’t answering."}
          trailing={
            <Button size="sm" variant="secondary" leadingIcon={<Copy />} disabled={!line} onClick={() => line && copyVersion(line)}>
              {copiedVersion ? "Copied" : "Copy"}
            </Button>
          }
        />
      </Card>
    </Section>
  );
}

/* ------------------------------------------------------------------ the page */

export function DeveloperPage({ settingsPathText }: { settingsPathText: string | null }) {
  return (
    <>
      <Notice
        tone="info"
        title="For testing Ghost"
        action={
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setDeveloper(false);
              navigate("/settings/about");
            }}
          >
            Hide Developer
          </Button>
        }
      >
        Nothing here is locked away from you: each thing on this page is also a file you can edit or a command you can run. Hiding the page changes nothing it set.
      </Notice>
      <DebugMode />
      <Tests />
      <Experimental />
      <LiveViews />
      <AdvancedPages settingsPathText={settingsPathText ? tildePath(settingsPathText) : null} />
    </>
  );
}
