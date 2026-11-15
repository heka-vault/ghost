/**
 * Owner: settings. "Remove Ghost from this computer" (#147), last on General: what Ghost left
 * on this computer by kind, each with its size and a plain sentence, chosen separately (the
 * person's data and passwords off unless they turn them on, each with its warning), then one
 * confirmation that names everything that goes and the one command that finishes it. Confirming
 * quits the app; `ghostd uninstall` removes what was chosen once the app is gone (the window
 * can't remove its own storage while it is open) and says so in a notification.
 *
 * The list is `GET /uninstall`, the same `ghost_core::leftovers` `ghostd uninstall` prints.
 */
import type { LeftoverCategoryView, UninstallView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Trash2 } from "lucide-react";
import { useEffect, useState } from "react";

import { type LeftoverKind, removeGhostAndQuit } from "../../app/uninstall";
import { inTauri } from "../../transport";
import { Button, Card, Checkbox, Dialog, ListRow, Section, errorText, useCopy, useToast } from "../../ui";
import { SECTION_TITLE, sectionDomId } from "./model";
import { chosenAtFirst, humanSize } from "./remove";
import "./settings.css";

function Kind({ c, on, onChange }: { c: LeftoverCategoryView; on: boolean; onChange: (on: boolean) => void }) {
  const [open, setOpen] = useState(false);
  const empty = c.items.length === 0;
  return (
    <ListRow
      align="start"
      leading={<Checkbox checked={on && !empty} disabled={empty} onChange={onChange} aria-label={c.title} />}
      title={c.title}
      description={
        <>
          {empty ? "Nothing of this kind is on this computer." : c.sentence}
          {!empty && c.warning && <span className="set-remove__warning">{c.warning}</span>}
          {open && (
            <span className="set-remove__items">
              {c.items.map((i) => (
                <span className="set-remove__item" key={i.key}>
                  <span>
                    {i.what}
                    {i.size > 0 && <span className="t-3"> · {humanSize(i.size)}</span>}
                  </span>
                  {i.paths.map((p) => (
                    <span className="set-remove__path" key={p}>
                      {p}
                    </span>
                  ))}
                  {i.note && <span className="t-3">{i.note}</span>}
                </span>
              ))}
            </span>
          )}
        </>
      }
      meta={empty ? undefined : humanSize(c.size)}
      trailing={
        empty ? undefined : (
          <Button size="sm" variant="ghost" onClick={() => setOpen(!open)} aria-expanded={open}>
            {open ? "Hide" : "Show"}
          </Button>
        )
      }
    />
  );
}

export function RemoveGhostSection({ confirmOpen = false }: { confirmOpen?: boolean }) {
  const { client } = useGhost();
  const toast = useToast();
  const [view, setView] = useState<UninstallView | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [copied, copy] = useCopy();

  useEffect(() => {
    let on = true;
    client
      .uninstallView()
      .then((v) => {
        if (!on) return;
        setView(v);
        setChosen(chosenAtFirst(v));
        // The mock's `?remove=confirm`: opened once the list is there, as a click would.
        if (confirmOpen) setTimeout(() => on && setConfirm(true), 300);
      })
      .catch(() => undefined);
    return () => {
      on = false;
    };
  }, [client, confirmOpen]);
  if (!view) return null;

  const picked = view.categories.filter((c) => chosen.has(c.id) && c.items.length > 0);
  const set = (id: string, on: boolean) => {
    const next = new Set(chosen);
    if (on) next.add(id);
    else next.delete(id);
    setChosen(next);
  };
  const go = async () => {
    setBusy(true);
    try {
      if (!inTauri) {
        toast({ title: "In the app, Ghost would quit now", description: `and remove ${picked.length === 1 ? "one kind" : `${picked.length} kinds`} of what it left.` });
        setConfirm(false);
        return;
      }
      await removeGhostAndQuit(picked.map((c) => c.id as LeftoverKind));
    } catch (e) {
      toast({ tone: "danger", title: "Ghost could not start removing itself", description: errorText(e) });
    } finally {
      setBusy(false);
    }
  };
  const finish = view.install === "checkout" ? null : view.finish;

  return (
    <Section
      id={sectionDomId("remove")}
      title={SECTION_TITLE.remove}
      description="What Ghost keeps on this computer, by kind. Choose what goes; Ghost quits and removes it, and one command finishes."
      className="set-section"
    >
      <Card list>
        {view.categories.map((c) => (
          <Kind key={c.id} c={c} on={chosen.has(c.id)} onChange={(on) => set(c.id, on)} />
        ))}
        <ListRow
          title={finish ? "Then, in a terminal" : "Then"}
          description={finish ? <span className="set-value is-mono">{finish}</span> : view.finish}
          trailing={
            <Button variant="danger-ghost" size="sm" leadingIcon={<Trash2 size={14} />} disabled={picked.length === 0} onClick={() => setConfirm(true)}>
              Remove Ghost…
            </Button>
          }
        />
      </Card>
      <Dialog
        open={confirm}
        onClose={() => !busy && setConfirm(false)}
        dismissible={!busy}
        size="md"
        title="Remove Ghost from this computer?"
        description="Ghost quits, then deletes:"
        initialFocus="[data-cancel]"
        footer={
          <>
            <Button variant="secondary" data-cancel onClick={() => setConfirm(false)} disabled={busy}>
              Cancel
            </Button>
            <Button variant="danger" loading={busy} onClick={() => void go()}>
              Remove and quit
            </Button>
          </>
        }
      >
        <ul className="set-remove__confirm">
          {picked.map((c) => (
            <li key={c.id}>
              <strong>{c.title}</strong> <span className="t-3">· {humanSize(c.size)}</span>
              {c.warning && <span className="set-remove__warning">{c.warning}</span>}
            </li>
          ))}
        </ul>
        {view.categories.some((c) => c.items.length > 0 && !chosen.has(c.id)) && (
          <p className="t-2">Kept: {view.categories.filter((c) => c.items.length > 0 && !chosen.has(c.id)).map((c) => c.title.toLowerCase()).join("; ")}.</p>
        )}
        {finish && (
          <div className="set-remove__finish">
            <p className="t-2">To finish, run this in a terminal once Ghost has quit:</p>
            <span className="set-remove__command">
              <code>{finish}</code>
              <Button size="sm" variant="ghost" onClick={() => copy(finish)}>
                {copied ? "Copied" : "Copy"}
              </Button>
            </span>
          </div>
        )}
      </Dialog>
    </Section>
  );
}
