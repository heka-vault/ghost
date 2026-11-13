/**
 * Settings › Sound: kept wake takes (ADR 0065). Only the trained detector keeps them, on
 * unless the operator turns it off, so under it this says so in one plain line, with the
 * switch, what is kept, Delete, and where to label them (Hearing test › Wake takes). Under
 * transcript wake nothing is kept and the row is not there, unless takes from an earlier
 * detector are left to delete.
 */
import type { WakeTakesView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

import { navigate } from "../../app/router";
import { Button, ListRow, Switch, errorText, useConfirm, useToast } from "../../ui";
import { bytesText, keeping } from "../hear/wakeTakes";

export function WakeTakesRow() {
  const { client, status } = useGhost();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [view, setView] = useState<WakeTakesView | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let on = true;
    client
      .wakeTakes()
      .then((v) => on && setView(v))
      .catch(() => on && setFailed(true));
    return () => {
      on = false;
    };
  }, [client]);

  const said = keeping(status?.wake_mode, view?.count ?? 0);
  if (failed || !said.row) return null;
  const keep = view?.choice.keep ?? true;
  const kept = view ? ` ${view.count} kept, ${bytesText(view.bytes)}.` : "";

  const toggle = async (on: boolean) => {
    try {
      setView(await client.setWakeTakesChoice({ keep: on, unlabelled_accepted_are_me: view?.choice.unlabelled_accepted_are_me ?? false }));
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t change it", description: errorText(e) });
    }
  };

  const removeAll = async () => {
    const ok = await confirm({
      title: "Delete every wake take?",
      description: "Their sound, readings and labels go from this computer. This can’t be undone.",
      confirmLabel: "Delete all",
      tone: "danger",
    });
    if (!ok) return;
    try {
      const r = await client.deleteWakeTakes();
      toast({ tone: "ok", title: `Deleted ${r.deleted} take${r.deleted === 1 ? "" : "s"}` });
      setView(await client.wakeTakes());
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t delete them", description: errorText(e) });
    }
  };

  return (
    <>
      {dialog}
      <ListRow
        title="Wake takes"
        description={
          <>
            {said.notice}
            {kept} Nothing is sent anywhere.
            {said.controls && (
              <>
                {" "}
                <a href="#/hear/wake" onClick={(e) => (e.preventDefault(), navigate("/hear/wake"))}>
                  Label them
                </a>
              </>
            )}
          </>
        }
        meta={
          <span className="set-wake-takes">
            <Button size="sm" variant="danger-ghost" onClick={() => void removeAll()} disabled={!view?.count}>
              Delete all
            </Button>
            {said.controls && <Switch checked={keep} onChange={(on) => void toggle(on)} aria-label="Keep wake takes" />}
          </span>
        }
      />
    </>
  );
}
