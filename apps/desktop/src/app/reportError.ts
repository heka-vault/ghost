/**
 * Owner: app. Error reports from a page (ADR 0067): an uncaught error in the main window or
 * the floating room is handed to the shell (`report_page_error`), which refuses what is not
 * the shape, scrubs it, builds the report and sends it, when this is a released build and the
 * operator has not turned reports off. The page hands over three things only: the error's
 * type, its message and its stack. It never builds a report and never talks to the service.
 *
 * Outside Tauri (the browser, the mock) nothing is sent anywhere.
 */
import { inTauri } from "../transport";

export type PageName = "main" | "floating";
export type CaughtBy = "boundary" | "error" | "rejection";

export interface PageError {
  page: PageName;
  source: CaughtBy;
  type: string;
  message: string;
  stack: string;
}

/** The shell refuses more than these (crates/ghost-reports, report.rs). */
export const MAX_MESSAGE = 2_000;
export const MAX_STACK = 16_000;
const TYPE = /^[A-Za-z_$][A-Za-z0-9_$.]{0,63}$/;

/** What a thrown value says about itself, cut to what the shell takes. */
export function describe(page: PageName, source: CaughtBy, thrown: unknown): PageError {
  if (thrown instanceof Error) {
    const name = thrown.name || thrown.constructor?.name || "Error";
    return {
      page,
      source,
      type: TYPE.test(name) ? name : "Error",
      message: String(thrown.message ?? "").slice(0, MAX_MESSAGE),
      stack: String(thrown.stack ?? "").slice(0, MAX_STACK),
    };
  }
  // A rejected promise with a string or an object: its text may be anyone's (an app's error
  // through the bridge), so only its type goes.
  return { page, source, type: typeof thrown === "string" ? "string" : "object", message: "", stack: "" };
}

let sending = false;

export async function reportPageError(page: PageName, source: CaughtBy, thrown: unknown): Promise<void> {
  if (!inTauri || sending) return;
  sending = true;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("report_page_error", { error: describe(page, source, thrown) });
  } catch {
    // Refused or not sent: never a second error, never shown.
  } finally {
    sending = false;
  }
}

/** `window.onerror` and `unhandledrejection` for this page. */
export function installErrorReporting(page: PageName): void {
  if (!inTauri) return;
  window.addEventListener("error", (e) => {
    // A resource that failed to load (an image) has no error object: not Ghost's code failing.
    if (e.error !== undefined) void reportPageError(page, "error", e.error);
  });
  window.addEventListener("unhandledrejection", (e) => void reportPageError(page, "rejection", e.reason));
}
