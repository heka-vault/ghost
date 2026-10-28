/**
 * Owner: skills. Find skills (#35): search skills.sh through Ghost as the operator types.
 * The daemon caches results for an hour and never searches during a conversation; the model
 * never sees any of this. A result opens the look before installing. When skills.sh fails
 * or changes, the section says so and installing from a GitHub address still works.
 */
import type { SkillListing, SkillSearchPage } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { CloudOff, Search, SearchX } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Badge, Button, Card, EmptyState, Input, ListRow, Notice, Section, Skeleton, Spinner, errorText } from "../../ui";
import { installsWords, sentence } from "./model";

const MIN_QUERY = 2; // skills.sh refuses shorter

export interface FindSkillsProps {
  /** Bumped after an install, so a result's “Installed” follows. */
  version: number;
  onPick: (listing: SkillListing) => void;
  onAddress: () => void;
}

export function FindSkills({ version, onPick, onAddress }: FindSkillsProps) {
  const { client } = useGhost();
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [page, setPage] = useState<SkillSearchPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  // Search as the operator types, a moment after they stop; Enter searches at once.
  useEffect(() => {
    const t = setTimeout(() => setSearch(query.trim()), 300);
    return () => clearTimeout(t);
  }, [query]);

  const load = useCallback(
    async (q: string) => {
      const n = ++seq.current;
      if (q.length < MIN_QUERY) {
        setPage(null);
        setError(null);
        setLoading(false);
        return;
      }
      setLoading(true);
      setError(null);
      try {
        const p = await client.searchSkills(q);
        if (n === seq.current) setPage(p);
      } catch (e) {
        if (n === seq.current) setError(errorText(e));
      } finally {
        if (n === seq.current) setLoading(false);
      }
    },
    [client],
  );

  useEffect(() => {
    void load(search);
  }, [load, search, version]);

  const short = search.length < MIN_QUERY;
  const results = page?.skills ?? [];
  const fromGitHub = (
    <Button size="sm" variant="secondary" onClick={onAddress}>
      Install from GitHub…
    </Button>
  );

  return (
    <Section
      id="skl-find"
      className="skl-find"
      title="Find skills"
      description="From skills.sh, the open directory of skills other agents use. Names are the authors’ own words. Ghost fetches a skill from GitHub and shows it to you before anything is installed."
    >
      <div className="skl-find__search">
        <Input
          id="skl-find-search"
          type="search"
          aria-label="Search skills"
          placeholder="Search skills, like pdf or changelog…"
          value={query}
          leading={<Search size={14} aria-hidden />}
          trailing={loading ? <Spinner size={14} label="Searching" /> : undefined}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") setSearch(query.trim());
            if (e.key === "Escape" && query) {
              e.preventDefault();
              setQuery("");
            }
          }}
        />
      </div>

      {page?.problem && !error && (
        <Notice tone="warn" title="Couldn’t search skills.sh" className="skl-find__notice" action={fromGitHub}>
          <p className="skl-notice__p">
            {sentence(page.problem)} {results.length > 0 ? "These are the results Ghost had for this search." : ""}
          </p>
          <p className="skl-notice__p">You can still install a skill from its GitHub address.</p>
        </Notice>
      )}

      {error ? (
        <Card>
          <EmptyState
            size="compact"
            icon={<CloudOff />}
            title="Couldn’t search"
            description={sentence(error)}
            actions={
              <Button size="sm" variant="secondary" onClick={() => void load(search)}>
                Try again
              </Button>
            }
          />
        </Card>
      ) : short ? (
        <p className="skl-find__hint">Type two letters or more.</p>
      ) : loading && !page ? (
        <Card list aria-busy="true" aria-label="Searching skills.sh">
          {[110, 150, 90].map((w, i) => (
            <div key={i} className="skl-skel-row">
              <Skeleton width={w} height={12} />
              <Skeleton width={w + 70} height={10} />
            </div>
          ))}
        </Card>
      ) : results.length === 0 && !page?.problem ? (
        <Card>
          <EmptyState
            size="compact"
            icon={<SearchX />}
            title={`No skill called “${search}” on skills.sh`}
            description="If you know where it lives on GitHub, install it from its address."
            actions={fromGitHub}
          />
        </Card>
      ) : results.length > 0 ? (
        <Card list className={loading ? "skl-find__list is-stale" : "skl-find__list"}>
          <ul aria-label={`Skills matching ${search}`}>
            {results.map((s) => (
              <li key={s.id}>
                <ListRow
                  onClick={() => onPick(s)}
                  chevron
                  title={<span data-user-content>{s.name}</span>}
                  description={
                    <>
                      <span className="t-mono" data-user-content>
                        {s.source}
                      </span>
                      {" · "}
                      {installsWords(s.installs)}
                    </>
                  }
                  trailing={
                    s.installed ? (
                      <Badge size="sm" tone="ok" dot>
                        Installed
                      </Badge>
                    ) : undefined
                  }
                />
              </li>
            ))}
          </ul>
        </Card>
      ) : null}

      {!error && (
        <p className="skl-find__own">
          Know where it lives on GitHub?{" "}
          <Button variant="ghost" size="sm" onClick={onAddress}>
            Install from GitHub…
          </Button>
        </p>
      )}
    </Section>
  );
}
