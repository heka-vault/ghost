import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

/**
 * True when Ghost can't answer yet: the model connection is missing something. The same test
 * the Settings screen uses for its first-run card. Reads `GET /model`, which reports the
 * Password only as set or not set; no value ever reaches the app. Re-checked on every route
 * change (so it clears the moment you leave Settings after saving) and when the window
 * regains focus. False while unknown: never flash a setup prompt on a configured machine.
 */
export function useSetupNeeded(path: string, online: boolean): boolean {
  const { client } = useGhost();
  const [needed, setNeeded] = useState(false);
  const [focus, setFocus] = useState(0);

  useEffect(() => {
    const f = () => setFocus((n) => n + 1);
    window.addEventListener("focus", f);
    return () => window.removeEventListener("focus", f);
  }, []);

  useEffect(() => {
    if (!online) return;
    let live = true;
    client
      .modelConnection()
      .then((m) => {
        // The model connection (ADR 0037) says whether anything is missing: a key, a sign-in,
        // an address. Its Password is reported only as set or not set.
        if (live) setNeeded(!m.ready);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, online, path, focus]);

  return online && needed;
}
