/**
 * The floating window's own commands (apps/desktop/src-tauri/examples/floating.rs). In a plain
 * browser (Vite on :1431 without the window) they do nothing, so the page can be designed
 * there too. SPIKE.
 */
import { invoke } from "@tauri-apps/api/core";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Where Ghost is drawn, in CSS pixels: everywhere else passes clicks through. */
export function setRegion(rects: [number, number, number, number][]): void {
  if (inTauri) void invoke("floating_region", { rects }).catch(() => {});
}

/** One line for the report (stderr of the window process). */
export function report(line: string): void {
  if (inTauri) void invoke("floating_report", { line }).catch(() => {});
  else console.info(`floating ${line}`);
}

/** With FLOATING_IDLE=hidden the window is unmapped when idle. */
export function setVisible(visible: boolean): void {
  if (inTauri) void invoke("floating_visible", { visible }).catch(() => {});
}

/** Rectangles of every element marked `data-hit` that is showing. */
export function hitRects(root: HTMLElement): [number, number, number, number][] {
  const out: [number, number, number, number][] = [];
  root.querySelectorAll<HTMLElement>("[data-hit]").forEach((el) => {
    const r = el.getBoundingClientRect();
    if (r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== "hidden") out.push([r.x, r.y, r.width, r.height]);
  });
  return out;
}
