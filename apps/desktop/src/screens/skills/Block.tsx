/** Owner: skills. A titled block, like Section, with its count beside the heading: the
 *  skill's page and the look before installing use the same one. */
import type { ReactNode } from "react";

import { count } from "./model";

/** A titled block, like Section, with its count beside the heading. */
export function Block({ title, count: n, description, children }: { title: string; count?: number; description?: ReactNode; children: ReactNode }) {
  return (
    <section className="skl-block">
      <header className="skl-block__header">
        <div className="skl-block__head">
          <h3 className="skl-block__title">
            {title}
            {n !== undefined && (
              <span className="skl-block__count t-num" aria-label={count(n, "item")}>
                {n}
              </span>
            )}
          </h3>
        </div>
        {description && <p className="skl-block__description">{description}</p>}
      </header>
      {children}
    </section>
  );
}
