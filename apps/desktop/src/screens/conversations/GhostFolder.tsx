/**
 * Owner: conversations. What every agent shares, under the list on #/agents (ADR 0035):
 * Ghost's rules (AGENTS.md, in every agent's prompt as written) with Add rule and Open
 * folder, and what Ghost made in out/ with Reveal and Open. A rule added here is the
 * operator's own click: POST /rules appends it verbatim through the gate, audited; the
 * permissions can still block it (409), and the screen shows their reason as the daemon
 * gave it. The screen never writes AGENTS.md itself.
 */
import type { OutputInfo, RulesView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { FolderOpen, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Button, Card, Dialog, Field, Notice, Section, Skeleton, Textarea, errorText, quoted, relativeTime, useLive, useToast } from "../../ui";
import { CONFIG_DIR, folderOf, formatBytes, openFile, revealFolder } from "../watching/model";
import { readParam, writeParam } from "./url";

type Opened = { ok: true } | { ok: false; path: string };

export function GhostFolder() {
  const { client, conversationsVersion } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  /** Ghost's AGENTS.md (GET /rules); null while it loads. */
  const [rules, setRules] = useState<RulesView | null>(null);
  const [rulesError, setRulesError] = useState<string | null>(null);
  /** What Ghost made in out/ (GET /outputs); null while it loads. */
  const [outputs, setOutputs] = useState<OutputInfo[] | null>(null);
  const [outputsError, setOutputsError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  // ?open=rule opens Add rule on load, for screenshots of the dialog.
  const [ruling, setRuling] = useState(() => readParam("open") === "rule");
  const [rule, setRule] = useState("");
  const [busy, setBusy] = useState(false);
  /** Why the rule didn't go in: the daemon's own words (a 409 from the permissions). */
  const [refused, setRefused] = useState<string | null>(null);
  const ruleRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    let on = true;
    client
      .rules()
      .then((r) => on && (setRules(r), setRulesError(null)))
      .catch((e) => on && setRulesError(errorText(e)));
    client
      .outputs()
      .then((o) => on && (setOutputs(o), setOutputsError(null)))
      .catch((e) => on && setOutputsError(errorText(e)));
    return () => {
      on = false;
    };
  }, [client, conversationsVersion, attempt]);

  useEffect(() => {
    if (!ruling) return;
    setRefused(null);
    const id = requestAnimationFrame(() => ruleRef.current?.focus());
    return () => cancelAnimationFrame(id);
  }, [ruling]);

  const closeRule = () => {
    setRuling(false);
    writeParam("open", null);
  };

  const openFolder = async () => {
    const r: Opened = rules ? await revealFolder(folderOf(rules.path)) : { ok: false, path: CONFIG_DIR };
    if (!r.ok) toast({ title: "Ghost’s folder", description: `${r.path} (open it in your file manager; this build can’t open folders itself).` });
  };

  /** Reveal or open a file Ghost made; a browser build shows the path instead. */
  const showFile = async (path: string, how: (p: string) => Promise<Opened>) => {
    const r = await how(path);
    if (!r.ok) toast({ title: "Open it yourself", description: `${r.path} (this build can’t open files itself).` });
  };

  const addRule = async () => {
    const text = rule.trim();
    if (!text || busy || !online) return;
    setBusy(true);
    setRefused(null);
    try {
      const out = await client.addRule(text);
      if (out.status !== "success") {
        setRefused(out.error || "Ghost didn’t write it. Nothing changed.");
        return;
      }
      closeRule();
      setRule("");
      toast({ title: "Rule added", description: "Appended to AGENTS.md as written. Every agent follows it from its next turn, and it’s in the audit log.", tone: "ok" });
      setRules(await client.rules().catch(() => rules));
    } catch (e) {
      // 409: the permissions block rules.add, or the text can't be a rule. Said as the daemon says it.
      setRefused(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const text = rules?.text.trim() ?? "";

  return (
    <div className="conv-folder">
      <Section
        id="conv-rules"
        title="Rules"
        description={
          <>
            Ghost’s AGENTS.md, in every agent’s prompt, as written. Say {quoted("add a rule: always say the currency")} in a session, or add one here; Ghost appends your words and never rewrites them.
          </>
        }
        actions={
          <>
            <Button size="sm" variant="ghost" leadingIcon={<FolderOpen />} onClick={() => void openFolder()}>
              Open folder
            </Button>
            <Button size="sm" variant="secondary" leadingIcon={<Plus />} disabled={!online || !rules} onClick={() => setRuling(true)}>
              Add rule
            </Button>
          </>
        }
      >
        {rulesError && !rules ? (
          <Notice tone="danger" title="Couldn’t load the rules" action={<Button size="sm" onClick={() => setAttempt((n) => n + 1)}>Try again</Button>}>
            {rulesError}
          </Notice>
        ) : !rules ? (
          <Skeleton height={96} radius="var(--radius-lg)" />
        ) : (
          <Card padding="none">
            <pre className={text ? "conv-rules" : "conv-rules conv-rules--empty"} data-user-content={text ? true : undefined}>
              {text || "No rules yet. Add one here, or write AGENTS.md in Ghost’s folder."}
            </pre>
          </Card>
        )}
      </Section>

      <Section id="conv-outputs" title="Outputs" description="What Ghost made for you, in out/ in its folder. Ghost never runs these; open them yourself.">
        {outputsError && !outputs ? (
          <Notice tone="danger" title="Couldn’t load what Ghost made" action={<Button size="sm" onClick={() => setAttempt((n) => n + 1)}>Try again</Button>}>
            {outputsError}
          </Notice>
        ) : !outputs ? (
          <Skeleton height={56} radius="var(--radius-lg)" />
        ) : (
          <Card list>
            {outputs.length === 0 ? (
              <p className="conv-rules conv-rules--empty">Nothing yet. Ask Ghost to make a page, a summary or a table, and it lands here.</p>
            ) : (
              outputs.map((o) => (
                <div key={o.name} className="conv-output">
                  <span className="conv-output__text">
                    <span className="t-mono">{o.name}</span> <span className="t-3">{formatBytes(o.bytes)} · {relativeTime(o.modified)}</span>
                  </span>
                  <span className="conv-output__actions">
                    <Button size="sm" variant="ghost" onClick={() => void showFile(o.path, revealFolder)}>
                      Reveal
                    </Button>
                    <Button size="sm" variant="secondary" onClick={() => void showFile(o.path, openFile)}>
                      Open
                    </Button>
                  </span>
                </div>
              ))
            )}
          </Card>
        )}
      </Section>

      <Dialog
        open={ruling}
        onClose={closeRule}
        dismissible={!busy}
        size="md"
        title="Add a rule"
        description="Appended to Ghost’s AGENTS.md exactly as written, and followed by every agent from its next turn. The audit log records it."
        footer={
          <>
            <Button variant="ghost" onClick={closeRule} disabled={busy}>
              Cancel
            </Button>
            <Button variant="primary" type="submit" form="conv-rule" loading={busy} disabled={!rule.trim() || !online}>
              Add the rule
            </Button>
          </>
        }
      >
        <form
          id="conv-rule"
          onSubmit={(e) => {
            e.preventDefault();
            void addRule();
          }}
        >
          <Field label="Rule" help="In your words. Ghost never paraphrases it." error={refused ? `Ghost didn’t add it: ${refused}` : undefined}>
            <Textarea
              ref={ruleRef}
              rows={3}
              value={rule}
              placeholder="Always say the currency with an amount."
              maxLength={1000}
              onChange={(e) => {
                setRule(e.target.value);
                setRefused(null);
              }}
            />
          </Field>
        </form>
      </Dialog>
    </div>
  );
}
