/**
 * Owner: apps. What the operator does to a connected app, shared by the list and an app's
 * page: sign in (again), try connecting again, disconnect. Errors come back for the screen
 * to show in a Notice near the app; a toast only ever confirms.
 */
import type { AppView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useState } from "react";

import { useDesign } from "../../design/context";
import { errorText, useConfirm, useToast } from "../../ui";
import { sentence } from "./model";
import type { OwnClientBody } from "./OwnClient";
import { expectSignIn, openSignIn } from "./signin";

export interface AppProblem {
  app: string;
  title: string;
  text: string;
}

export function useAppActions({ onChanged, onDisconnected }: { onChanged: () => void; onDisconnected?: (name: string) => void }) {
  const { client } = useGhost();
  const toast = useToast();
  const design = useDesign();
  const mock = import.meta.env.DEV && !!design;
  const { confirm, dialog } = useConfirm();
  const [busy, setBusy] = useState<string | null>(null);
  const [problem, setProblem] = useState<AppProblem | null>(null);

  /** Sign in again, or try connecting again: the same route, POST /apps/{name}/sign-in. */
  const signIn = async (a: AppView) => {
    if (busy) return;
    setBusy(a.name);
    setProblem(null);
    expectSignIn(a.name);
    try {
      const r = await client.signIn(a.name);
      if (r.sign_in_url) {
        await openSignIn(r.sign_in_url, { again: true, mock });
        toast({ title: "Finish signing in in your browser", description: `Ghost opened ${a.title}’s sign-in page. It shows as connected here once you’re done.` });
      } else if (a.kind === "sign_in") {
        toast({ title: `Signing in to ${a.title}`, description: "If your browser doesn’t open, use Open sign-in page on the app." });
      } else {
        toast({ title: `Connecting ${a.title}`, description: "It shows as connected here once it answers." });
      }
      onChanged();
    } catch (e) {
      setProblem({ app: a.name, title: a.kind === "sign_in" ? `Couldn’t sign in to ${a.title}` : `Couldn’t connect ${a.title}`, text: sentence(errorText(e)) });
    } finally {
      setBusy(null);
    }
  };

  /** The person's own client for an app (ADR 0076), or null to take it back; then it signs in again. */
  const giveClient = async (a: AppView, body: OwnClientBody) => {
    if (busy) return false;
    setBusy(a.name);
    setProblem(null);
    expectSignIn(a.name);
    try {
      const r = await client.setSignInClient(a.name, body);
      const now = r.apps[0];
      if (r.sign_in_url) {
        await openSignIn(r.sign_in_url, { again: true, mock });
        toast({ title: "Finish signing in in your browser", description: r.sign_in_code ? `Type ${r.sign_in_code} on the page Ghost opened.` : `Ghost opened ${a.title}’s sign-in page with your client ID.` });
      } else if (now && (now.state === "needs_client" || now.state === "failed")) {
        setProblem({ app: a.name, title: `${a.title} didn’t take it`, text: now.problem ? sentence(now.problem) : "Check the client ID and try again." });
      } else {
        toast({ title: body.client_id ? `${a.title} signs in with your client ID` : `${a.title} signs in as Ghost again` });
      }
      onChanged();
      return true;
    } catch (e) {
      setProblem({ app: a.name, title: `Couldn’t give ${a.title} the client ID`, text: sentence(errorText(e)) });
      return false;
    } finally {
      setBusy(null);
    }
  };

  /** Open a sign-in that's already waiting, again. */
  const reopen = async (a: AppView) => {
    if (!a.sign_in_url) return;
    const ok = await openSignIn(a.sign_in_url, { again: true, mock });
    if (!ok) setProblem({ app: a.name, title: "Couldn’t open your browser", text: `Open this address yourself to sign in to ${a.title}: ${a.sign_in_url}` });
  };

  const disconnect = async (a: AppView) => {
    const ok = await confirm({
      title: `Disconnect ${a.title}?`,
      description:
        a.settings.length > 0
          ? `Ghost stops using ${a.title} at once and forgets its sign-in and the passwords it saved for it. A password you named yourself stays on Passwords. To use it again, connect it again.`
          : `Ghost stops using ${a.title} at once and forgets its sign-in. To use it again, connect it again.`,
      confirmLabel: `Disconnect ${a.title}`,
      tone: "danger",
    });
    if (!ok) return;
    setBusy(a.name);
    setProblem(null);
    try {
      await client.disconnectApp(a.name);
      toast({ title: `Disconnected ${a.title}`, description: "Ghost no longer uses it." });
      onDisconnected?.(a.name);
      onChanged();
    } catch (e) {
      setProblem({ app: a.name, title: `Couldn’t disconnect ${a.title}`, text: sentence(errorText(e)) });
    } finally {
      setBusy(null);
    }
  };

  return { signIn, giveClient, reopen, disconnect, busy, problem, clearProblem: () => setProblem(null), dialog };
}
