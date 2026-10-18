import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

/**
 * True when Ghost can't answer yet: no Anthropic token and no model endpoint. The same test
 * the Settings screen uses for its first-run card. Reads `GET /settings`, which reports
 * secrets only as set or not set; no value ever reaches the app. Re-checked on every route
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
      .settings()
      .then((s) => {
        if (!live) return;
        const token = s.secrets.find((x) => x.name === "ANTHROPIC_AUTH_TOKEN")?.set ?? false;
        setNeeded(!token && !s.values.ANTHROPIC_BASE_URL);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, online, path, focus]);

  return online && needed;
}
