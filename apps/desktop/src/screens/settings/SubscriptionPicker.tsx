/**
 * Owner: settings. Choose your subscription (ADR 0078): Claude Code, OpenAI Codex or Google
 * Antigravity, each the vendor's own program. One card each with its state and one action:
 * Install (its own installer, the command shown first), Sign in (its own sign-in: the browser
 * opens; the link in case it didn't; the code its page shows pasted back here), Use it. Settings
 * › Model and the first run's model step show the same picker.
 *
 * A pasted code goes to Ghost once and is never shown again: the field is cleared as it is sent,
 * and the program's echo of it is hidden by the daemon.
 */
import type { SubscriptionJobView, SubscriptionsView, SubscriptionView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Copy, ExternalLink, RefreshCw, Terminal } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Badge, Button, Card, errorText, Input, Kbd, ListRow, Notice, Skeleton, useConfirm, useCopy, useLive, useToast } from "../../ui";
import { openSignIn } from "../apps/signin";
import { cardFace, codeProblem, endedWords, inUseLine, installWords, jobRunning } from "./subscriptions";

const POLL_MS = 1000;

/** The subscriptions as their programs say they are, asked when the screen opens. */
export function useSubscriptions() {
  const { client } = useGhost();
  const { online } = useLive();
  const [view, setView] = useState<SubscriptionsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [looking, setLooking] = useState(false);
  const look = useCallback(async () => {
    setLooking(true);
    try {
      setView(await client.subscriptions());
      setError(null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLooking(false);
    }
  }, [client]);
  useEffect(() => {
    if (online) void look();
  }, [online, look]);
  return { view, setView, error, look, looking };
}

export interface SubscriptionPickerProps {
  view: SubscriptionsView | null;
  error?: string | null;
  look: () => Promise<void>;
  looking: boolean;
  setView: (v: SubscriptionsView) => void;
  /** It became Ghost's model: read the settings and the connection again. */
  onUsed?: () => void;
  /** Ghost's model now when it is none of these (its provider's title), for the line on top. */
  otherProvider?: string | null;
  mock?: boolean;
}

export function SubscriptionPicker({ view, error, look, looking, setView, onUsed, otherProvider = null, mock = false }: SubscriptionPickerProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [busy, setBusy] = useState<string | null>(null);

  // A job that is going on is followed without asking every program again; when it ends,
  // everything is asked again.
  const running = view?.subscriptions.filter((s) => jobRunning(s.job)).map((s) => s.id) ?? [];
  const runningKey = running.join(",");
  const viewRef = useRef(view);
  viewRef.current = view;
  useEffect(() => {
    if (!runningKey) return;
    let live = true;
    const t = window.setInterval(async () => {
      for (const id of runningKey.split(",")) {
        try {
          const job = await client.subscriptionJob(id);
          if (!live || !viewRef.current) return;
          const subs = viewRef.current.subscriptions.map((s) => (s.id === id ? { ...s, job: job ?? null } : s));
          setView({ ...viewRef.current, subscriptions: subs });
          if (job && !jobRunning(job)) {
            const sub = subs.find((s) => s.id === id);
            const words = endedWords(sub?.title ?? id, job);
            if (words) toast({ title: words.title, description: job.message ?? undefined, tone: words.tone });
            void look();
          }
        } catch {
          /* the next tick asks again */
        }
      }
    }, POLL_MS);
    return () => {
      live = false;
      window.clearInterval(t);
    };
  }, [client, runningKey, setView, look, toast]);

  const withJob = (id: string, job: SubscriptionJobView) => {
    if (!view) return;
    setView({ ...view, subscriptions: view.subscriptions.map((s) => (s.id === id ? { ...s, job } : s)) });
  };

  const install = async (s: SubscriptionView) => {
    const words = installWords(s);
    const ok = await confirm({
      title: words.title,
      description: (
        <>
          <p>{words.body}</p>
          <p className="set-subs__command">{s.install_command}</p>
          <p className="set-help is-readable">From {s.install_source}</p>
        </>
      ),
      confirmLabel: "Install",
    });
    if (!ok) return;
    setBusy(s.id);
    try {
      withJob(s.id, await client.installSubscription(s.id));
    } catch (e) {
      toast({ title: `Couldn’t install ${s.title}`, description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const signIn = async (s: SubscriptionView) => {
    setBusy(s.id);
    try {
      withJob(s.id, await client.signInSubscription(s.id));
    } catch (e) {
      toast({ title: `Couldn’t start ${s.title}’s sign-in`, description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const use = async (s: SubscriptionView) => {
    setBusy(s.id);
    try {
      const r = await client.useSubscription(s.id);
      setView(r.subscriptions);
      toast({ title: r.done, description: r.restart_required ? "Ghost picks it up at its next start." : undefined, tone: "ok" });
      onUsed?.();
    } catch (e) {
      toast({ title: `Couldn’t use ${s.title}`, description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const cancel = async (s: SubscriptionView) => {
    try {
      withJob(s.id, await client.cancelSubscription(s.id));
    } catch (e) {
      toast({ title: "Couldn’t cancel it", description: errorText(e), tone: "danger" });
    }
  };

  if (!view) {
    return error ? (
      <Notice tone="danger" title="Ghost couldn’t ask your programs">
        {error}
      </Notice>
    ) : (
      <Card aria-hidden padding="md">
        <Skeleton height={14} width={260} />
      </Card>
    );
  }

  return (
    <div className="set-subs">
      {dialog}
      <div className="set-subs__head">
        <p className="set-note" aria-live="polite">
          {inUseLine(view.subscriptions, otherProvider)}
        </p>
        <Button size="sm" variant="ghost" leadingIcon={<RefreshCw />} loading={looking} disabled={!online} onClick={() => void look()}>
          Look again
        </Button>
      </div>
      <Card list>
        {view.subscriptions.map((s) => (
          <SubscriptionCard
            key={s.id}
            s={s}
            busy={busy === s.id}
            online={online}
            mock={mock}
            onInstall={() => void install(s)}
            onSignIn={() => void signIn(s)}
            onUse={() => void use(s)}
            onLook={() => void look()}
            onCancel={() => void cancel(s)}
            onJob={(j) => withJob(s.id, j)}
          />
        ))}
      </Card>
      <p className="set-help is-readable">
        Ghost runs your own program, signed in to your own account, for its own short turns. It never reads or keeps your sign-in, and each program is used as its makers allow.
      </p>
    </div>
  );
}

function SubscriptionCard({
  s,
  busy,
  online,
  mock,
  onInstall,
  onSignIn,
  onUse,
  onLook,
  onCancel,
  onJob,
}: {
  s: SubscriptionView;
  busy: boolean;
  online: boolean;
  mock: boolean;
  onInstall: () => void;
  onSignIn: () => void;
  onUse: () => void;
  onLook: () => void;
  onCancel: () => void;
  onJob: (j: SubscriptionJobView) => void;
}) {
  const face = cardFace(s);
  const click = { install: onInstall, sign_in: onSignIn, terminal: onSignIn, look: onLook, use: onUse, fix: onLook } as Partial<Record<typeof face.action, () => void>>;
  const button =
    face.label &&
    (face.action === "coming_next" ? (
      <Button size="sm" variant="secondary" disabled>
        {face.label}
      </Button>
    ) : (
      <Button size="sm" variant={face.action === "use" || face.action === "install" || face.action === "sign_in" || face.action === "terminal" ? "primary" : "secondary"} loading={busy} disabled={!online} onClick={click[face.action]}>
        {face.label}
      </Button>
    ));
  const showJob = !!s.job && (jobRunning(s.job) || s.job.state === "failed");
  return (
    <div className={s.in_use ? "set-subs__card is-in-use" : "set-subs__card"}>
      <ListRow
        leading={<Terminal aria-hidden />}
        title={
          <>
            {s.title} {s.version && <span className="set-detail">{s.version}</span>}
          </>
        }
        description={
          <>
            {face.line}
            {s.note && <span className="set-help is-readable"> {s.note}</span>}
          </>
        }
        meta={
          <Badge tone={face.tone === "neutral" ? undefined : face.tone} size="sm" dot={face.tone === "ok"}>
            {face.badge}
          </Badge>
        }
        trailing={button || undefined}
      />
      {(face.action === "terminal" || face.action === "look") && <CommandLine command={s.sign_in_command} />}
      {showJob && s.job && <JobPanel s={s} job={s.job} mock={mock} online={online} onCancel={onCancel} onJob={onJob} />}
    </div>
  );
}

function CommandLine({ command }: { command: string }) {
  const [copied, copy] = useCopy();
  return (
    <div className="set-subs__job">
      <p className="set-subs__command">
        {command}{" "}
        <Button size="sm" variant="ghost" leadingIcon={<Copy />} onClick={() => copy(command)}>
          {copied ? "Copied" : "Copy"}
        </Button>
      </p>
    </div>
  );
}

function JobPanel({ s, job, mock, online, onCancel, onJob }: { s: SubscriptionView; job: SubscriptionJobView; mock: boolean; online: boolean; onCancel: () => void; onJob: (j: SubscriptionJobView) => void }) {
  const { client } = useGhost();
  const toast = useToast();
  const [code, setCode] = useState("");
  const [sending, setSending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [copied, copy] = useCopy();
  const running = jobRunning(job);
  const send = async () => {
    const p = codeProblem(code);
    if (p) return setProblem(p);
    setSending(true);
    setProblem(null);
    const typed = code;
    setCode(""); // never kept on screen once it is sent
    try {
      onJob(await client.subscriptionCode(s.id, typed));
    } catch (e) {
      toast({ title: "Couldn’t send the code", description: errorText(e), tone: "danger" });
    } finally {
      setSending(false);
    }
  };
  const lines = job.lines.slice(-12);
  return (
    <div className="set-subs__job" aria-live="polite">
      {job.kind === "sign_in" && running && job.url && (
        <p className="set-help is-readable">
          The sign-in opens in your browser. If it didn’t,{" "}
          <Button size="sm" variant="secondary" leadingIcon={<ExternalLink />} onClick={() => void openSignIn(job.url ?? "", { again: true, mock })}>
            Open the sign-in page
          </Button>
        </p>
      )}
      {job.device_code && running && (
        <p className="set-help is-readable">
          Type this code on the page: <Kbd>{job.device_code}</Kbd>{" "}
          <Button size="sm" variant="ghost" leadingIcon={<Copy />} onClick={() => copy(job.device_code ?? "")}>
            {copied ? "Copied" : "Copy"}
          </Button>
        </p>
      )}
      {job.state === "needs_code" && (
        <form
          className="set-subs__code"
          onSubmit={(e) => {
            e.preventDefault();
            void send();
          }}
          autoComplete="off"
        >
          <label className="set-label" htmlFor={`sub-code-${s.id}`}>
            Paste the code
          </label>
          <p className="set-help">If the page in your browser shows a code, paste it here. Ghost gives it to {s.title} and keeps nothing.</p>
          <div className="set-subs__code-row">
            <Input id={`sub-code-${s.id}`} type="password" mono value={code} onChange={(e) => setCode(e.target.value)} spellCheck={false} autoComplete="off" invalid={!!problem} />
            <Button size="sm" type="submit" loading={sending} disabled={!online || !code.trim()}>
              Send
            </Button>
          </div>
          {problem && <p className="set-help t-danger">{problem}</p>}
        </form>
      )}
      {lines.length > 0 && (
        <pre className="set-subs__lines" aria-label={`What ${s.program} says`}>
          {lines.join("\n")}
        </pre>
      )}
      {job.message && !running && <p className="set-help t-danger">{job.message}</p>}
      {running && (
        <p>
          <Button size="sm" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        </p>
      )}
    </div>
  );
}
