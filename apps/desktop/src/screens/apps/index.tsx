/**
 * Owner: apps. What Ghost connects to (ADR 0034).
 *   #/apps          the connected apps, then the catalog to add more from, and Add your own
 *   #/apps/:name    one app: whether it is connected, and each of its tools with what Ghost
 *                   may do with it (Allow, Ask first, Block; Quiet from the row's menu)
 * `#/servers`, the old route, lands on #/apps.
 *
 * URL states (query before the hash, mock only): `fail=apps` (GET /apps fails),
 * `fail=catalog` (GET /catalog fails), `&catalog=slow|problem|empty` (see the mock).
 */
import { useEffect } from "react";

import type { RouteProps } from "../../app/routes";
import { navigate, usePath } from "../../app/router";
import { AppDetail } from "./AppDetail";
import { AppsIndex } from "./AppsIndex";
import { useSignInOpener } from "./signin";
import "./apps.css";

export default function AppsScreen({ params }: RouteProps) {
  const path = usePath();
  useEffect(() => {
    if (path === "/servers") navigate("/apps", { replace: true });
  }, [path]);
  useSignInOpener();
  return params.name ? <AppDetail key={params.name} name={params.name} /> : <AppsIndex />;
}
