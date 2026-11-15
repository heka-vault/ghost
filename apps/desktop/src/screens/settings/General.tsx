/**
 * Owner: settings. The parts of Settings › General that came from pages of their own until
 * 2026-11-14 (ADR 0073):
 *   StartAtLogin     the tray's “Start at login”, as a switch here too (the shell's choice)
 *   ReachSection     how Ghost reaches you, in sentences, with the three everyday routes as
 *                    switches; the rest of `[routes]` (how long before the next way, quiet
 *                    hours, ntfy, the greeting) is Developer › Notification routes
 *   ConnectionsSection  your phone (Pair a phone in two clicks from here) and Apps
 * Telling Ghost where to send updates by voice is a later step, not built here.
 */
import type { PhoneView, RoutesView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useCallback, useEffect, useState } from "react";

import { hrefFor } from "../../app/router";
import { settingsPath } from "../../app/routes";
import { inTauri } from "../../transport";
import { Badge, Button, Card, ListRow, Section, Switch, errorText, useToast } from "../../ui";
import { SECTION_TITLE, sectionDomId } from "./model";
import { EVERYDAY, type Route, reachSentence } from "./plain";

/* ------------------------------------------------------------------ start at login */

interface LoginView {
  managed: boolean;
  on: boolean;
}

async function invokeLogin(on?: boolean): Promise<LoginView | null> {
  if (!inTauri) return { managed: true, on: on ?? true };
  const { invoke } = await import("@tauri-apps/api/core");
  return on === undefined ? invoke<LoginView>("login_status") : invoke<LoginView>("login_set", { on });
}

/** The tray's Start at login, as a switch. A build from the checkout never registers one. */
export function StartAtLogin() {
  const [view, setView] = useState<LoginView | null>(null);
  useEffect(() => {
    let live = true;
    invokeLogin()
      .then((v) => live && setView(v))
      .catch(() => live && setView(null));
    return () => {
      live = false;
    };
  }, []);
  if (!view) return null;
  return (
    <ListRow
      title="Start at login"
      description={
        view.managed
          ? "Ghost starts in the tray when you log in, listening for its name. Also in the tray’s menu."
          : "A build from the checkout never starts at login; make dev runs it."
      }
      trailing={
        <Switch
          aria-label="Start at login"
          checked={view.managed && view.on}
          disabled={!view.managed}
          onChange={(on: boolean) => void invokeLogin(on).then((v) => v && setView(v))}
        />
      }
    />
  );
}

/* ------------------------------------------------------------------ how Ghost reaches you */

export function ReachSection() {
  const { client } = useGhost();
  const toast = useToast();
  const [routes, setRoutes] = useState<RoutesView | null>(null);
  const [busy, setBusy] = useState<Route | null>(null);
  const load = useCallback(() => {
    client
      .policy()
      .then((p) => setRoutes(p.routes ?? null))
      .catch(() => setRoutes(null));
  }, [client]);
  useEffect(load, [load]);
  if (!routes) return null;
  const toggle = async (name: Route, on: boolean) => {
    setBusy(name);
    try {
      const enabled = on ? Array.from(new Set([...routes.enabled, name])) : routes.enabled.filter((n) => n !== name);
      const res = await client.updatePolicy({ routes: { ...routes, enabled } });
      setRoutes(res.policy.routes ?? null);
    } catch (e) {
      toast({ title: "Couldn’t save it", description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };
  return (
    <Section id={sectionDomId("reach")} title={SECTION_TITLE.reach} description={reachSentence(routes)} className="set-section">
      <Card list>
        {EVERYDAY.map((x) => {
          const there = routes.available.includes(x.name);
          return (
            <ListRow
              key={x.name}
              title={x.title}
              description={there ? x.help : `${x.help} Nothing is behind it on this computer yet.`}
              trailing={<Switch aria-label={x.title} checked={routes.enabled.includes(x.name)} disabled={busy !== null} onChange={(on: boolean) => void toggle(x.name, on)} />}
            />
          );
        })}
        <ListRow
          title="Quiet hours and every other way"
          description="How long Ghost waits before the next way, quiet hours, a calls limit, ntfy and what it says first."
          href={hrefFor(settingsPath("notifications"))}
          chevron
        />
      </Card>
    </Section>
  );
}

/* ------------------------------------------------------------------ connections */

/** Your phone, and the apps Ghost is connected to. Pairing is at most two clicks from here. */
export function ConnectionsSection() {
  const { client, status } = useGhost();
  const [phone, setPhone] = useState<PhoneView | null>(null);
  useEffect(() => {
    client
      .phone()
      .then(setPhone)
      .catch(() => setPhone(null));
  }, [client]);
  const paired = phone?.device ?? null;
  const apps = status?.apps.length ?? 0;
  return (
    <Section id={sectionDomId("connections")} title={SECTION_TITLE.connections} description="What else reaches Ghost: your phone, and the apps it uses." className="set-section">
      <Card list>
        <ListRow
          title="Your phone"
          description={paired ? "Paired. Talk to Ghost from it at home, or away through your own tunnel." : "Talk to Ghost from your phone, at home or away through your own tunnel."}
          meta={paired ? <Badge size="sm" tone="ok" dot>Paired</Badge> : undefined}
          trailing={
            <Button size="sm" variant="secondary" href={hrefFor(paired ? settingsPath("phone") : `${settingsPath("phone")}/pair`)}>
              {paired ? "Open" : "Pair a phone"}
            </Button>
          }
        />
        <ListRow title="Apps" description={apps ? `${apps} connected now. Connect, sign in and see what each can do.` : "Connect an app so Ghost can read and act in it."} href={hrefFor("/apps")} chevron />
      </Card>
    </Section>
  );
}
