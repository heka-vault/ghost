import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

export interface SetupState {
  /** Ghost can't answer yet: the model connection is missing something. */
  needed: boolean;
  /**
   * Why, when it is the keyring: Ghost's vault is locked (ADR 0036), so the model's password
   * can't be read. Nothing is missing; the operator unlocks, never re-enters a password.
   */
  locked: boolean;
}

/**
 * Whether Ghost can't answer yet: the model connection is missing something. The same test
 * the Settings screen uses for its first-run card. Reads `GET /model`, which reports the
 * Password only as set or not set; no value ever reaches the app. When something is missing,
 * `GET /passwords` says whether it is only the keyring being locked. Re-checked on every route
 * change (so it clears the moment you leave Settings after saving) and when the window
 * regains focus. False while unknown: never flash a setup prompt on a configured machine.
 */
export function useSetupNeeded(path: string, online: boolean): SetupState {
  const { client } = useGhost();
  const [state, setState] = useState<SetupState>({ needed: false, locked: false });
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
      .then(async (m) => {
        // The model connection (ADR 0037) says whether anything is missing: a key, a sign-in,
        // an address. Its Password is reported only as set or not set.
        if (m.ready) {
          if (live) setState({ needed: false, locked: false });
          return;
        }
        const vault = await client.passwords().then((v) => v.vault.state, () => null);
        if (live) setState({ needed: true, locked: vault === "locked" });
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, online, path, focus]);

  return { needed: online && state.needed, locked: online && state.needed && state.locked };
}
