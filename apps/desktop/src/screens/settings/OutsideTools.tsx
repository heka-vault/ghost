/**
 * Owner: settings. The Outside tools page (ADR 0048): the tools on this computer the operator
 * connected to Ghost (Claude Code, Cursor, and the like), when each last reached Ghost, and
 * connecting and removing one.
 *
 * Connecting is a short focused task, so it is a Dialog in two steps. First the tool's name,
 * which is what Ghost says when something comes from it. Then, once, its token and what to
 * paste into the tool, each with Copy: the token is a password, so Ghost keeps only a
 * fingerprint of it and can never show it again, and the dialog says so before it closes.
 * Removing asks first; the tool is refused from its next request.
 *
 * Mock states: any scenario; one tool is connected, two when the data is `busy`, none when
 * it is `empty`.
 */
import type { ClientCreated, ClientsView, ClientView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Check, Copy, PlugZap } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";

import { Button, Card, Checkbox, Dialog, Field, IconButton, Input, ListRow, Notice, Section, Skeleton, errorText, relativeTime, useConfirm, useLive, useToast } from "../../ui";
import { nameProblem, toolName, toolTitle } from "./tools";

export function OutsideToolsSection() {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog: confirmDialog } = useConfirm();
  const [view, setView] = useState<ClientsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const connectButton = useRef<HTMLButtonElement>(null);

  const load = useCallback(async () => {
    try {
      setView(await client.clients());
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [client]);
  useEffect(() => {
    void load();
  }, [load]);

  const remove = async (tool: ClientView) => {
    const ok = await confirm({
      title: `Remove ${tool.title}?`,
      description: `Ghost stops answering ${tool.title} at once. To use it again, connect it again and paste the new token into it.`,
      confirmLabel: `Remove ${tool.title}`,
      tone: "danger",
    });
    if (!ok) return;
    setRemoving(tool.name);
    try {
      setView(await client.removeClient(tool.name));
      toast({ title: `${tool.title} removed`, description: "Its token no longer works." });
      requestAnimationFrame(() => connectButton.current?.focus({ preventScroll: true }));
    } catch (e) {
      toast({ title: `Couldn’t remove ${tool.title}`, description: errorText(e), tone: "danger" });
    } finally {
      setRemoving(null);
    }
  };

  const tools = view?.clients ?? [];
  return (
    <Section id="set-tools" className="set-section">
      {confirmDialog}
      {!view && error && (
        <Notice tone="danger" title="Couldn’t check the outside tools" action={<Button size="sm" onClick={() => void load()}>Try again</Button>}>
          {error}
        </Notice>
      )}
      {!view && !error && (
        <Card list>
          <div className="set-row set-skeleton__row" aria-busy="true" aria-label="Loading the outside tools">
            <div className="set-skeleton__text">
              <Skeleton width={140} height={12} />
              <Skeleton width={260} height={10} />
            </div>
            <Skeleton width={104} height={28} radius="var(--radius-md)" />
          </div>
        </Card>
      )}

      {view?.problem && (
        <Notice tone="warn" title="Ghost isn’t listening for outside tools">
          {view.problem}
        </Notice>
      )}
      {view && !view.problem && view.listening.length === 0 && (
        <Notice tone="neutral" title="Listening for outside tools is off">
          A tool you connect can’t reach Ghost until it is on. Remove the empty <span className="set-path">GHOST_MCP_LISTEN</span> line from Ghost’s settings file, then restart Ghost.
        </Notice>
      )}

      {view && (
        <Card list>
          {tools.length === 0 && (
            <ListRow
              leading={<PlugZap size={16} aria-hidden className="set-phone__icon" />}
              title="No tools connected"
              description="Connect a tool like Claude Code or Cursor, and it can tell you things through Ghost: by voice at your desk, on the waiting list, or on your phone."
              trailing={
                <Button ref={connectButton} size="sm" variant="secondary" disabled={!online} onClick={() => setConnecting(true)}>
                  Connect a tool
                </Button>
              }
            />
          )}
          {tools.map((t) => (
            <ListRow
              key={t.name}
              leading={<PlugZap size={16} aria-hidden className="set-phone__icon" />}
              title={t.title}
              description={
                <>
                  Connected <time dateTime={t.created_at}>{relativeTime(t.created_at)}</time> ·{" "}
                  {t.last_used_at ? (
                    <>
                      last used <time dateTime={t.last_used_at}>{relativeTime(t.last_used_at)}</time>
                    </>
                  ) : (
                    "not used yet"
                  )}
                </>
              }
              trailing={
                <Button size="sm" variant="danger-ghost" loading={removing === t.name} disabled={!online} onClick={() => void remove(t)}>
                  Remove
                </Button>
              }
            />
          ))}
          {tools.length > 0 && (
            <ListRow
              title="Connect another tool"
              description="Each tool has its own name and token, so you hear who a message is from and can remove one without the others."
              trailing={
                <Button ref={connectButton} size="sm" variant="secondary" disabled={!online} onClick={() => setConnecting(true)}>
                  Connect a tool
                </Button>
              }
            />
          )}
        </Card>
      )}

      {view && (
        <p className="set-help is-readable">
          A connected tool can read Ghost’s status and documentation, put a message on your waiting list, and ask Ghost to say one sentence at your desk. It can’t read your passwords,
          change a permission, or hear anything. What it may change follows your permissions for <span className="set-path">ghost</span>, and every change is in the audit log under the
          tool’s name.
        </p>
      )}

      <ConnectDialog
        open={connecting}
        taken={tools.map((t) => t.name)}
        onClose={() => {
          setConnecting(false);
          void load();
        }}
        onConnected={(v) => setView(v)}
      />
    </Section>
  );
}

/** Text to paste elsewhere, with Copy beside its label; long text wraps instead of scrolling. */
function CopyBlock({ label, text, multiline }: { label: string; text: string; multiline?: boolean }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <div className="set-tools__copy">
      <div className="set-tools__copy-head">
        <span className="set-tools__copy-label">{label}</span>
        <IconButton
          size="sm"
          label={copied ? "Copied" : `Copy ${label.toLowerCase()}`}
          icon={copied ? <Check /> : <Copy />}
          onClick={() => {
            navigator.clipboard?.writeText(text).then(
              () => setCopied(true),
              () => {},
            );
          }}
        />
      </div>
      {multiline ? <pre className="set-tools__code">{text}</pre> : <code className="set-tools__code">{text}</code>}
    </div>
  );
}

function ConnectDialog({ open, taken, onClose, onConnected }: { open: boolean; taken: string[]; onClose: () => void; onConnected: (v: ClientsView) => void }) {
  const { client } = useGhost();
  const [typed, setTyped] = useState("");
  const [command, setCommand] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [made, setMade] = useState<ClientCreated | null>(null);

  useEffect(() => {
    if (!open) return;
    setTyped("");
    setCommand(false);
    setError(null);
    setMade(null);
  }, [open]);

  const problem = nameProblem(typed, taken);
  const name = toolName(typed);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!name || problem || busy) return;
    setBusy(true);
    setError(null);
    try {
      const m = await client.connectClient(name, command);
      setMade(m);
      onConnected(m.clients);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  if (made) {
    return (
      <Dialog
        open={open}
        onClose={onClose}
        size="md"
        title={`${made.client.title} is connected`}
        description="Paste one of these into the tool now. Ghost can’t show the token again: it keeps only a fingerprint of it."
        footer={
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        }
      >
        <div className="set-tools__made">
          <CopyBlock label="For Claude Code, run" text={made.claude_code} />
          <CopyBlock label="For Cursor and other tools, add to their config" text={made.config_json} multiline />
          <CopyBlock label="Token" text={made.token} />
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="sm"
      title="Connect a tool"
      description="A tool on this computer that should be able to reach you through Ghost."
      initialFocus="#set-tools-name"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="set-tools-form" loading={busy} disabled={!name || !!problem}>
            Connect
          </Button>
        </>
      }
    >
      <form id="set-tools-form" className="set-tools__form" onSubmit={(e) => void submit(e)}>
        <Field
          id="set-tools-name"
          label="Name"
          help={problem ?? (name && name !== typed.trim() ? `Ghost will say “From ${toolTitle(name)}”.` : "What Ghost says a message is from.")}
          error={problem ?? undefined}
        >
          <Input value={typed} onChange={(e) => setTyped(e.target.value)} placeholder="Claude Code" autoComplete="off" spellCheck={false} maxLength={60} />
        </Field>
        <Checkbox
          checked={command}
          onChange={setCommand}
          label="This tool can only start a command"
          description="Ghost keeps the token in a private file and gives the tool a command to run instead of an address."
        />
        {error && (
          <Notice tone="danger" title="Couldn’t connect it">
            {error}
          </Notice>
        )}
      </form>
    </Dialog>
  );
}
