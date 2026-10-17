import type { AbilityInfo } from "@ghost/client-ts";
import { autonomyLabel, useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

export function Abilities() {
  const { client } = useGhost();
  const [items, setItems] = useState<AbilityInfo[]>([]);
  const [msg, setMsg] = useState<string | null>(null);

  const load = () => client.abilities().then(setItems).catch((e) => setMsg(String(e)));
  useEffect(() => {
    load();
  }, [client]);

  const toggle = async (a: AbilityInfo) => {
    const enabled = items.filter((x) => x.enabled).map((x) => x.name);
    const next = a.enabled ? enabled.filter((n) => n !== a.name) : [...enabled, a.name];
    try {
      const r = await client.updatePolicy({ enabled: next });
      setMsg(r.restart_required ? "Saved. Restart ghostd to load or unload abilities." : "Saved.");
      await load();
    } catch (e) {
      setMsg(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <>
      <h1>Abilities</h1>
      <p className="muted">
        Installed packages that give Ghost things to read, change, hear or react to. Add one by
        installing a package or dropping a folder in <span className="mono">~/.config/ghost/abilities/</span>.
      </p>
      {msg && <p className="muted">{msg}</p>}
      {items.map((a) => (
        <div key={a.name} className="panel" style={{ marginBottom: 12 }}>
          <div className="row">
            <strong>{a.name}</strong> <span className="muted">{a.version}</span>
            {a.passive && <span className="tag">hears idle audio</span>}
            <span style={{ flex: 1 }} />
            <label className="row">
              <input type="checkbox" checked={a.enabled} onChange={() => toggle(a)} /> enabled
            </label>
          </div>
          <p className="muted" style={{ margin: "6px 0" }}>{a.description}</p>
          {a.enabled && (
            <div className="grid2">
              <div>
                <h2>Reads</h2>
                {a.read_tools.length === 0 && <div className="muted">none</div>}
                {a.read_tools.map((t) => (
                  <div key={t.name}><span className="mono">{t.name}</span> <span className="muted">{t.description}</span></div>
                ))}
              </div>
              <div>
                <h2>Writes</h2>
                {a.operations.length === 0 && <div className="muted">none</div>}
                {a.operations.map((o) => (
                  <div key={o.name}><span className="mono">{o.name}</span> <span className="tag">{autonomyLabel(o.autonomy)}</span></div>
                ))}
              </div>
            </div>
          )}
          {a.enabled && (a.interceptors.length > 0 || a.slots.length > 0 || a.listeners > 0 || a.commands.length > 0) && (
            <p className="muted" style={{ marginBottom: 0 }}>
              {a.interceptors.length > 0 && <>interceptors: {a.interceptors.join(", ")} · </>}
              {a.slots.length > 0 && <>pipeline slots: {a.slots.join(", ")} · </>}
              {a.listeners > 0 && <>{a.listeners} listener(s) · </>}
              {a.commands.length > 0 && <>commands: {a.commands.join(", ")}</>}
            </p>
          )}
        </div>
      ))}
    </>
  );
}
