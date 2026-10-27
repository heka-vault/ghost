/**
 * Owner: apps. The sign-in pages this window has asked for (ADR 0034). A connect or a sign-in
 * usually comes back with the page's address; when the app takes longer, the address comes
 * later on the event stream (`{type: "apps", sign_in_url}`), and is opened only for an app
 * the operator just asked to connect here, and only once.
 */
import { useGhost } from "@ghost/ui";
import { useEffect } from "react";

import { useDesign } from "../../design/context";
import { openExternal } from "./model";

const asked = new Set<string>();
const opened = new Set<string>();

/** Mark an app as one whose sign-in page should open when it arrives. */
export function expectSignIn(name: string) {
  asked.add(name);
}

/**
 * Open a sign-in page in the browser, once per address unless `again`. Under the mock it opens
 * nothing (the address is made up); the screen still shows the waiting state.
 */
export async function openSignIn(url: string, opts: { again?: boolean; mock?: boolean } = {}): Promise<boolean> {
  if (!opts.again && opened.has(url)) return true;
  opened.add(url);
  if (opts.mock) return true;
  return openExternal(url);
}

/** Watch the event stream for a sign-in page this window asked for, and open it. */
export function useSignInOpener() {
  const { lastEvent } = useGhost();
  const design = useDesign();
  const mock = import.meta.env.DEV && !!design;
  useEffect(() => {
    if (lastEvent?.type !== "apps" || !lastEvent.sign_in_url) return;
    const name = lastEvent.changed ?? null;
    if (name ? !asked.has(name) : asked.size === 0) return;
    if (name) asked.delete(name);
    void openSignIn(lastEvent.sign_in_url, { mock });
  }, [lastEvent, mock]);
}
