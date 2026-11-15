/**
 * Owner: settings. What Settings › Model says when the connection Ghost uses comes from the
 * environment it was started in (ADR 0037). Pure; no React, no window.
 */
import type { ModelConnectionView, PasswordInfo } from "@ghost/client-ts";

export interface EnvironmentNote {
  title: string;
  text: string;
  /** Settings the page can keep, by saving them (settings.env). */
  settings: string[];
  /** The connection's password, when it comes from the environment. */
  password: string | null;
  keepSettings: string | null;
  keepPassword: string | null;
}

/**
 * What Settings › Model says when the connection Ghost uses comes, in part, from the environment
 * it was started in (a shell's export) rather than settings.env or the vault (ADR 0037). Names
 * only: a value never reaches the desktop (ADR 0036).
 */
export function environmentNote(conn: Pick<ModelConnectionView, "from_environment" | "password">): EnvironmentNote | null {
  const names = conn.from_environment ?? [];
  if (names.length === 0) return null;
  const password = conn.password && names.includes(conn.password) ? conn.password : null;
  const settings = names.filter((n) => n !== password);
  const all = [...settings, ...(password ? [password] : [])];
  const list = all.length <= 1 ? all.join("") : `${all.slice(0, -1).join(", ")} and ${all[all.length - 1]}`;
  return {
    title: "From the environment Ghost was started in",
    text: `Ghost is using ${list} from the environment it was started in, not from its own settings. They last until Ghost is started another way.`,
    settings,
    password,
    keepSettings: settings.length ? "Keep the settings: save this page, and they go in settings.env." : null,
    keepPassword: password ? `Keep the password: add ${password} to Ghost’s vault on its row here or on Settings › Passwords.` : null,
  };
}

/** The setting that says which of the environment's keys Ghost may use (#137). */
export const FROM_ENV = "GHOST_LLM_FROM_ENV";

function andList(names: string[]): string {
  return names.length <= 1 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

export interface OfferNote {
  title: string;
  text: string;
  /** What `GHOST_LLM_FROM_ENV` becomes when the person says Use them. */
  useValue: string;
  useLabel: string;
}

/**
 * A provider's key or address in the environment Ghost was started in that Ghost does not use,
 * because nobody chose it for Ghost (#137): offered by name, never a value. Use writes the names
 * to `GHOST_LLM_FROM_ENV` in settings.env, with what is chosen already.
 */
export function offerNote(conn: Pick<ModelConnectionView, "offered" | "chosen_from_environment">): OfferNote | null {
  const offered = conn.offered ?? [];
  if (offered.length === 0) return null;
  const one = offered.length === 1;
  const chosen = new Set([...(conn.chosen_from_environment ?? []), ...offered]);
  return {
    title: "Found in the environment Ghost was started in",
    text: `${andList(offered)} ${one ? "is" : "are"} set there, and Ghost doesn’t use ${one ? "it" : "them"}: another tool or a shell put ${one ? "it" : "them"} there, not you in Ghost. Replies would be billed to ${one ? "it" : "them"}. Ghost uses ${one ? "it" : "them"} only while Ghost is started that way.`,
    useValue: [...chosen].sort().join(","),
    useLabel: one ? "Use it" : "Use them",
  };
}

export interface KeptNote {
  title: string;
  text: string;
}

/** What an update kept for an install that already ran on the environment's keys (#137). */
export function keptNote(conn: Pick<ModelConnectionView, "kept_from_before">): KeptNote | null {
  const kept = conn.kept_from_before ?? [];
  if (kept.length === 0) return null;
  const one = kept.length === 1;
  return {
    title: "Kept from before the update",
    text: `Ghost is using ${andList(kept)} from the environment it was started in, as it did before this update. Nobody chose ${one ? "it" : "them"} for Ghost: use ${one ? "it" : "them"}, or don’t and choose another connection here.`,
  };
}

export interface PasswordEnvironmentNote {
  /** In place of "Not set". */
  badge: string;
  /** The row's help line. */
  text: string;
}

/**
 * Settings › Passwords for a password Ghost is using from the environment it was started in
 * rather than its vault (`PasswordInfo.from_environment`): the same as Settings › Model says,
 * with the row's Add as the way to keep it. Names only (ADR 0036).
 */
export function passwordFromEnvironment(p: Pick<PasswordInfo, "name" | "set"> & { from_environment?: boolean; offered?: boolean }): PasswordEnvironmentNote | null {
  if (p.set) return null;
  if (p.offered && !p.from_environment) {
    // #137: there, and not used: nobody chose it for Ghost. Settings › Model offers it.
    return {
      badge: "In the environment, not used",
      text: "Set in the environment Ghost was started in, by another tool or a shell. Ghost doesn’t use it unless you choose it on Settings › Model, or add it here.",
    };
  }
  if (!p.from_environment) return null;
  return {
    badge: "From the environment",
    text: "Ghost is using it from the environment Ghost was started in, not its vault, until Ghost is started another way. Add it to keep it.",
  };
}
