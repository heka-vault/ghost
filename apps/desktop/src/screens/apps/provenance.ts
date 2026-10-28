/**
 * Owner: apps. A catalog entry's publisher, maker and place in the operator's words (#37). The
 * daemon decides (ghost_core.provenance, deterministic, no model); this only words it. Every
 * name here is the registry's text: shown, never acted on, never sent to the model.
 */
import type { CatalogEntryView, CatalogOption } from "@ghost/client-ts";

type Publisher = NonNullable<CatalogEntryView["publisher"]>;

/** “PostHog · verified on GitHub”, “mcparmory.com · verified domain”: the parts, so the name can be marked as the registry’s text. */
export function publisherParts(p: Publisher): { name: string; proof: string } {
  return { name: p.name, proof: p.verified === "github" ? "verified on GitHub" : "verified domain" };
}

/** What the registry’s check means, for someone who has never heard of a namespace. */
export function publisherHelp(p: Publisher): string {
  const checked = p.verified === "github" ? `signs in to GitHub as ${p.name}` : `owns the domain ${p.name}`;
  return `The app registry checked that whoever published this ${checked}. That tells you who published it, not that it’s safe or that it does what it says.`;
}

const REGISTRY: Record<NonNullable<CatalogOption["registry"]>, string> = { npm: "npm", pypi: "PyPI", oci: "a container" };

/** The owner of a GitHub, GitLab or Codeberg repository from its address. */
export function repositoryOwner(url: string | null | undefined): string | null {
  const m = url ? /^https?:\/\/(?:www\.)?(?:github\.com|gitlab\.com|codeberg\.org)\/([^/?#]+)/i.exec(url) : null;
  return m ? m[1] : null;
}

/** An https address’s host, for a link’s label; null for anything else. */
export function httpsHost(url: string | null | undefined): string | null {
  if (!url || !/^https:\/\//i.test(url)) return null;
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return null;
  }
}

/**
 * Why Ghost says it is the maker’s own, from the evidence the daemon found: “Published by the
 * GitHub account PostHog. Its code is in PostHog’s repository, and it connects to
 * mcp.posthog.com.”
 */
export function makerWhy(e: CatalogEntryView): string | null {
  const m = e.maker;
  if (!m) return null;
  const p = e.publisher;
  const first = m.evidence.includes("listed")
    ? `Ghost’s own list of makers names ${p ? (p.verified === "github" ? `the GitHub account ${p.name}` : p.name) : "its publisher"} as ${m.product}’s maker.`
    : p?.verified === "github"
      ? `Published by the GitHub account ${p.name}, the same name as the product.`
      : `Published by the owner of ${p?.name ?? "its domain"}.`;
  const more: string[] = [];
  const owner = repositoryOwner(e.repository);
  if (m.evidence.includes("repository") && owner) more.push(`its code is in ${owner}’s repository`);
  const hosts = [...new Set(e.options.filter((o) => o.host_owner === "maker" && o.host).map((o) => o.host as string))];
  if (m.evidence.includes("remote") && hosts.length) more.push(`it connects to ${hosts.slice(0, 2).join(" and ")}`);
  const site = httpsHost(e.website);
  if (m.evidence.includes("website") && site) more.push(`its website is ${site}`);
  if (!more.length) return first;
  const list = more.length === 1 ? more[0] : `${more.slice(0, -1).join(", ")} and ${more[more.length - 1]}`;
  return `${first} ${list[0].toUpperCase()}${list.slice(1)}.`;
}

/** Where one way to connect runs: a host and whose it is, or this computer and the package. */
export interface Place {
  /** “mcp.posthog.com”, “npm posthog-mcp”. */
  where: string;
  local: boolean;
  /** Said after the place when the host isn’t the product maker’s: “not PostHog’s servers”. */
  caution: string | null;
  /** The longer sentence for the detail. */
  sentence: string;
}

export function placeOf(o: CatalogOption, e: CatalogEntryView): Place {
  if (o.host) {
    const host = o.host;
    const product = e.product;
    const publisher = o.host_owner === "publisher" ? (e.publisher?.name ?? null) : null;
    let sentence: string;
    if (o.host_owner === "maker") sentence = `Runs on ${host}, ${product ?? "the maker"}’s own server.`;
    else if (product)
      sentence = publisher
        ? `Runs on ${host}, ${publisher}’s server, not ${product}’s. What you ask of ${product} through this app passes through ${publisher}.`
        : `Runs on ${host}, not ${product}’s servers. What you ask of ${product} through this app passes through whoever runs it.`;
    else sentence = publisher ? `Runs on ${host}, the publisher’s own server.` : `Runs on ${host}.`;
    return { where: host, local: false, caution: o.host_owner !== "maker" && product ? `not ${product}’s servers` : null, sentence };
  }
  const reg = o.registry ? REGISTRY[o.registry] : null;
  const where = reg && o.package ? `${o.registry === "oci" ? "container" : reg} ${o.package}` : "this computer";
  return {
    where,
    local: true,
    caution: null,
    sentence: reg && o.package ? `Runs on this computer, from ${reg}: ${o.package}.` : "Runs on this computer.",
  };
}

/** The places an entry can run, one per distinct place, in the order it offers them. */
export function placesOf(e: CatalogEntryView): Place[] {
  const seen = new Set<string>();
  const out: Place[] = [];
  for (const o of e.options) {
    const p = placeOf(o, e);
    if (seen.has(p.where)) continue;
    seen.add(p.where);
    out.push(p);
  }
  return out;
}

/** “4 more from other publishers”. */
export const moreWords = (n: number) => (n === 1 ? "1 more from another publisher" : `${n} more from other publishers`);
