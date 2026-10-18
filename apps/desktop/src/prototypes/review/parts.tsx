/** Owner: review. Small page-local pieces: spoken phrases, issue links, comment authors. */
import { type ReactNode, type Ref, useLayoutEffect, useRef, useState } from "react";

import type { ReviewLink } from "../../design/mock/fixtures/review";
import { Badge, cx, quoted, Tooltip } from "../../ui";
import { spokenRef } from "./model";

/**
 * Phrases the operator can say: Say “approve that” · “next”. Text, not chips, because nothing
 * here is clickable; speaking is the action. Body size, the phrases in --text-1: in the
 * decision block this is the primary action, and the button beside it is the second way.
 */
export function Say({ phrases, label = "Say", className, id, ref }: { phrases: string[]; label?: string; className?: string; id?: string; ref?: Ref<HTMLParagraphElement> }) {
  return (
    <p className={cx("rev-say", className)} id={id} ref={ref}>
      <span className="rev-say__label">{label}</span>{" "}
      {phrases.map((p, i) => (
        <span key={p}>
          {i > 0 && <span className="rev-say__sep" aria-hidden> · </span>}
          <span className="rev-say__phrase">{quoted(p)}</span>
        </span>
      ))}
    </p>
  );
}

const LINK_STATE: Record<ReviewLink["state"], { label: string; tone: "neutral" | "info" | "ok" }> = {
  open: { label: "Open", tone: "neutral" },
  in_review: { label: "In review", tone: "info" },
  closed: { label: "Closed", tone: "ok" },
};

/** A dependency: the spoken ref, the title, its state. Titles are other people's text. */
export function LinkRow({ link, onOpen }: { link: ReviewLink; onOpen?: () => void }) {
  const s = LINK_STATE[link.state];
  const body = (
    <>
      <span className="rev-link__ref t-num">{spokenRef(link)}</span>
      <span className="rev-link__title" data-user-content>{link.title}</span>
      <Badge size="sm" tone={s.tone} variant="outline">{s.label}</Badge>
    </>
  );
  return (
    <li className="rev-link">
      {onOpen ? (
        <button type="button" className="rev-link__hit" onClick={onOpen}>
          {body}
        </button>
      ) : (
        <div className="rev-link__hit is-static">{body}</div>
      )}
    </li>
  );
}

/** A teammate's initial in a neutral disc: comment authors are people, not lines. */
export function Author({ name }: { name: string }) {
  return (
    <span className="rev-author" aria-hidden>
      {name.slice(0, 1).toUpperCase()}
    </span>
  );
}

/**
 * A one-line value that may be cut: the Tooltip primitive carries the whole text, but only
 * while the text actually overflows, so a title that fits gets no bubble. The check runs on
 * mount, once the web font has loaded (Inter is wider than the fallback, so a title that fit
 * before can be cut after), whenever the row around it resizes, and again as the pointer or
 * focus enters that row, before it reaches the text. The outer span holds the flex sizing and
 * is measured; the Tooltip attaches to the inner span (asChild), which does the cutting.
 */
export function Truncated({ text, className, children }: { text: string; className?: string; children?: ReactNode }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [cut, setCut] = useState(false);
  useLayoutEffect(() => {
    const outer = ref.current;
    if (!outer) return;
    const check = () => {
      const el = outer.firstElementChild as HTMLElement | null;
      if (el) setCut(el.scrollWidth > el.clientWidth + 1);
    };
    check();
    let live = true;
    document.fonts?.ready.then(() => live && check());
    // The row, not the text: the text's own box doesn't change when the title is cut.
    const host = outer.closest<HTMLElement>(".rev-row, .rev-link, li") ?? outer.parentElement ?? outer;
    const ro = new ResizeObserver(check);
    ro.observe(host);
    host.addEventListener("pointerenter", check);
    host.addEventListener("focusin", check);
    return () => {
      live = false;
      ro.disconnect();
      host.removeEventListener("pointerenter", check);
      host.removeEventListener("focusin", check);
    };
  }, [text]);
  return (
    <span ref={ref} className={cx("rev-trunc", className)}>
      <Tooltip content={cut ? text : null} asChild>
        <span className="rev-trunc__text" data-user-content>
          {children ?? text}
        </span>
      </Tooltip>
    </span>
  );
}
