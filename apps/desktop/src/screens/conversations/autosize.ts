/**
 * Owner: conversations. A textarea that fits its text: at least `min` rows, at most `max`,
 * then it scrolls. Measured from the element, so it follows the type scale and the theme.
 */
import { type RefObject, useLayoutEffect, useState } from "react";

export function useAutoRows(ref: RefObject<HTMLTextAreaElement | null>, value: string, min: number, max: number): number {
  const [rows, setRows] = useState(min);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const cs = getComputedStyle(el);
    const line = parseFloat(cs.lineHeight) || 20;
    const pad = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
    // Measure at the minimum, so the field can shrink as well as grow.
    const was = el.rows;
    const height = el.style.height;
    el.style.height = "";
    el.rows = min;
    const need = Math.ceil((el.scrollHeight - pad) / line);
    el.rows = was;
    el.style.height = height;
    setRows(Math.max(min, Math.min(max, need)));
  }, [ref, value, min, max]);
  return rows;
}
