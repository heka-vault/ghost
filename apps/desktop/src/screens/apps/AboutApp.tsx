/**
 * Owner: apps. "About this app" in the connect dialog, before anything is connected (#37): who
 * published it and what the registry checked, whether it is the product maker's own and why,
 * where it runs, its code, its website, its version and when it last changed. All of it is the
 * registry's text and Ghost's deterministic reading of it (ghost_core::provenance); shown to the
 * operator, never to the model.
 */
import type { CatalogEntryView } from "@ghost/client-ts";
import { BadgeCheck, ExternalLink } from "lucide-react";
import type { ReactNode } from "react";

import { Button, relativeTime } from "../../ui";
import { openExternal } from "./model";
import { httpsHost, makerWhy, placesOf, publisherHelp, publisherParts } from "./provenance";

/** An https link that opens in the operator's browser: the registry's address, shown as host and path. */
function Outside({ url }: { url: string }) {
  const host = httpsHost(url);
  if (!host) return null;
  let shown = host;
  try {
    const u = new URL(url);
    shown = (host + u.pathname.replace(/\/$/, "")).replace(/\.git$/, "");
  } catch {
    /* host alone */
  }
  return (
    <a
      className="apps-about__link"
      href={url}
      rel="noopener noreferrer"
      target="_blank"
      onClick={(ev) => {
        ev.preventDefault();
        void openExternal(url);
      }}
    >
      <span data-user-content>{shown}</span>
      <ExternalLink aria-hidden size={12} />
    </a>
  );
}

function Row({ term, children }: { term: string; children: ReactNode }) {
  return (
    <div className="apps-about__row">
      <dt>{term}</dt>
      <dd>{children}</dd>
    </div>
  );
}

export function AboutApp({ entry: e, onOpen }: { entry: CatalogEntryView; onOpen?: (name: string) => void }) {
  const pub = e.publisher ? publisherParts(e.publisher) : null;
  const why = makerWhy(e);
  const places = placesOf(e);
  const updated = e.updated_at ? new Date(e.updated_at) : null;
  return (
    <section className="apps-about" aria-label="About this app">
      <dl className="apps-about__list">
        {e.publisher && pub && (
          <Row term="Published by">
            <span className="apps-about__value">
              <span data-user-content>{pub.name}</span> · {pub.proof}
            </span>
            <span className="apps-about__help">{publisherHelp(e.publisher)}</span>
          </Row>
        )}
        {e.maker ? (
          <Row term="Made by">
            <span className="apps-about__value apps-about__maker">
              <BadgeCheck aria-hidden size={14} />
              <span>
                From <span data-user-content>{e.maker.product}</span>, the product’s maker
              </span>
            </span>
            {why && (
              <span className="apps-about__help" data-user-content>
                {why}
              </span>
            )}
          </Row>
        ) : e.product ? (
          <Row term="Made by">
            <span className="apps-about__value">
              Not <span data-user-content>{e.product}</span>’s own app
            </span>
            <span className="apps-about__help">
              <span data-user-content>{e.product}</span> publishes its own.
              {e.product_maker && onOpen && (
                <>
                  {" "}
                  <Button variant="ghost" size="sm" className="apps-about__inline" onClick={() => onOpen(e.product_maker as string)}>
                    See {e.product}’s own
                  </Button>
                </>
              )}
            </span>
          </Row>
        ) : null}
        {places.length > 0 && (
          <Row term="Runs on">
            {places.map((p) => (
              <span key={p.where} className={p.caution ? "apps-about__value is-caution" : "apps-about__value"} data-user-content>
                {p.sentence}
              </span>
            ))}
          </Row>
        )}
        {e.repository && httpsHost(e.repository) && (
          <Row term="Code">
            <Outside url={e.repository} />
          </Row>
        )}
        {e.website && httpsHost(e.website) && (
          <Row term="Website">
            <Outside url={e.website} />
          </Row>
        )}
        {(e.version || updated) && (
          <Row term="Version">
            <span className="apps-about__value">
              {e.version && (
                <span className="t-mono" data-user-content>
                  {e.version}
                </span>
              )}
              {e.version && updated && " · "}
              {updated && !Number.isNaN(updated.getTime()) && (
                <span title={updated.toLocaleString()}>updated {relativeTime(e.updated_at)}</span>
              )}
            </span>
          </Row>
        )}
      </dl>
    </section>
  );
}
