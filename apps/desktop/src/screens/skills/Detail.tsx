/**
 * Owner: skills. The selected skill: where Ghost reads it from, the apps it names and
 * whether each is connected, the files beside SKILL.md, and the instructions as the model
 * reads them. Skill text is untrusted (ADR 0023): shown as plain text, never acted on.
 */
import type { SkillBody, SkillInfo } from "@ghost/client-ts";
import type { ReactNode, Ref } from "react";

import { Badge, Button, Card, Notice, SkeletonText } from "../../ui";
import { count, humanize, isGhosts, listWords, rootName, appOf, tildePath } from "./model";

export interface DetailProps {
  skill: SkillInfo;
  /** SKILL.md once read; null while it loads. */
  body: SkillBody | null;
  /** Why SKILL.md couldn't be read, if it couldn't. */
  bodyError: string | null;
  /** Why the last remove didn't go through, if it didn't. */
  error: string | null;
  online: boolean;
  removing: boolean;
  onRemove: () => void;
  /** Placed just above the header, offset by the compact page bar; the screen scrolls to it on a new pick. */
  headerAnchor?: Ref<HTMLSpanElement>;
}

export function Detail({ skill: s, body, bodyError, error, online, removing, onRemove, headerAnchor }: DetailProps) {
  const name = humanize(s.name);
  const ghosts = isGhosts(s);
  const missing = new Set(s.missing.map(appOf));
  const needs = listWords(s.missing.map(appOf));
  return (
    <article className="skl-detail" aria-labelledby="skl-detail-title">
      <header className="skl-detail__header">
        {/* Where a scroll lands so the header sits under the compact page bar, level with the list. */}
        <span ref={headerAnchor} className="skl-detail__anchor" aria-hidden />
        <div className="skl-detail__titles">
          <h2 id="skl-detail-title" className="t-title skl-detail__title">
            {name}
          </h2>
          <p className="skl-detail__meta">
            <span className="t-mono">{s.name}</span>
            <span aria-hidden>·</span>
            <span>{ghosts ? "Ghost’s own" : s.source === "home" ? "yours, in ~/.agents/skills" : `read in place from ${rootName(s.source)}`}</span>
          </p>
        </div>
        <div className="skl-detail__control">
          {s.problem ? (
            <Badge tone="danger" dot>
              Not offered
            </Badge>
          ) : s.missing.length > 0 ? (
            <Badge tone="warn" dot>
              Needs {needs}
            </Badge>
          ) : (
            <Badge tone="ok" dot>
              Ready
            </Badge>
          )}
          <Button size="sm" variant="danger-ghost" onClick={onRemove} loading={removing} disabled={!ghosts || !online}>
            Remove
          </Button>
        </div>
      </header>

      <p className="skl-detail__description t-body-lg" data-user-content>
        {s.description}
      </p>
      <p className="skl-detail__meta">
        <span className="t-mono">{tildePath(s.path)}</span>
      </p>
      {s.problem && (
        <Notice tone="danger" title="Ghost doesn’t offer this skill" className="skl-detail__notice">
          <p className="skl-notice__p">{s.problem} Fix the folder and Ghost picks it up on its next look.</p>
        </Notice>
      )}
      {!ghosts && (
        <p className="skl-detail__why">
          Read in place from <span className="t-mono">{tildePath(s.path)}</span>, so Ghost can’t remove it. To take it out, remove the folder there.
        </p>
      )}

      {error && (
        <Notice tone="danger" title="Couldn’t remove it" className="skl-detail__notice">
          <p className="skl-notice__p">{error}</p>
        </Notice>
      )}

      <div className="skl-sections">
        <Block
          title="Needs"
          count={s.requires.length}
          description={
            s.requires.length > 0
              ? s.missing.length > 0
                ? `The apps the skill names. Ghost still reads it, but what needs ${needs} can’t happen until you connect ${s.missing.length === 1 ? "it" : "them"} under Apps.`
                : "The apps the skill names, every one connected."
              : undefined
          }
        >
          {s.requires.length > 0 ? (
            <Card list>
              {s.requires.map((r) => (
                <div key={r} className="skl-item skl-item--compact">
                  <div className="skl-item__text">
                    <p className="skl-item__title">
                      {humanize(appOf(r))}
                      <span className="skl-item__id">{appOf(r)}</span>
                    </p>
                  </div>
                  <span className="skl-item__state">
                    {missing.has(appOf(r)) ? (
                      <Badge tone="danger" dot size="sm">
                        Not connected
                      </Badge>
                    ) : (
                      <Badge tone="ok" dot size="sm">
                        Connected
                      </Badge>
                    )}
                  </span>
                </div>
              ))}
            </Card>
          ) : (
            <p className="skl-none">
              Nothing beyond Ghost itself. A skill names the apps it needs under <span className="t-mono">requires</span> in its front matter.
            </p>
          )}
        </Block>

        <Block title="Files" count={s.files.length} description={s.files.length > 0 ? "Beside SKILL.md. Ghost reads one when the instructions point to it, text only, inside this folder only." : undefined}>
          {s.files.length > 0 ? (
            <Card list>
              {s.files.map((f) => (
                <div key={f} className="skl-item skl-item--compact">
                  <p className="skl-item__title t-mono">{f}</p>
                </div>
              ))}
            </Card>
          ) : (
            <p className="skl-none">Only SKILL.md.</p>
          )}
        </Block>

        <Block title="Instructions" description="SKILL.md as Ghost reads it, without the front matter. It shapes what Ghost does, never what it may do; nothing in it runs.">
          {bodyError ? (
            <Notice tone="danger" title="Couldn’t read SKILL.md">
              <p className="skl-notice__p">{bodyError}</p>
            </Notice>
          ) : body ? (
            body.body.trim() ? (
              <Card tone="sunken">
                <pre className="skl-body" data-user-content>
                  {body.body}
                </pre>
              </Card>
            ) : (
              <p className="skl-none">SKILL.md has its front matter and nothing else yet.</p>
            )
          ) : (
            <Card tone="sunken" aria-busy="true" aria-label="Reading SKILL.md">
              <div className="skl-skel-body">
                <SkeletonText lines={6} />
              </div>
            </Card>
          )}
        </Block>
      </div>
    </article>
  );
}

/** A titled block, like Section, with its count beside the heading. */
function Block({ title, count: n, description, children }: { title: string; count?: number; description?: ReactNode; children: ReactNode }) {
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
