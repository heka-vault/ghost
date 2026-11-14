/**
 * Owner: apps. The AI tools already on this computer (ADR 0068): the apps Claude Code, Cursor
 * and Codex use, each with a Connect, and a way to let each of them use Ghost. Shown in the
 * first run's app step, on Apps, and (the links alone) in Settings › Outside tools.
 *
 * Everything here is read from another tool's files, so it is someone else's text: shown as
 * data (`data-user-content`), a command exactly as it would run, in mono, never run before the
 * operator's click. Nothing is shown when no tool is there, so a newcomer without one sees
 * exactly what they saw before.
 */
import type { FoundAppView, OtherToolsView, OtherToolView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Check, RefreshCw, Terminal } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { useDesign } from "../../design/context";
import { Badge, Button, Card, errorText, ListRow, Mascot, Notice, useLive } from "../../ui";
import { AppIcon } from "./AppIcon";
import "./OtherTools.css";
import { expectSignIn, openSignIn } from "./signin";

/** What the tools on this computer have set up, read when the screen opens and on Look again. */
export function useOtherTools() {
  const { client } = useGhost();
  const { online } = useLive();
  const [view, setView] = useState<OtherToolsView | null>(null);
  const [looking, setLooking] = useState(false);
  const look = useCallback(async () => {
    setLooking(true);
    try {
      setView(await client.otherTools());
    } catch {
      setView(null);
    } finally {
      setLooking(false);
    }
  }, [client]);
  useEffect(() => {
    if (online) void look();
  }, [online, look]);
  return { view, setView, look, looking };
}

/** “Cursor and Claude Code are set up on this computer.”, with Wisp looking up, and Look again. */
export function NoticedLine({ view, look, looking }: { view: OtherToolsView; look: () => void; looking: boolean }) {
  if (!view.line) return null;
  return (
    <div className="other-noticed">
      <Mascot size={28} mood="listening" className="other-noticed__wisp" />
      <p className="other-noticed__line">{view.line}</p>
      <Button size="sm" variant="ghost" leadingIcon={<RefreshCw />} loading={looking} onClick={look}>
        Look again
      </Button>
    </div>
  );
}

function describe(a: FoundAppView) {
  const facts = [
    a.scope !== "every project" ? `Used in ${a.scope}` : null,
    a.sign_in ? "You sign in through Ghost" : null,
    a.moved.length ? "Its key goes into Ghost’s vault" : null,
    a.needs.length ? `Needs the password ${a.needs.join(", ")}` : null,
    ...a.notes,
  ].filter(Boolean);
  return (
    <>
      {a.kind === "command" ? (
        <span className="other-command" data-user-content>
          <Terminal size={12} aria-hidden /> <span className="t-mono">{a.command_line}</span>
        </span>
      ) : (
        <span className="t-mono" data-user-content>
          {a.address}
        </span>
      )}
      {(a.skip ?? facts.join(" · ")) && <span className="other-facts">{a.skip ?? facts.join(" · ")}</span>}
    </>
  );
}

/** One tool's apps, and letting it use Ghost. */
function ToolApps({ tool, onView, links }: { tool: OtherToolView; onView: (v: OtherToolsView) => void; links: boolean }) {
  const { client } = useGhost();
  const design = useDesign();
  const mock = import.meta.env.DEV && !!design;
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [showLink, setShowLink] = useState(false);

  const connect = async (a: FoundAppView) => {
    setBusy(a.key);
    setError(null);
    try {
      expectSignIn(a.suggested);
      const r = await client.importOtherApps(tool.id, [a.key]);
      if (r.sign_in_url) await openSignIn(r.sign_in_url, { mock });
      onView(r.tools);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };
  const link = async () => {
    setBusy("link");
    setError(null);
    try {
      const r = await client.linkOtherTool(tool.id);
      setDone(r.backup ? `${r.done} The file as it was is in ${r.backup}.` : r.done);
      setShowLink(false);
      onView(r.tools);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const problems = tool.files.map((f) => f.problem).filter((p): p is string => !!p);
  const apps = links ? [] : tool.apps;
  if (!apps.length && !tool.installed && !problems.length) return null;
  return (
    <section className="other-tool" aria-label={`Found in ${tool.title}`}>
      <h3 className="other-tool__title">Found in {tool.title}</h3>
      {problems.map((p) => (
        <Notice key={p} tone="info" title={`Ghost couldn’t read ${tool.title}’s apps`}>
          {p}
        </Notice>
      ))}
      {error && (
        <Notice tone="danger" title="Couldn’t do that" onDismiss={() => setError(null)}>
          {error}
        </Notice>
      )}
      <Card list>
        {apps.map((a) => (
          <ListRow
            key={a.key}
            align="start"
            leading={<AppIcon title={a.name} icon={null} />}
            title={<span data-user-content>{a.name}</span>}
            description={describe(a)}
            trailing={
              a.skip ? (
                a.skip.startsWith("Already connected") ? (
                  <Badge tone="ok" dot>
                    Connected
                  </Badge>
                ) : null
              ) : (
                <Button size="sm" variant="secondary" loading={busy === a.key} disabled={!!busy && busy !== a.key} onClick={() => connect(a)}>
                  {a.sign_in ? "Sign in…" : a.kind === "command" ? "Connect and run" : "Connect"}
                </Button>
              )
            }
          />
        ))}
        {tool.uses_ghost ? (
          <ListRow
            leading={<Check className="t-ok" size={16} aria-hidden />}
            title={`${tool.title} uses Ghost`}
            description={done ?? (tool.uses_ghost.client ? `As the outside tool ${tool.uses_ghost.client}.` : undefined)}
          />
        ) : tool.link ? (
          <ListRow
            align="start"
            title={`Let ${tool.title} use Ghost`}
            description={
              showLink ? (
                <>
                  <span className="other-facts">
                    {tool.link.by === "command" ? `Ghost runs ${tool.title}’s own command:` : "Ghost backs the file up beside itself, then adds Ghost to it:"}
                  </span>
                  <pre className="other-link t-mono" data-user-content>
                    {tool.link.shown}
                  </pre>
                  {tool.link.by_address && <span className="other-facts">“&lt;token&gt;” is a token of its own, made when you press the button and never shown.</span>}
                </>
              ) : (
                `So ${tool.title} can tell you things through Ghost and ask your agents.`
              )
            }
            trailing={
              showLink ? (
                <Button size="sm" loading={busy === "link"} onClick={link}>
                  Add Ghost to {tool.title}
                </Button>
              ) : (
                <Button size="sm" variant="secondary" onClick={() => setShowLink(true)}>
                  Show what changes…
                </Button>
              )
            }
          />
        ) : (
          tool.link_problem && <ListRow title={`Let ${tool.title} use Ghost`} description={<span className="other-facts">{tool.link_problem.split("\n")[0]}</span>} />
        )}
      </Card>
    </section>
  );
}

/**
 * The tools found, each with its apps (unless `links`, which shows only letting them use Ghost).
 * Renders nothing when no tool is here.
 */
export function OtherToolsApps({ view, onView, links = false }: { view: OtherToolsView | null; onView: (v: OtherToolsView) => void; links?: boolean }) {
  if (!view || !view.tools.length) return null;
  return (
    <div className="other-tools">
      {view.tools.map((t) => (
        <ToolApps key={t.id} tool={t} onView={onView} links={links} />
      ))}
    </div>
  );
}

/** “Claude Code”, “Claude Code and Codex”. */
const both = (names: string[]) => (names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`);

/**
 * A model connection that is already here (ADR 0068), for the first run's model step: a local
 * server answering on its own port, or a provider's key in the environment Ghost was started
 * in, each one click. Never another tool's sign-in, and the step says why in one sentence.
 */
export function FoundModels({ view, chosen, onChoose }: { view: OtherToolsView | null; chosen: string; onChoose: (provider: string, model: string | null) => void }) {
  if (!view || (!view.models.length && !view.own_sign_in.length)) return null;
  return (
    <div className="other-models">
      {view.models.length > 0 && (
        <Card list>
          {view.models.map((m) => (
            <ListRow
              key={m.provider}
              leading={<Mascot size={24} mood="listening" />}
              title={m.title}
              description={
                <>
                  {m.how}
                  {m.models.length > 0 && (
                    <span className="other-facts t-mono" data-user-content>
                      {m.models.slice(0, 4).join(", ")}
                      {m.models.length > 4 ? ", …" : ""}
                    </span>
                  )}
                </>
              }
              trailing={
                chosen === m.provider ? (
                  <Badge tone="ok" dot>
                    Chosen
                  </Badge>
                ) : (
                  <Button size="sm" variant="secondary" onClick={() => onChoose(m.provider, m.models[0] ?? null)}>
                    Use {m.title}
                  </Button>
                )
              }
            />
          ))}
        </Card>
      )}
      {view.own_sign_in.length > 0 && (
        <p className="other-facts other-models__note">
          {both(view.own_sign_in)} {view.own_sign_in.length > 1 ? "are" : "is"} installed. {view.own_sign_in.length > 1 ? "Their sign-ins belong" : "Its sign-in belongs"} to{" "}
          {view.own_sign_in.length > 1 ? "them" : "it"}, so Ghost can’t use {view.own_sign_in.length > 1 ? "them" : "it"}: give Ghost a key, sign in to OpenRouter, or use a model on this computer.
        </p>
      )}
    </div>
  );
}
