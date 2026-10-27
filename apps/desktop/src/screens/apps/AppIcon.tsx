/**
 * Owner: apps. An app's tile: its icon from the catalog when it is an https address, or a
 * two-letter monogram. The icon is untrusted: loaded as a plain <img> with no referrer, and
 * a broken one falls back to the monogram.
 */
import { useContext, useState } from "react";

import { GlyphSizeContext, cx } from "../../ui";
import { monogram, safeIcon } from "./model";

export function AppIcon({ title, icon, size: own = "md", className }: { title: string; icon?: string | null; size?: "sm" | "md" | "lg"; className?: string }) {
  // In the compact page bar the header's leading tile shrinks with the title, as LineGlyph does.
  const size = useContext(GlyphSizeContext) === "sm" ? "sm" : own;
  const src = safeIcon(icon);
  const [broken, setBroken] = useState(false);
  return (
    <span className={cx("apps-icon", `apps-icon--${size}`, src && !broken && "has-img", className)} aria-hidden>
      {src && !broken ? (
        <img src={src} alt="" referrerPolicy="no-referrer" loading="lazy" decoding="async" draggable={false} onError={() => setBroken(true)} />
      ) : (
        <span className="apps-icon__mono">{monogram(title)}</span>
      )}
    </span>
  );
}
