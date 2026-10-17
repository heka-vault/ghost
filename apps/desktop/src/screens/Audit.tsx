import type { AuditPage } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

export function Audit() {
  const { client, lastEvent } = useGhost();
  const [page, setPage] = useState<AuditPage | null>(null);
  const [offset, setOffset] = useState(0);
  const [open, setOpen] = useState<string | null>(null);
  const limit = 25;

  useEffect(() => {
    client.audit({ limit, offset }).then(setPage).catch(() => setPage(null));
  }, [client, offset, lastEvent?.type === "outcome" ? lastEvent : null]);

  return (
    <>
      <h1>Audit log</h1>
      <p className="muted">Every write Ghost made or proposed, at which level, and what happened.</p>
      <div className="panel">
        <table>
          <thead>
            <tr><th>When</th><th>Change</th><th>Level</th><th>Status</th></tr>
          </thead>
          <tbody>
            {page?.items.map((e) => (
              <>
                <tr key={e.id} onClick={() => setOpen(open === e.id ? null : e.id)} style={{ cursor: "pointer" }}>
                  <td className="muted">{new Date(e.created_at).toLocaleString()}</td>
                  <td>
                    <div><strong>{e.ability}.{e.operation}</strong> on {e.target}</div>
                    <div className="muted">{e.summary}</div>
                  </td>
                  <td><span className="tag">{e.autonomy}</span></td>
                  <td><span className={`tag ${e.status}`}>{e.status}</span>{e.undo_of && <span className="muted"> (undo)</span>}</td>
                </tr>
                {open === e.id && (
                  <tr key={e.id + "-events"}>
                    <td colSpan={4}>
                      <ul className="mono" style={{ margin: 0 }}>
                        {e.events.map((ev, i) => (
                          <li key={i}>{new Date(ev.at).toLocaleTimeString()} {ev.event} {ev.detail ? JSON.stringify(ev.detail) : ""}</li>
                        ))}
                      </ul>
                    </td>
                  </tr>
                )}
              </>
            ))}
            {page && page.items.length === 0 && <tr><td colSpan={4} className="muted">No writes yet.</td></tr>}
          </tbody>
        </table>
        <div className="row" style={{ marginTop: 10 }}>
          <button className="ghost" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - limit))}>Newer</button>
          <span className="muted">{page ? `${offset + 1}–${Math.min(offset + limit, page.total)} of ${page.total}` : ""}</span>
          <button className="ghost" disabled={!page || offset + limit >= page.total} onClick={() => setOffset(offset + limit)}>Older</button>
        </div>
      </div>
    </>
  );
}
