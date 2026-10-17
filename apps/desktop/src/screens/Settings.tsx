import type { DevicesView, ModelsView, SettingsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

const NON_SECRET = [
  ["GHOST_REPOS", "GitHub repositories Ghost may touch (owner/repo, comma-separated)"],
  ["GHOST_GITHUB_MOCK", "Use the in-memory GitHub (1) until a token exists"],
  ["ANTHROPIC_BASE_URL", "Anthropic-compatible endpoint"],
  ["GHOST_LLM_MODEL", "Model id (default claude-sonnet-5-5)"],
  ["GHOST_INPUT_DEVICE", "Microphone source (ghost.mic after echo-cancel setup)"],
  ["GHOST_WAKE_PHRASES", "Comma-separated wake phrases"],
  ["GHOST_JEV", "Confirmation classifier: auto | on | off"],
] as const;

/** Settings and first run: tokens (write-only), repos, devices, echo cancellation, models. */
export function Settings() {
  const { client, status } = useGhost();
  const [s, setS] = useState<SettingsView | null>(null);
  const [devices, setDevices] = useState<DevicesView | null>(null);
  const [models, setModels] = useState<ModelsView | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = async () => {
    const [sv, dv, mv] = await Promise.all([client.settings(), client.devices().catch(() => null), client.models()]);
    setS(sv);
    setValues(sv.values);
    setDevices(dv);
    setModels(mv);
  };
  useEffect(() => {
    load().catch((e) => setMsg(String(e)));
  }, [client, status?.models_ready]);

  const save = async () => {
    setBusy(true);
    setMsg(null);
    try {
      const changedValues: Record<string, string | null> = {};
      for (const [k] of NON_SECRET) {
        const v = values[k] ?? "";
        if ((s?.values[k] ?? "") !== v) changedValues[k] = v === "" ? null : v;
      }
      const changedSecrets: Record<string, string | null> = {};
      for (const [k, v] of Object.entries(secrets)) if (v) changedSecrets[k] = v;
      const r = await client.updateSettings({
        values: Object.keys(changedValues).length ? changedValues : null,
        secrets: Object.keys(changedSecrets).length ? changedSecrets : null,
      });
      setSecrets({});
      setMsg(r.restart_required ? "Saved. Restart ghostd to apply." : "Saved.");
      await load();
    } catch (e) {
      setMsg(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const echo = async (uninstall: boolean) => {
    setBusy(true);
    try {
      const r = await client.setupEchoCancel(uninstall);
      setMsg(r.ok ? (uninstall ? "Echo cancellation removed." : "Echo cancellation installed. Set the microphone to ghost.mic and restart ghostd.") : r.detail ?? "failed");
      await load();
    } finally {
      setBusy(false);
    }
  };

  const ready = !!s && (s.secrets.find((x) => x.name === "ANTHROPIC_AUTH_TOKEN")?.set || !!values.ANTHROPIC_BASE_URL);
  return (
    <>
      <h1>Settings</h1>
      {msg && <p className="muted">{msg}</p>}

      {!ready && (
        <div className="panel card steps" style={{ marginBottom: 12 }}>
          <strong>First run</strong>
          <div className="step">Put in a model endpoint and token below, and a GitHub token or keep the mock on.</div>
          <div className="step">Install echo cancellation so Ghost does not hear itself, then set the microphone to ghost.mic.</div>
          <div className="step">Save, restart ghostd, and say "Hey Ghost".</div>
        </div>
      )}

      <h2>Secrets</h2>
      <div className="panel">
        <p className="muted" style={{ marginTop: 0 }}>
          Stored in <span className="mono">{s?.env_path}</span> (mode 600). Never shown again once set.
          {s?.vault_managed && <> This machine renders secrets from its vault at every start; values typed here are replaced then.</>}
        </p>
        {s?.secrets.map((sec) => (
          <div key={sec.name} className="row" style={{ marginBottom: 8 }}>
            <span className="mono" style={{ width: 220 }}>{sec.name}</span>
            <span className={`tag ${sec.set ? "success" : ""}`}>{sec.set ? "set" : "not set"}</span>
            <input type="password" placeholder={sec.set ? "replace…" : "paste…"} value={secrets[sec.name] ?? ""} onChange={(e) => setSecrets({ ...secrets, [sec.name]: e.target.value })} style={{ flex: 1 }} />
          </div>
        ))}
      </div>

      <h2>Settings</h2>
      <div className="panel">
        {NON_SECRET.map(([k, help]) => (
          <div key={k} className="row" style={{ marginBottom: 8 }}>
            <span className="mono" style={{ width: 220 }} title={help}>{k}</span>
            <input value={values[k] ?? ""} placeholder={help} onChange={(e) => setValues({ ...values, [k]: e.target.value })} style={{ flex: 1 }} />
          </div>
        ))}
        <button className="primary" disabled={busy} onClick={save}>Save</button>
      </div>

      <h2>Audio</h2>
      <div className="panel">
        <div className="row">
          <span>Echo cancellation:</span>
          <span className={`tag ${devices?.echo_cancel_active ? "success" : ""}`}>{devices?.echo_cancel_active ? "running" : "not running"}</span>
          <span className="muted">{devices?.echo_cancel_source_present ? "ghost.mic present" : "ghost.mic absent"}</span>
          <span style={{ flex: 1 }} />
          <button className="ghost" disabled={busy} onClick={() => echo(false)}>Install</button>
          <button className="danger" disabled={busy} onClick={() => echo(true)}>Remove</button>
        </div>
        <p className="muted">Input: {devices?.input_device ?? "default"} · Output: {devices?.output_device ?? "default"}</p>
        <details>
          <summary className="muted">Devices PortAudio sees</summary>
          <ul className="mono">
            {devices?.devices.map((d) => (
              <li key={d.index}>{d.index} {d.name} in:{d.max_input_channels} out:{d.max_output_channels}{d.is_default_input ? " (default in)" : ""}{d.is_default_output ? " (default out)" : ""}</li>
            ))}
          </ul>
        </details>
      </div>

      <h2>Models</h2>
      <div className="panel">
        <p style={{ margin: 0 }}>
          <span className={`tag ${models?.ready ? "success" : "pending"}`}>{models?.ready ? "loaded" : "loading"}</span>{" "}
          <span className="muted">Kokoro {models?.kokoro_present ? "cached" : "will download"} · STT {models?.stt_model} · cache {models?.cache_dir}</span>
        </p>
      </div>
    </>
  );
}
