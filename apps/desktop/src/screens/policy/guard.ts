/**
 * Owner: policy. Ask before leaving with unsaved edits. Page-local until the shell has a
 * shared useUnsavedGuard: route links are caught before they navigate, any other route
 * change (Back, a shortcut) is put back while the operator decides, and closing or
 * reloading the window gets the browser’s own prompt.
 */
import { useEffect, useRef } from "react";

import { navigate } from "../../app/router";

const routeOf = (hash: string) => {
  const h = hash.replace(/^#/, "");
  return h.startsWith("/") ? h.replace(/\/+$/, "") || "/" : null;
};

export function useLeaveGuard(dirty: boolean, here: string, ask: () => Promise<boolean>) {
  const state = useRef({ dirty, ask, asking: false });
  state.current.dirty = dirty;
  state.current.ask = ask;

  useEffect(() => {
    const leave = async (to: string) => {
      const s = state.current;
      if (s.asking) return;
      s.asking = true;
      const ok = await s.ask();
      s.asking = false;
      if (ok) {
        s.dirty = false;
        navigate(to);
      }
    };
    const onClick = (e: MouseEvent) => {
      if (!state.current.dirty || e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
      const a = (e.target as Element | null)?.closest?.("a[href]");
      const to = a ? routeOf(a.getAttribute("href") ?? "") : null;
      if (!to || to === here || a?.getAttribute("target")) return;
      e.preventDefault();
      e.stopPropagation();
      void leave(to);
    };
    const onHash = () => {
      const to = routeOf(window.location.hash);
      if (!state.current.dirty || !to || to === here) return;
      // Put the route back before the router renders it, then ask.
      window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}#${here}`);
      void leave(to);
    };
    const onUnload = (e: BeforeUnloadEvent) => {
      if (!state.current.dirty) return;
      e.preventDefault();
      e.returnValue = "";
    };
    document.addEventListener("click", onClick, true);
    window.addEventListener("hashchange", onHash);
    window.addEventListener("beforeunload", onUnload);
    return () => {
      document.removeEventListener("click", onClick, true);
      window.removeEventListener("hashchange", onHash);
      window.removeEventListener("beforeunload", onUnload);
    };
  }, [here]);
}
