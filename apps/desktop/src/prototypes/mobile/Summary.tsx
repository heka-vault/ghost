/**
 * After End: how long, which line, what changed. Done is the single primary action, low on
 * the screen. The transcript is there but folded.
 */
import type { MobileFixture } from "../../design/mock/fixtures/mobile";
import { useRef } from "react";

import { hrefFor } from "../../app/router";
import { LineGlyph, Notice, Skeleton, formatTime } from "../../ui";
import { EntryRow, useAudit } from "./entries";
import { CompactBar, durationWords, Scroller, TouchButton, TouchDisclosure } from "./parts";
import type { PhoneState } from "./store";

export function Summary({ phone, sample }: { phone: PhoneState; sample: MobileFixture["sampleSummary"] }) {
  const s = phone.ended ?? { ...sample, endFailed: false };
  const changes = useAudit({ limit: 20, session_id: s.sessionId || null });
  const titleRef = useRef<HTMLHeadingElement>(null);

  const lineLabel = s.line.slug === "ghost" ? "Main line" : `${s.line.name} line`;

  return (
    <div className="mob-screen mob-summary">
      <CompactBar title="Session ended" watch={titleRef} />
      <Scroller className="mob-scroll--pad">
        <header className="mob-summary__head">
          <LineGlyph slug={s.line.slug} name={s.line.name} size="xl" />
          <h1 className="mob-summary__title" ref={titleRef}>Session ended</h1>
          <p className="mob-summary__meta">
            {lineLabel} · {durationWords(s.startedAt, s.endedAt)} · ended {formatTime(s.endedAt)}
          </p>
          <p className="mob-summary__mic">The mic on this phone is off.</p>
        </header>

        {s.endFailed && (
          <Notice tone="warn" title="Your desk didn’t get the End">
            The phone couldn’t reach it. If the session is still open at your desk, end it there.
          </Notice>
        )}

        <section className="mob-section" aria-labelledby="mob-changed">
          <div className="mob-section__head">
            <h2 id="mob-changed" className="mob-section__title">
              What changed{changes.items && changes.items.length > 0 ? <span className="t-3 t-num"> · {changes.items.length}</span> : null}
            </h2>
          </div>
          {changes.items == null && !changes.error ? (
            <div className="mob-list" aria-busy="true" aria-label="Loading">
              {[0, 1].map((i) => <div key={i} className="mob-row mob-row--skeleton"><Skeleton width="80%" height={16} /><Skeleton width="45%" height={12} /></div>)}
            </div>
          ) : changes.items && changes.items.length > 0 ? (
            <div className="mob-list">
              {changes.items.map((e) => <EntryRow key={e.id} e={e} />)}
            </div>
          ) : changes.error && !changes.items ? (
            <p className="mob-quiet">Couldn’t load what changed. It’s in Activity once the phone reaches your desk.</p>
          ) : (
            <p className="mob-quiet">Nothing changed. Ghost only looked things up and answered.</p>
          )}
        </section>

        {s.turns.length > 0 && (
          <TouchDisclosure summary={`What was said · ${s.turns.length} ${s.turns.length === 1 ? "turn" : "turns"}`} className="mob-summary__transcript">
            <ol className="mob-transcript">
              {s.turns.map((t, i) => (
                <li key={i} className={t.role === "user" ? "is-you" : "is-ghost"}>
                  <span className="mob-transcript__who">{t.role === "user" ? "You" : "Ghost"}</span>
                  <span data-user-content>{t.text}</span>
                </li>
              ))}
            </ol>
          </TouchDisclosure>
        )}
      </Scroller>

      <div className="mob-cta">
        <TouchButton variant="primary" size="xl" fullWidth href={hrefFor("/mobile/home")}>
          Done
        </TouchButton>
      </div>
    </div>
  );
}
