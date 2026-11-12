/**
 * Owner: apps. What Ghost connects to (ADR 0034).
 *   #/apps          the connected apps, then the catalog to add more from, and Add your own
 *   #/apps/:name    one app: whether it is connected, and each of its tools with what
 *                            Ghost may do with it (Allow, Ask first, Block; Quiet from the row's menu)
 * Its own place on the rail, not a page of Settings (the owner, 2026-11-12). `#/settings/apps…` and
 * `#/servers`, the old addresses, land here (the shell's redirects, app/routes.tsx).
 *
 * URL states (query before the hash, mock only): `fail=apps` (GET /apps fails),
 * `fail=catalog` (GET /catalog fails), `&catalog=slow|problem|empty` (see the mock).
 */
import type { RouteProps } from "../../app/routes";
import { AppDetail } from "./AppDetail";
import { AppsIndex } from "./AppsIndex";
import { useSignInOpener } from "./signin";
import "./apps.css";

export default function AppsScreen({ params }: RouteProps) {
  useSignInOpener();
  return params.name ? <AppDetail key={params.name} name={params.name} /> : <AppsIndex />;
}
