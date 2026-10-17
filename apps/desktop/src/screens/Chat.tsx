import { phaseLabel } from "@ghost/ui";
import { useGhost } from "@ghost/ui";
import { useEffect, useRef, useState } from "react";

/** The session view: status, the pending-change card, the transcript, and a text input.
 *  A typed turn goes through the same pipeline, tools and gate as speech. */
export function Chat() {
  const { client, status, connected, turns, pending } = useGhost();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const bottom = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: "smooth" });
  }, [turns.length]);

  const run = async (f: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const send = () => {
    const t = text.trim();
    if (!t) return;
    setText("");
    void run(() => client.say(t));
  };

  if (!connected) {
    return (
      <>
        <h1>Ghost</h1>
        <div className="panel">
          <p>ghostd is not running. Start it with <span className="mono">systemctl --user start ghostd</span> or <span className="mono">make run</span>.</p>
        </div>
      </>
    );
  }

  const inSession = !!status?.session_id;
  return (
    <>
      <h1>Ghost</h1>
      <div className="panel row">
        <span className={`dot ${status?.phase}`} />
        <strong>{status ? phaseLabel(status.phase) : ""}</strong>
        {!status?.models_ready && <span className="muted">loading models…</span>}
        <span className="spacer" style={{ flex: 1 }} />
        {inSession ? (
          <button className="ghost" disabled={busy} onClick={() => run(() => client.sleep())}>
            Go to sleep
          </button>
        ) : (
          <button className="primary" disabled={busy || !status?.models_ready} onClick={() => run(() => client.wake())}>
            Wake
          </button>
        )}
      </div>

      {pending && (
        <div className="panel card" style={{ marginTop: 12 }}>
          <div className="muted" style={{ fontSize: 12 }}>
            Waiting for your okay · {pending.ability}.{pending.operation} · expires in {Math.round(pending.expires_in_secs)}s
          </div>
          <p style={{ margin: "6px 0 10px" }}>{pending.summary}</p>
          {pending.rationale && <p className="muted" style={{ marginTop: 0 }}>{pending.rationale}</p>}
          <div className="row">
            <button className="primary" disabled={busy} onClick={() => run(() => client.confirm())}>
              Confirm
            </button>
            <button className="ghost" disabled={busy} onClick={() => run(() => client.cancel())}>
              Cancel
            </button>
          </div>
        </div>
      )}

      <h2>Conversation</h2>
      <div className="panel">
        <div className="turns">
          {turns.length === 0 && <div className="muted">Nothing yet. Say "Hey Ghost" or type below.</div>}
          {turns.map((t, i) => (
            <div key={i} className={`turn ${t.role}`}>
              {t.text}
              <div className="meta">
                {t.role === "user" ? (t.via === "text" ? "typed" : "spoken") : "Ghost"} · {new Date(t.at).toLocaleTimeString()}
              </div>
            </div>
          ))}
          <div ref={bottom} />
        </div>
        <div className="row" style={{ marginTop: 10 }}>
          <input
            style={{ flex: 1 }}
            placeholder={inSession ? "Type to Ghost…" : "Type to wake Ghost with a request…"}
            value={text}
            disabled={busy || !status?.models_ready}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && send()}
          />
          <button className="primary" disabled={busy || !text.trim()} onClick={send}>
            Send
          </button>
          <button className="ghost" disabled={busy || !inSession} title="Undo the last change" onClick={() => run(() => client.undo())}>
            Undo
          </button>
        </div>
        {error && <p style={{ color: "var(--bad)" }}>{error}</p>}
      </div>
    </>
  );
}
