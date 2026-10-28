/**
 * Home: one thing to do (Start session), how the phone reaches the desk, and the last two
 * decisions. Start sits low, in thumb reach, above the tab bar. Nothing listens here: the mic
 * opens only after Start.
 *
 * Two decisions, not three: at 390 by 844 the hero and two full rows clear Start with room to
 * spare, and See all carries the rest. A third row would always sit half under the fade. While
 * Ghost is asking, the question is the hero and only settled decisions make the list, so the
 * screen never says the same thing twice.
 */
import type { AuditEntry } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { AudioLines, ChevronRight, Inbox, Mic } from "lucide-react";
import { useId, useState } from "react";

import { hrefFor, navigate } from "../../app/router";
import { EmptyState, GhostMark, Progress, Skeleton, VoicePresence, errorText, lineName, relativeTime, useLive } from "../../ui";
import { EntryRow, TargetText, lastSeenAudit, rememberAudit, splitSummary, useAudit } from "./entries";
import { LinkDot, linkLabel, Scroller, Sentences, TabBar, TouchButton, useLink } from "./parts";
import { update } from "./store";

const RECENT = 2;

export function Home({ workstation, via, cached }: { workstation: string; via: string; cached: { savedAt: string; audit: AuditEntry[] } }) {
  const { client, status, pending } = useGhost();
  const { phase, online, loadingModels, setupNeeded } = useLive();
  const link = useLink();
  const { tone, title } = linkLabel(link);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState(false);
  const why = useId();
  // Fetch one more than shown: the live question is the hero, so its row is dropped from the list.
  const recent = useAudit({ limit: RECENT + 1 });
  rememberAudit(recent.items);
  const settledOnly = (items: AuditEntry[] | null) => items?.filter((e) => e.status !== "pending" && e.id !== pending?.id).slice(0, RECENT) ?? null;
  // While the desk is away, show what this phone saw last: this visit's page, or what it saved before.
  const stale = link !== "online" && link !== "connecting";
  const freshList = settledOnly(recent.items);
  const fresh = freshList && freshList.length > 0 ? freshList : null;
  const remembered = settledOnly(lastSeenAudit());
  const shown = fresh ?? (stale ? remembered ?? (cached.audit.length ? cached.audit.slice(0, RECENT) : null) : null);
  const savedAt = !fresh && !remembered && stale && cached.audit.length ? cached.savedAt : null;
  const inSession = online && !!status?.session_id;
  const line = status?.conversation && status.conversation !== "ghost" ? `${lineName(status.conversation)} line` : "main line";

  const start = async () => {
    setStarting(true);
    setError(null);
    try {
      await client.wake();
      update({ startedAt: new Date().toISOString() });
      navigate("/mobile/session");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setStarting(false);
    }
  };

  // Try again asks the desk once, now, with a spinner; the phone also keeps retrying on its own.
  const retry = async () => {
    setRetrying(true);
    setError(null);
    try {
      await client.status();
      recent.retry();
    } catch {
      setError("Still can’t reach it. The phone keeps trying on its own.");
    } finally {
      setRetrying(false);
    }
  };

  // Start is off for one reason at a time; the hero says which, and Start points at it.
  const blocked = link === "reconnecting" || loadingModels || setupNeeded;

  return (
    <div className="mob-screen mob-home">
      <header className="mob-home__head">
        <GhostMark size={32} />
        <div className="mob-home__who">
          <h1 className="mob-home__title">Ghost</h1>
          <p className="mob-home__link" role="status">
            <LinkDot tone={tone} />
            <span className="mob-home__linktext">
              {title}
              {link === "online" && <span className="t-3"> · {workstation} · {via}</span>}
            </span>
          </p>
        </div>
      </header>

      <Scroller>
        <section className="mob-hero" aria-label="Session">
          {link === "connecting" ? (
            <>
              <Skeleton width={96} height={96} radius="var(--radius-full)" />
              <Skeleton width={220} height={28} />
              <Skeleton width={260} height={16} />
            </>
          ) : link === "offline" ? (
            <>
              <VoicePresence phase="offline" size={96} label="" />
              <p className="mob-hero__title">Can’t reach your desk</p>
              <p className="mob-hero__body">Ghost isn’t answering on <span className="mob-nowrap">{workstation}</span>. Check that it’s running and that this phone is on the same Wi-Fi or on Tailscale.</p>
            </>
          ) : inSession && pending ? (
            // The question itself is the body, so the hero says one thing: what Ghost is asking.
            <>
              <VoicePresence phase={phase} size={96} />
              <p className="mob-hero__title">Ghost is waiting on you</p>
              <p className="mob-hero__body" data-user-content>{splitSummary(pending).text}</p>
              <p className="mob-hero__target"><TargetText {...splitSummary(pending)} /></p>
            </>
          ) : inSession ? (
            <>
              <VoicePresence phase={phase} size={96} />
              <p className="mob-hero__title">A session is live</p>
              <p className="mob-hero__body">
                <Sentences text={`On the ${line} at your desk. Join and the mic moves to this phone.`} />
              </p>
            </>
          ) : (
            <>
              <VoicePresence phase={link === "reconnecting" ? "offline" : loadingModels ? "starting" : "passive"} size={96} label="" />
              <p className="mob-hero__title">{link === "reconnecting" ? "Lost your desk for a moment" : loadingModels ? "Getting ready" : setupNeeded ? "Almost ready" : "Ready when you are"}</p>
              <p className="mob-hero__body" id={why}>
                <Sentences
                  text={
                    link === "reconnecting"
                      ? "Start comes back as soon as the phone reaches it again."
                      : loadingModels
                        ? "Ghost is loading its voice models. Usually under a minute."
                        : setupNeeded
                          ? "Ghost needs a model password before it can answer. Finish setup at your desk."
                          : "Tap Start and talk. The mic stays off until you do."
                  }
                />
              </p>
              {loadingModels && link === "online" && <Progress label="Loading voice models" className="mob-hero__progress" />}
            </>
          )}
        </section>

        {link !== "connecting" && shown && (
          <section className="mob-section" aria-labelledby="mob-recent">
            <div className="mob-section__head">
              <h2 id="mob-recent" className="mob-section__title">Recent decisions</h2>
              <a className="mob-more" href={hrefFor("/mobile/activity")}>
                See all <ChevronRight size={16} aria-hidden />
              </a>
            </div>
            {stale && (
              <p className="mob-stale-note">
                Showing what this phone saw last{savedAt ? <>, {relativeTime(savedAt)}</> : null}.
              </p>
            )}
            <div className="mob-list">
              {shown.map((e) => <EntryRow key={e.id} e={e} />)}
            </div>
          </section>
        )}
        {link === "online" && recent.items && !fresh && (
          <section className="mob-section" aria-labelledby="mob-recent">
            <div className="mob-section__head">
              <h2 id="mob-recent" className="mob-section__title">Recent decisions</h2>
            </div>
            <div className="mob-list">
              <EmptyState className="mob-empty mob-empty--compact" size="compact" icon={<Inbox />} title="Nothing yet" description="When Ghost changes something, or asks to, it shows up here." headingLevel="p" />
            </div>
          </section>
        )}
      </Scroller>

      <div className="mob-cta">
        {error && <p className={link === "offline" ? "mob-cta__why" : "mob-cta__error"} role="alert">{error}</p>}
        {link === "offline" ? (
          <TouchButton size="xl" fullWidth loading={retrying} onClick={retry}>
            Try again
          </TouchButton>
        ) : inSession ? (
          <TouchButton variant="primary" size="xl" fullWidth icon={<AudioLines size={22} />} onClick={() => navigate(pending ? "/mobile/decision" : "/mobile/session")}>
            Join the session
          </TouchButton>
        ) : (
          <TouchButton
            variant="primary"
            size="xl"
            fullWidth
            icon={<Mic size={22} />}
            loading={starting}
            disabled={link !== "online" || loadingModels || setupNeeded}
            aria-describedby={blocked ? why : undefined}
            onClick={start}
          >
            Start session
          </TouchButton>
        )}
      </div>

      <TabBar current="home" />
    </div>
  );
}
