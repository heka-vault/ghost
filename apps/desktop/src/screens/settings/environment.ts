/**
 * Owner: settings. What Settings › Model says when the connection Ghost uses comes from the
 * environment it was started in (ADR 0037). Pure; no React, no window.
 */
import type { ModelConnectionView } from "@ghost/client-ts";

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
