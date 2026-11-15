/**
 * Owner: settings. Licences, on About (ADR 0073): Ghost's own (MIT) and the open-source
 * software it is built from, each with its licence, from `licences.json` (bundled with the
 * page; tools/licences.py writes it from Cargo.lock and the desktop's packages).
 */
import { useEffect, useMemo, useState } from "react";

import { Button, Card, Dialog, Input, ListRow, Section, Table } from "../../ui";
import { SECTION_TITLE, sectionDomId } from "./model";

interface Entry {
  name: string;
  version: string;
  license: string;
}
interface Doc {
  ghost: string;
  rust: Entry[];
  javascript: Entry[];
}

export function Licences() {
  const [open, setOpen] = useState(false);
  const [doc, setDoc] = useState<Doc | null>(null);
  useEffect(() => {
    if (!open || doc) return;
    fetch(`${import.meta.env.BASE_URL}licences.json`)
      .then((r) => r.json())
      .then(setDoc)
      .catch(() => setDoc({ ghost: "MIT", rust: [], javascript: [] }));
  }, [open, doc]);
  return (
    <Section id={sectionDomId("licences")} title={SECTION_TITLE.licences} className="set-section">
      <Card list>
        <ListRow
          title="Ghost’s licence"
          description="Ghost is open source under the MIT licence, and so is much of what it is built from."
          trailing={
            <Button size="sm" variant="secondary" onClick={() => setOpen(true)}>
              Open-source software…
            </Button>
          }
        />
      </Card>
      {open && <LicencesDialog doc={doc} onClose={() => setOpen(false)} />}
    </Section>
  );
}

function LicencesDialog({ doc, onClose }: { doc: Doc | null; onClose: () => void }) {
  const [q, setQ] = useState("");
  const rows = useMemo(() => {
    const all = [...(doc?.rust ?? []).map((e) => ({ ...e, kind: "Rust" })), ...(doc?.javascript ?? []).map((e) => ({ ...e, kind: "JavaScript" }))];
    const n = q.trim().toLowerCase();
    return n ? all.filter((e) => e.name.toLowerCase().includes(n) || e.license.toLowerCase().includes(n)) : all;
  }, [doc, q]);
  return (
    <Dialog open onClose={onClose} size="lg" title="Open-source software in Ghost" description={doc ? `${doc.rust.length} Rust crates and ${doc.javascript.length} JavaScript packages, each under its own licence.` : "Loading…"}>
      <Input aria-label="Find a package or licence" placeholder="Find a package or licence" value={q} onChange={(e) => setQ(e.target.value)} />
      <div className="set-licences">
        <Table density="compact">
          <thead>
            <tr>
              <th>Package</th>
              <th>Version</th>
              <th>Licence</th>
            </tr>
          </thead>
          <tbody>
            {rows.slice(0, 400).map((e) => (
              <tr key={`${e.kind}-${e.name}-${e.version}`}>
                <td className="is-mono">{e.name}</td>
                <td className="is-mono is-muted">{e.version}</td>
                <td>{e.license}</td>
              </tr>
            ))}
          </tbody>
        </Table>
        {rows.length > 400 && <p className="set-licences__more">{rows.length - 400} more: type to find one.</p>}
      </div>
    </Dialog>
  );
}
