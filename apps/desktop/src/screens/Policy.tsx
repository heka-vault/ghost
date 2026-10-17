import { type AbilityInfo, AUTONOMY_LEVELS, type PolicyView } from "@ghost/client-ts";
import { autonomyLabel, useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

/** Per-operation autonomy, written back to ~/.config/ghost/policy.toml. */
export function Policy() {
  const { client } = useGhost();
  const [abilities, setAbilities] = useState<AbilityInfo[]>([]);
  const [policy, setPolicy] = useState<PolicyView | null>(null);
  const [skills, setSkills] = useState<Record<string, Record<string, string>>>({});
  const [defaultLevel, setDefaultLevel] = useState<string>("");
  const [dirty, setDirty] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [raw, setRaw] = useState(false);
  const [text, setText] = useState("");

  const load = async () => {
    const [a, p] = await Promise.all([client.abilities(), client.policy()]);
    setAbilities(a.filter((x) => x.enabled));
    setPolicy(p);
    setSkills(p.skills);
    setDefaultLevel(p.default ?? "");
    setText(p.text);
    setDirty(false);
  };
  useEffect(() => {
    load().catch((e) => setMsg(String(e)));
  }, [client]);

  const set = (ability: string, op: string, level: string) => {
    setSkills((s) => {
      const next = { ...s, [ability]: { ...(s[ability] ?? {}) } };
      if (level === "") delete next[ability][op];
      else next[ability][op] = level;
      return next;
    });
    setDirty(true);
  };

  const save = async () => {
    setMsg(null);
    try {
      const r = raw
        ? await client.updatePolicy({ text })
        : await client.updatePolicy({ skills, default: defaultLevel || null });
      setMsg(r.restart_required ? "Saved. Restart ghostd for the enabled list to change." : "Saved and applied.");
      await load();
    } catch (e) {
      setMsg(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <>
      <h1>What Ghost may do on its own</h1>
      <p className="muted">
        Each operation has a level. Anything you leave on "ability default" falls back to the
        operation's risk class: low-risk ones announce, the rest ask. Every level still logs the
        write and lets you say "undo that". Stored in <span className="mono">{policy?.path}</span>.
      </p>
      <div className="row" style={{ marginBottom: 10 }}>
        <label className="row">
          Default for anything unlisted:
          <select value={defaultLevel} onChange={(e) => { setDefaultLevel(e.target.value); setDirty(true); }}>
            <option value="">risk class</option>
            {AUTONOMY_LEVELS.map((l) => (
              <option key={l} value={l}>{autonomyLabel(l)}</option>
            ))}
          </select>
        </label>
        <span style={{ flex: 1 }} />
        <button className="ghost" onClick={() => setRaw((r) => !r)}>{raw ? "Form" : "Edit TOML"}</button>
        <button className="primary" disabled={!dirty && !raw} onClick={save}>Save</button>
      </div>
      {msg && <p className="muted">{msg}</p>}
      {raw ? (
        <textarea className="mono" style={{ width: "100%", minHeight: 320 }} value={text} onChange={(e) => setText(e.target.value)} />
      ) : (
        abilities.map((a) => (
          <div key={a.name} className="panel" style={{ marginBottom: 12 }}>
            <h2 style={{ marginTop: 0 }}>{a.name} <span className="muted">{a.version}</span></h2>
            {a.operations.length === 0 && <div className="muted">Read-only: no write operations.</div>}
            <table>
              <tbody>
                {a.operations.map((op) => {
                  const chosen = skills[a.name]?.[op.name] ?? "";
                  return (
                    <tr key={op.name}>
                      <td><strong>{op.name}</strong><div className="muted">{op.description}</div></td>
                      <td className="muted">risk {op.risk}</td>
                      <td>
                        <select value={chosen} onChange={(e) => set(a.name, op.name, e.target.value)}>
                          <option value="">ability default ({autonomyLabel(op.autonomy)})</option>
                          {AUTONOMY_LEVELS.map((l) => (
                            <option key={l} value={l}>{autonomyLabel(l)}</option>
                          ))}
                        </select>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            {policy && Object.keys(policy.targets[a.name] ?? {}).length > 0 && (
              <p className="muted" style={{ marginBottom: 0 }}>
                Per-target overrides exist for: {Object.keys(policy.targets[a.name]).join(", ")} (edit in TOML).
              </p>
            )}
          </div>
        ))
      )}
    </>
  );
}
