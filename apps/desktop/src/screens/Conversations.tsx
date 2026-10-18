import type { ConversationDetail, ConversationView, TurnView, VoiceInfo } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

/** Saved conversations (ADR 0014): list and switch, pick a voice and hear it, edit the
 *  persona, memory and notes Ghost writes, reset, delete, compact. Every edit here is
 *  an audited, undoable write to the same files Ghost uses. */
export function Conversations() {
  const { client, status, conversationsVersion } = useGhost();
  const [list, setList] = useState<ConversationView[]>([]);
  const [voices, setVoices] = useState<VoiceInfo[]>([]);
  const [slug, setSlug] = useState<string>("ghost");
  const [detail, setDetail] = useState<ConversationDetail | null>(null);
  const [turns, setTurns] = useState<TurnView[]>([]);
  const [draft, setDraft] = useState({ name: "", aliases: "", voice: "", speed: 1, persona: "", memory: "", notes: "" });
  const [newName, setNewName] = useState("");
  const [note, setNote] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const inSession = !!status?.session_id;
  const active = status?.conversation ?? null;

  const loadList = async () => {
    const [l, v] = await Promise.all([client.conversations(), client.voices().catch(() => [])]);
    setList(l);
    setVoices(v);
    if (!l.find((c) => c.slug === slug)) setSlug("ghost");
  };
  const loadDetail = async (s: string) => {
    const [d, t] = await Promise.all([client.conversation(s), client.conversationTurns(s, 100)]);
    setDetail(d);
    setTurns(t);
    setDraft({
      name: d.name,
      aliases: d.aliases.join(", "),
      voice: d.voice,
      speed: d.speed,
      persona: d.persona,
      memory: d.memory,
      notes: d.notes.join("\n"),
    });
  };
  useEffect(() => {
    loadList().catch((e) => setMsg(String(e)));
  }, [client, conversationsVersion, status?.models_ready]);
  useEffect(() => {
    loadDetail(slug).catch((e) => setMsg(String(e)));
  }, [client, slug, conversationsVersion]);

  const run = async (f: () => Promise<unknown>, ok?: string) => {
    setBusy(true);
    setMsg(null);
    try {
      await f();
      if (ok) setMsg(ok);
      await loadList();
      await loadDetail(slug);
    } catch (e) {
      setMsg(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    run(async () => {
      if (!detail) return;
      const body: Parameters<typeof client.updateConversation>[1] = {};
      const aliases = draft.aliases.split(",").map((a) => a.trim()).filter(Boolean);
      if (!detail.is_ghost && draft.name.trim() !== detail.name) body.name = draft.name.trim();
      if (!detail.is_ghost && aliases.join("|") !== detail.aliases.join("|")) body.aliases = aliases;
      if (draft.voice !== detail.voice) body.voice = draft.voice;
      if (draft.speed !== detail.speed) body.speed = draft.speed;
      if (draft.persona !== detail.persona) body.persona = draft.persona;
      if (draft.memory !== detail.memory) body.memory = draft.memory;
      const notes = draft.notes.split("\n").map((n) => n.trim()).filter(Boolean);
      if (notes.join("|") !== detail.notes.join("|")) body.notes = notes;
      if (Object.keys(body).length === 0) return;
      await client.updateConversation(slug, body);
    }, "Saved. Each change is in the audit log and can be undone from a session.");

  const create = () =>
    run(async () => {
      const name = newName.trim();
      if (!name) return;
      const d = await client.createConversation({ name, aliases: [], voice: null, speed: null, persona: "" });
      setNewName("");
      setSlug(d.slug);
    });

  const label = (c: { slug: string; name: string }) => (c.slug === "ghost" ? "Ghost" : c.name);

  return (
    <>
      <h1>Conversations</h1>
      <p className="muted">
        Lines you can be patched through to: "patch me to accounting", "back to Ghost". Each keeps its own
        voice, persona and memory. {detail && <span className="mono">{detail.dir}</span>}
      </p>
      {msg && <p className="muted">{msg}</p>}

      <div className="grid2" style={{ gridTemplateColumns: "260px 1fr" }}>
        <div className="panel">
          {list.map((c) => (
            <div key={c.slug} className="row" style={{ padding: "6px 0", borderBottom: "1px solid var(--line)" }}>
              <button className={c.slug === slug ? "active" : "ghost"} style={{ flex: 1, textAlign: "left" }} onClick={() => setSlug(c.slug)}>
                {label(c)}
                {c.active && <span className="tag" style={{ marginLeft: 8 }}>live</span>}
              </button>
              <button
                className="ghost"
                title={inSession ? "Patch the session through to this conversation" : "Wake Ghost first"}
                disabled={busy || !inSession || c.slug === active}
                onClick={() => run(() => client.switchConversation(c.slug))}
              >
                Switch
              </button>
            </div>
          ))}
          <div className="row" style={{ marginTop: 10 }}>
            <input
              style={{ flex: 1 }}
              placeholder="New conversation name"
              value={newName}
              disabled={busy}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && create()}
            />
            <button className="primary" disabled={busy || !newName.trim()} onClick={create}>
              Start
            </button>
          </div>
        </div>

        {detail && (
          <div className="panel">
            <div className="row">
              <h2 style={{ margin: 0, flex: 1 }}>{label(detail)}</h2>
              <span className="muted">
                {detail.turn_count} turns · {detail.compactions} compactions
                {detail.last_active_at && ` · last ${new Date(detail.last_active_at).toLocaleString()}`}
              </span>
            </div>

            {!detail.is_ghost && (
              <div className="grid2" style={{ marginTop: 10 }}>
                <label>
                  <div className="muted">Name</div>
                  <input style={{ width: "100%" }} value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
                </label>
                <label>
                  <div className="muted">Also answers to (comma-separated)</div>
                  <input style={{ width: "100%" }} value={draft.aliases} onChange={(e) => setDraft({ ...draft, aliases: e.target.value })} />
                </label>
              </div>
            )}

            <div className="grid2" style={{ marginTop: 10 }}>
              <label>
                <div className="muted">Voice</div>
                <div className="row">
                  <select style={{ flex: 1 }} value={draft.voice} onChange={(e) => setDraft({ ...draft, voice: e.target.value })}>
                    {!voices.length && <option value={draft.voice}>{draft.voice}</option>}
                    {voices.map((v) => (
                      <option key={v.id} value={v.id}>
                        {v.label} ({v.id})
                      </option>
                    ))}
                  </select>
                  <button
                    className="ghost"
                    disabled={busy || !status?.models_ready}
                    title="Speak a sample through the speakers"
                    onClick={() => run(() => client.sampleVoice(draft.voice, draft.speed))}
                  >
                    Hear it
                  </button>
                </div>
              </label>
              <label>
                <div className="muted">Speed {draft.speed.toFixed(2)}x</div>
                <input type="range" min={0.5} max={2} step={0.05} style={{ width: "100%" }} value={draft.speed} onChange={(e) => setDraft({ ...draft, speed: Number(e.target.value) })} />
              </label>
            </div>

            <label style={{ display: "block", marginTop: 10 }}>
              <div className="muted">Persona, in your words (goes into the prompt; Ghost stores this verbatim)</div>
              <textarea rows={3} style={{ width: "100%" }} value={draft.persona} onChange={(e) => setDraft({ ...draft, persona: e.target.value })} />
            </label>
            <label style={{ display: "block", marginTop: 10 }}>
              <div className="muted">Memory: the rolling summary (rewritten by compaction; your edits are folded into the next one)</div>
              <textarea rows={6} style={{ width: "100%" }} value={draft.memory} onChange={(e) => setDraft({ ...draft, memory: e.target.value })} />
            </label>
            <label style={{ display: "block", marginTop: 10 }}>
              <div className="muted">Pinned notes, one per line (never summarized away)</div>
              <textarea rows={4} style={{ width: "100%" }} value={draft.notes} onChange={(e) => setDraft({ ...draft, notes: e.target.value })} />
            </label>
            <div className="row" style={{ marginTop: 6 }}>
              <input style={{ flex: 1 }} placeholder="Remember this…" value={note} onChange={(e) => setNote(e.target.value)} onKeyDown={(e) => e.key === "Enter" && note.trim() && run(() => client.addNote(slug, note.trim()).then(() => setNote("")))} />
            </div>

            <div className="row" style={{ marginTop: 14 }}>
              <button className="primary" disabled={busy} onClick={save}>
                Save
              </button>
              <button
                className="ghost"
                disabled={busy || !inSession || active !== slug}
                title="Ask the model to rewrite the memory now (needs the session to be in this conversation)"
                onClick={() => run(() => client.compactConversation(slug), "Asked Ghost to compact; the new memory appears when it finishes.")}
              >
                Compact now
              </button>
              <span className="spacer" style={{ flex: 1 }} />
              <button
                className="ghost"
                disabled={busy || active === slug}
                title="Forget memory, notes and transcript; keeps name and voice. Undoable for ten minutes from a session."
                onClick={() => confirm(`Reset ${label(detail)}? Memory, notes and transcript are cleared.`) && run(() => client.resetConversation(slug), "Reset.")}
              >
                Reset
              </button>
              {!detail.is_ghost && (
                <button
                  className="ghost"
                  disabled={busy || active === slug}
                  onClick={() => confirm(`Delete ${label(detail)}?`) && run(() => client.deleteConversation(slug).then(() => setSlug("ghost")), "Deleted.")}
                >
                  Delete
                </button>
              )}
            </div>

            <h2 style={{ marginTop: 18 }}>Transcript</h2>
            <div className="turns" style={{ maxHeight: "30vh" }}>
              {turns.length === 0 && <div className="muted">Nothing on record.</div>}
              {turns.map((t) => (
                <div key={t.id} className={`turn ${t.role === "user" ? "user" : "assistant"}`} style={{ opacity: t.compacted ? 0.55 : 1 }}>
                  {t.text}
                  <div className="meta">
                    {t.role} · {new Date(t.at).toLocaleString()}
                    {t.compacted && " · in memory"}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </>
  );
}
