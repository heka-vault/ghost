//! Who publishes a catalog entry, whether it is the product maker's own, and where it runs
//! (#37). Deterministic: string rules over the registry's record, no model.
//!
//! The registry checks one thing about every entry: its name's namespace.
//!
//! - `io.github.<owner>/…`: the publisher signed in to GitHub as `<owner>`;
//! - `com.example/…` (reverse DNS): the publisher proved they own `example.com`.
//!
//! That says who published it, not that it is safe. Ghost builds on it:
//!
//! - **The maker's own.** An entry is the product maker's own app when the publisher's name
//!   *is* the product's (`io.github.PostHog` publishing "PostHog", `com.notion` publishing
//!   "Notion") and it connects to the maker's own host: for a domain, a host under that exact
//!   domain (`com.notion` and `mcp.notion.com`; `ai.posthog` is not `posthog.com`); for a
//!   GitHub account, a host registered under the account's name (`mcp.posthog.com` for
//!   `PostHog`). The name alone is never enough, and neither is a repository or a website:
//!   every GitHub account owns repositories under its own name, and anyone can write any
//!   website. [`KNOWN_MAKERS`] is the short hand-kept list for makers whose names differ from
//!   their product's (`getsentry` makes Sentry), or whose own app has no remote. The
//!   registry's `_meta` carries no maker flag (2026-10: only status and dates), so nothing is
//!   read from it here.
//! - **Where it runs.** A remote's host, and whether that host is the maker's, the
//!   publisher's, or someone else's; a package's registry and identifier.
//! - **The same product.** Entries whose titles normalize to the maker's own product name
//!   (`Posthog`, `PostHog`) fold under the maker's entry in search results; nothing else
//!   does.
//!
//! All of it is the registry's text shown to the operator. None of it reaches the model.

use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

use regex::Regex;

/// What the registry checked about a publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verified {
    Github,
    Domain,
}

impl Verified {
    pub fn as_str(self) -> &'static str {
        match self {
            Verified::Github => "github",
            Verified::Domain => "domain",
        }
    }
}

/// What agrees that an entry is the maker's own: the publisher's name is the product's;
/// Ghost's own list says so; it connects to the maker's host. Only `Listed` or `Remote` is
/// proof; a domain publisher's repository under its name and a website on the maker's host
/// are said beside it, never instead of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    Publisher,
    Listed,
    Remote,
    Repository,
    Website,
}

impl Evidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Evidence::Publisher => "publisher",
            Evidence::Listed => "listed",
            Evidence::Remote => "remote",
            Evidence::Repository => "repository",
            Evidence::Website => "website",
        }
    }
}

/// Whose host an entry connects to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOwner {
    Maker,
    Publisher,
    Other,
}

impl HostOwner {
    pub fn as_str(self) -> &'static str {
        match self {
            HostOwner::Maker => "maker",
            HostOwner::Publisher => "publisher",
            HostOwner::Other => "other",
        }
    }
}

const GITHUB_NS: &str = "io.github.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownMaker {
    /// The product's name as its maker writes it.
    pub product: &'static str,
    /// The names its entries go by: "sentry", "getsentry".
    pub brands: &'static [&'static str],
    /// The maker's own registered domains: "sentry.dev".
    pub domains: &'static [&'static str],
}

/// Makers whose evidence is unusual: a GitHub account or a host not named like the product.
/// Keyed by the registry namespace. Keep it short; every entry is a judgment someone checked.
pub const KNOWN_MAKERS: &[(&str, KnownMaker)] = &[
    // GitHub's own account; its app runs on api.githubcopilot.com.
    (
        "io.github.github",
        KnownMaker {
            product: "GitHub",
            brands: &["github", "githubcopilot"],
            domains: &["github.com", "githubcopilot.com"],
        },
    ),
    // Sentry's account is getsentry; its app runs on mcp.sentry.dev.
    (
        "io.github.getsentry",
        KnownMaker {
            product: "Sentry",
            brands: &["sentry", "getsentry"],
            domains: &["sentry.dev", "sentry.io"],
        },
    ),
    // Notion's account is makenotion (`io.github.notion` is not Notion).
    (
        "io.github.makenotion",
        KnownMaker {
            product: "Notion",
            brands: &["notion", "makenotion"],
            domains: &["notion.com", "notion.so"],
        },
    ),
    // Neon proved neon.com; its app runs on mcp.neon.tech, Neon's other domain.
    (
        "com.neon",
        KnownMaker {
            product: "Neon",
            brands: &["neon"],
            domains: &["neon.com", "neon.tech"],
        },
    ),
    // The makers below publish a package run on this computer, so no host of theirs can
    // agree. Each namespace was checked by hand on 2026-10-28 as the company's own account
    // or domain, and was among the real makers the stricter rule above unmarked.
    (
        "com.auth0",
        KnownMaker {
            product: "Auth0",
            brands: &["auth0"],
            domains: &["auth0.com"],
        },
    ),
    (
        "io.github.ChromeDevTools",
        KnownMaker {
            product: "Chrome DevTools",
            brands: &["chromedevtools"],
            domains: &["chrome.com"],
        },
    ),
    (
        "io.github.ClickHouse",
        KnownMaker {
            product: "ClickHouse",
            brands: &["clickhouse"],
            domains: &["clickhouse.com", "clickhouse.cloud"],
        },
    ),
    (
        "io.github.PagerDuty",
        KnownMaker {
            product: "PagerDuty",
            brands: &["pagerduty"],
            domains: &["pagerduty.com"],
        },
    ),
    (
        "io.github.browserbase",
        KnownMaker {
            product: "Browserbase",
            brands: &["browserbase"],
            domains: &["browserbase.com"],
        },
    ),
    (
        "io.github.buildkite",
        KnownMaker {
            product: "Buildkite",
            brands: &["buildkite"],
            domains: &["buildkite.com"],
        },
    ),
    (
        "io.github.couchbase",
        KnownMaker {
            product: "Couchbase",
            brands: &["couchbase"],
            domains: &["couchbase.com"],
        },
    ),
    (
        "io.github.firebase",
        KnownMaker {
            product: "Firebase",
            brands: &["firebase"],
            domains: &["firebase.com"],
        },
    ),
    (
        "io.github.jfrog",
        KnownMaker {
            product: "JFrog",
            brands: &["jfrog"],
            domains: &["jfrog.com", "jfrog.io"],
        },
    ),
    (
        "io.github.localstack",
        KnownMaker {
            product: "LocalStack",
            brands: &["localstack"],
            domains: &["localstack.cloud"],
        },
    ),
    (
        "io.github.metabase",
        KnownMaker {
            product: "Metabase",
            brands: &["metabase"],
            domains: &["metabase.com"],
        },
    ),
    (
        "io.github.paypal",
        KnownMaker {
            product: "PayPal",
            brands: &["paypal"],
            domains: &["paypal.com"],
        },
    ),
    (
        "io.github.planetscale",
        KnownMaker {
            product: "PlanetScale",
            brands: &["planetscale"],
            domains: &["planetscale.com", "pscale.dev"],
        },
    ),
    (
        "io.github.questdb",
        KnownMaker {
            product: "QuestDB",
            brands: &["questdb"],
            domains: &["questdb.com", "questdb.io"],
        },
    ),
    (
        "io.github.upstash",
        KnownMaker {
            product: "Upstash",
            brands: &["upstash"],
            domains: &["upstash.com", "upstash.io"],
        },
    ),
    (
        "io.github.zscaler",
        KnownMaker {
            product: "Zscaler",
            brands: &["zscaler"],
            domains: &["zscaler.com"],
        },
    ),
];

/// The hand-kept entry for a registry namespace, when there is one.
pub fn known_maker(namespace: &str) -> Option<&'static KnownMaker> {
    KNOWN_MAKERS
        .iter()
        .find(|(ns, _)| *ns == namespace)
        .map(|(_, m)| m)
}

/// Hosts where anyone can run something under a name of their own: `x.vercel.app` is not
/// Vercel's, so a host here has no brand and is never the maker's.
pub const SHARED_HOSTS: &[&str] = &[
    "vercel.app",
    "fly.dev",
    "workers.dev",
    "pages.dev",
    "herokuapp.com",
    "netlify.app",
    "onrender.com",
    "railway.app",
    "github.io",
    "githubusercontent.com",
    "run.app",
    "appspot.com",
    "web.app",
    "firebaseapp.com",
    "azurewebsites.net",
    "azurecontainerapps.io",
    "amazonaws.com",
    "cloudfront.net",
    "supabase.co",
    "deno.dev",
    "deno.net",
    "ngrok.io",
    "ngrok.app",
    "ngrok-free.app",
    "glitch.me",
    "replit.app",
    "repl.co",
    "modal.run",
    "hf.space",
    "trycloudflare.com",
];
/// Second-level labels under a country code: `foo.co.uk` is foo's.
const SECOND_LEVEL: &[&str] = &[
    "co", "com", "org", "net", "ac", "gov", "edu", "ne", "or", "go",
];
/// A GitHub account named for a company often carries one of these: `pipeworx-io`,
/// `engageable-tech`. Used only to tell a publisher's own host, never for the maker's mark.
const ACCOUNT_SUFFIXES: &[&str] = &[
    "io", "hq", "inc", "dev", "ai", "app", "labs", "official", "tech", "oss", "org", "com", "co",
    "team",
];

static SHARED: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| SHARED_HOSTS.iter().copied().collect());

/// A name for comparing: lower case, letters and digits only. "PostHog" and "Posthog"
/// are both `posthog`.
pub fn key(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

/// `[\d.]+` or `[.*]`: an IP address, not a name.
fn is_address(host: &str) -> bool {
    host.chars().all(|c| c.is_ascii_digit() || c == '.')
        || (host.starts_with('[') && host.ends_with(']'))
}

/// The domain a host is registered under: `mcp.posthog.com` is `posthog.com`,
/// `api.foo.co.uk` is `foo.co.uk`. None for an address, a bare name, or a shared host
/// (`x.vercel.app`), where the name isn't the owner's.
pub fn registered_domain(host: &str) -> Option<String> {
    let lower = host.to_lowercase();
    let host = lower.trim_matches('.').split(':').next().unwrap_or("");
    if host.is_empty() || is_address(host) || !host.contains('.') {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if (0..labels.len() - 1).any(|i| SHARED.contains(labels[i..].join(".").as_str())) {
        return None;
    }
    let n = labels.len();
    let keep =
        if n >= 3 && SECOND_LEVEL.contains(&labels[n - 2]) && labels[n - 1].chars().count() == 2 {
            3
        } else {
            2
        };
    let tail = &labels[n - keep..];
    if tail.iter().any(|l| l.is_empty()) {
        return None;
    }
    Some(tail.join("."))
}

/// The name a host is registered under, as a key: `mcp.posthog.com` is `posthog`,
/// `foo.co.uk` is `foo`. None where `registered_domain` is.
pub fn host_brand(host: &str) -> Option<String> {
    let domain = registered_domain(host)?;
    let brand = key(domain.split('.').next().unwrap_or(""));
    (!brand.is_empty()).then_some(brand)
}

// scheme://[user@]host[:port][/path]: the host, and the path's first part. A regex, not a
// URL parser: the index reads ~50,000 addresses.
static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*://(?:[^/?#@]*@)?([^/?#:]+)(?::\d*)?(?:/([^/?#]*))?")
        .unwrap()
});
const FORGES: &[&str] = &["github.com", "gitlab.com", "codeberg.org"];

fn url_parts(url: &str) -> Option<(String, Option<String>)> {
    let m = URL.captures(url)?;
    let host = m[1].to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    Some((host, m.get(2).map(|p| p.as_str().to_string())))
}

/// `https://mcp.posthog.com/mcp` is `mcp.posthog.com`; `www.` is dropped.
pub fn url_host(url: &str) -> Option<String> {
    url_parts(url).map(|(host, _)| host)
}

/// `https://github.com/PostHog/posthog` is `PostHog`; only GitHub, GitLab and
/// Codeberg addresses, where the first part of the path is the owner.
pub fn repository_owner(url: &str) -> Option<String> {
    let (host, owner) = url_parts(url)?;
    if !FORGES.contains(&host.as_str()) {
        return None;
    }
    owner.filter(|o| !o.is_empty())
}

/// Who published an entry, as the registry checked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publisher {
    /// "io.github.PostHog", "com.mcparmory".
    pub namespace: String,
    /// As shown: the GitHub account ("PostHog") or the domain ("mcparmory.com").
    pub name: String,
    pub verified: Verified,
    /// Names a host of theirs may carry: {"pipeworxio", "pipeworx"}.
    pub brands: BTreeSet<String>,
}

/// From the registry name: `io.github.PostHog/mcp` was published by the GitHub
/// account PostHog; `com.mcparmory/posthog` by whoever owns mcparmory.com.
pub fn publisher_of(name: &str) -> Option<Publisher> {
    let (ns, _) = name.split_once('/')?;
    if ns.is_empty() {
        return None;
    }
    publisher(ns)
}

fn publisher(ns: &str) -> Option<Publisher> {
    if let Some(owner) = ns.strip_prefix(GITHUB_NS) {
        if owner.is_empty() || owner.contains('.') {
            return None;
        }
        let mut brands = BTreeSet::from([key(owner)]);
        let lower = owner.to_lowercase();
        let parts: Vec<&str> = lower.split('-').collect();
        if parts.len() > 1 && ACCOUNT_SUFFIXES.contains(&parts[parts.len() - 1]) {
            brands.insert(key(&parts[..parts.len() - 1].join("-")));
        }
        brands.remove("");
        return Some(Publisher {
            namespace: ns.to_string(),
            name: owner.to_string(),
            verified: Verified::Github,
            brands,
        });
    }
    let labels: Vec<&str> = ns.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {
        return None;
    }
    let domain = labels
        .iter()
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join(".")
        .to_lowercase();
    let brands = host_brand(&domain).into_iter().collect();
    Some(Publisher {
        namespace: ns.to_string(),
        name: domain,
        verified: Verified::Domain,
        brands,
    })
}

/// The product a registry name is about, from the part after the namespace:
/// `posthog-mcp-server` is "posthog". Empty when that part says nothing
/// (`io.github.PostHog/mcp`).
fn title_of(name: &str) -> String {
    let mut tail = name
        .split_once('/')
        .map(|(_, t)| t)
        .unwrap_or("")
        .to_lowercase();
    for affix in ["-mcp-server", "-mcp", "_mcp", "-server"] {
        if let Some(t) = tail.strip_suffix(affix) {
            tail = t.to_string();
        }
    }
    for affix in ["mcp-server-", "mcp-", "mcp_"] {
        if let Some(t) = tail.strip_prefix(affix) {
            tail = t.to_string();
        }
    }
    if ["", "mcp", "server", "mcp-server"].contains(&tail.as_str()) {
        String::new()
    } else {
        tail
    }
}

/// The keys an entry's product goes by: its title, and the product its registry name
/// names. "Atlassian Rovo" from `com.atlassian/atlassian-mcp-server` is
/// `atlassianrovo` and `atlassian`.
pub fn product_keys(name: &str, title: &str) -> BTreeSet<String> {
    [key(title), key(&title_of(name))]
        .into_iter()
        .filter(|k| !k.is_empty())
        .collect()
}

/// The entry is the product maker's own, and why Ghost thinks so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maker {
    /// "PostHog".
    pub product: String,
    pub evidence: Vec<Evidence>,
    /// The maker's own registered domains: "posthog.com".
    pub domains: BTreeSet<String>,
    /// What other entries for the same product are titled, as keys.
    pub keys: BTreeSet<String>,
}

/// Which hosts are the maker's own, by the way its publisher was checked.
enum Own {
    Listed(&'static [&'static str]),
    Github(BTreeSet<String>),
    Domain(String),
}

impl Own {
    fn own(&self, host: &str) -> bool {
        match self {
            Own::Listed(domains) => {
                registered_domain(host).is_some_and(|d| domains.contains(&d.as_str()))
            }
            Own::Github(brands) => host_brand(host).is_some_and(|b| brands.contains(&b)),
            Own::Domain(proven) => registered_domain(host).as_deref() == Some(proven.as_str()),
        }
    }
}

/// Whether an entry is its product maker's own. The publisher's name must be the
/// product's, and the entry must connect to the maker's own host (a domain publisher's
/// host under that exact domain; a GitHub account's host registered under its name), or
/// its namespace is in [`KNOWN_MAKERS`]. A repository or a website is said, never proof.
pub fn maker_of(
    name: &str,
    title: &str,
    hosts: &[&str],
    repository: Option<&str>,
    website: Option<&str>,
    publisher: Option<&Publisher>,
) -> Option<Maker> {
    let found;
    let publisher = match publisher {
        Some(p) => p,
        None => {
            found = publisher_of(name)?;
            &found
        }
    };
    let keys = product_keys(name, title);
    let listed = known_maker(&publisher.namespace);
    let mut domains: BTreeSet<String> = BTreeSet::new();
    let brands: BTreeSet<String>;
    let product: String;
    let own: Own;
    if let Some(listed) = listed {
        brands = listed.brands.iter().map(|b| b.to_string()).collect();
        let mut named = brands.clone();
        named.insert(key(listed.product));
        if keys.is_disjoint(&named) {
            return None; // the maker's, but about something else: no mark
        }
        product = listed.product.to_string();
        domains.extend(listed.domains.iter().map(|d| d.to_string()));
        own = Own::Listed(listed.domains);
    } else if publisher.verified == Verified::Github {
        // Only the exact name: `posthog-hq` is not PostHog.
        brands = BTreeSet::from([key(&publisher.name)]);
        if keys.is_disjoint(&brands) {
            return None;
        }
        product = title.to_string();
        own = Own::Github(brands.clone());
    } else {
        // A domain proves its owner, and only that domain: `ai.posthog` publishing
        // "PostHog" owns posthog.ai, which says nothing about posthog.com.
        brands = publisher.brands.clone();
        if keys.is_disjoint(&brands) {
            return None;
        }
        product = title.to_string();
        let proven = registered_domain(&publisher.name)?;
        domains.insert(proven.clone());
        own = Own::Domain(proven);
    }

    let mut evidence = vec![if listed.is_some() {
        Evidence::Listed
    } else {
        Evidence::Publisher
    }];
    let mine: Vec<String> = hosts
        .iter()
        .filter(|h| own.own(h))
        .filter_map(|h| registered_domain(h))
        .collect();
    if !mine.is_empty() {
        evidence.push(Evidence::Remote);
        domains.extend(mine);
    } else if listed.is_none() {
        return None; // the publisher's name alone is not enough
    }
    // Said beside the proof, never instead of it. A GitHub account's repository is always
    // its own, so for one it says nothing.
    if let Some(owner) = repository.and_then(repository_owner)
        && publisher.verified == Verified::Domain
        && brands.contains(&key(&owner))
    {
        evidence.push(Evidence::Repository);
    }
    if let Some(site) = website.and_then(url_host)
        && own.own(&site)
    {
        evidence.push(Evidence::Website);
    }
    // Others fold under it by its title, its product's name, or the maker's own name in the
    // registry name ("atlassian" for "Atlassian Rovo"); never by another product the
    // registry name happens to mention (`com.foo/posthog` titled "Foo" is not PostHog's).
    let mut group: BTreeSet<String> = BTreeSet::from([key(title), key(&product)]);
    group.extend(keys.intersection(&brands).cloned());
    group.remove("");
    Some(Maker {
        product,
        evidence,
        domains,
        keys: group,
    })
}

/// Whose host an entry connects to: the product maker's (one of its own registered
/// domains, exactly: `posthog.ai` is not PostHog's `posthog.com`), the publisher's,
/// or someone else's (a gateway, a shared host).
pub fn host_owner(host: &str, maker: Option<&Maker>, publisher: Option<&Publisher>) -> HostOwner {
    let Some(domain) = registered_domain(host) else {
        return HostOwner::Other;
    };
    if maker.is_some_and(|m| m.domains.contains(&domain)) {
        return HostOwner::Maker;
    }
    let Some(publisher) = publisher else {
        return HostOwner::Other;
    };
    if publisher.verified == Verified::Domain {
        // The domain the registry checked, and nothing else under the same name.
        return if registered_domain(&publisher.name).as_deref() == Some(domain.as_str()) {
            HostOwner::Publisher
        } else {
            HostOwner::Other
        };
    }
    if host_brand(host).is_some_and(|b| publisher.brands.contains(&b)) {
        HostOwner::Publisher
    } else {
        HostOwner::Other
    }
}

#[cfg(test)]
mod tests {
    //! Who publishes a catalog entry, whether it is the product maker's own, where it runs,
    //! and the folding of one product's entries under the maker's (#37). The fixture is the
    //! six records Ghost's copy of the registry gave for "posthog" on 2026-10-28.

    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::*;
    use crate::catalog::{Catalog, CatalogEntry, Kind, Registry, parse_entry};

    const POSTHOG_SIX_JSON: &str = include_str!("../tests/fixtures/catalog_posthog.json");

    fn posthog_six() -> Vec<Value> {
        serde_json::from_str(POSTHOG_SIX_JSON).unwrap()
    }

    fn record(server: Value) -> Value {
        let mut s = json!({"version": "1.0.0", "description": ""});
        for (k, v) in server.as_object().unwrap() {
            s[k] = v.clone();
        }
        json!({"server": s, "_meta": {"io.modelcontextprotocol.registry/official": {}}})
    }

    fn remote(url: &str) -> Value {
        json!([{"type": "streamable-http", "url": url}])
    }

    fn npm(identifier: &str) -> Value {
        json!([{"registryType": "npm", "identifier": identifier, "transport": {"type": "stdio"}}])
    }

    fn catalog(dir: &Path, records: &[Value]) -> Catalog {
        let path = dir.join("catalog.json");
        let by_name: BTreeMap<String, &Value> = records
            .iter()
            .map(|r| (r["server"]["name"].as_str().unwrap().to_string(), r))
            .collect();
        std::fs::write(
            &path,
            serde_json::to_string(&json!({"records": by_name})).unwrap(),
        )
        .unwrap();
        Catalog::new(path)
    }

    fn entry(rec: &Value) -> CatalogEntry {
        parse_entry(rec).expect("an entry")
    }

    fn entries(records: &[Value]) -> BTreeMap<String, CatalogEntry> {
        records
            .iter()
            .map(entry)
            .map(|e| (e.name.clone(), e))
            .collect()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn names(found: &[Arc<CatalogEntry>]) -> Vec<&str> {
        found.iter().map(|e| e.name.as_str()).collect()
    }

    // --- the namespace ----------------------------------------------------------------

    #[test]
    fn the_namespace_says_who_published_it() {
        for (name, shown, verified) in [
            ("io.github.PostHog/mcp", "PostHog", Verified::Github),
            (
                "io.github.pipeworx-io/posthog",
                "pipeworx-io",
                Verified::Github,
            ),
            ("com.mcparmory/posthog", "mcparmory.com", Verified::Domain),
            (
                "tech.engageable/analytics",
                "engageable.tech",
                Verified::Domain,
            ),
            ("com.figma.mcp/mcp", "mcp.figma.com", Verified::Domain),
        ] {
            let p = publisher_of(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!((p.name.as_str(), p.verified), (shown, verified), "{name}");
        }
    }

    #[test]
    fn a_name_without_a_namespace_has_no_publisher() {
        for name in [
            "nonamespace",
            "/x",
            "io.github./x",
            "io.github.a.b/x",
            "com/x",
        ] {
            assert!(publisher_of(name).is_none(), "{name}");
        }
    }

    #[test]
    fn a_company_account_on_github_may_run_hosts_under_its_name() {
        assert_eq!(
            publisher_of("io.github.pipeworx-io/posthog")
                .unwrap()
                .brands,
            set(&["pipeworxio", "pipeworx"])
        );
        assert_eq!(
            publisher_of("io.github.PostHog/mcp").unwrap().brands,
            set(&["posthog"])
        );
    }

    #[test]
    fn a_hosts_brand() {
        for (host, brand) in [
            ("mcp.posthog.com", Some("posthog")),
            ("gateway.pipeworx.io", Some("pipeworx")),
            ("mcp.linear.app", Some("linear")),
            ("api.example.co.uk", Some("example")),
            ("example.io", Some("example")),
            ("someone.vercel.app", None), // shared: the name is someone's, not Vercel's
            ("x.y.workers.dev", None),
            ("127.0.0.1", None),
            ("localhost", None),
        ] {
            assert_eq!(host_brand(host).as_deref(), brand, "{host}");
        }
    }

    #[test]
    fn addresses_and_repositories() {
        assert_eq!(
            url_host("https://www.PostHog.com:443/mcp?x=1").as_deref(),
            Some("posthog.com")
        );
        assert_eq!(
            url_host("https://user@mcp.posthog.com/mcp").as_deref(),
            Some("mcp.posthog.com")
        );
        assert!(url_host("not a url").is_none() && url_host("").is_none());
        assert_eq!(
            repository_owner("https://github.com/PostHog/posthog").as_deref(),
            Some("PostHog")
        );
        assert_eq!(
            repository_owner("https://gitlab.com/acme/tool.git").as_deref(),
            Some("acme")
        );
        assert!(repository_owner("https://example.com/PostHog/posthog").is_none());
        assert!(repository_owner("https://github.com/").is_none());
    }

    // --- the six PostHog results ------------------------------------------------------

    #[test]
    fn of_the_six_posthog_results_only_posthogs_own_is_the_makers() {
        let entries = entries(&posthog_six());
        assert_eq!(entries.len(), 6);
        let own = &entries["io.github.PostHog/mcp"];
        let maker = own.maker.as_ref().expect("the maker's own");
        assert_eq!(maker.product, "PostHog");
        assert_eq!(maker.evidence, vec![Evidence::Publisher, Evidence::Remote]);
        for (name, e) in &entries {
            if name != &own.name {
                assert!(e.maker.is_none(), "{name}");
            }
        }
        let who: BTreeMap<&str, (&str, &str)> = entries
            .iter()
            .map(|(n, e)| {
                let p = e.publisher.as_ref().unwrap();
                (n.as_str(), (p.name.as_str(), p.verified.as_str()))
            })
            .collect();
        assert_eq!(
            who,
            BTreeMap::from([
                ("io.github.PostHog/mcp", ("PostHog", "github")),
                ("com.mcparmory/posthog", ("mcparmory.com", "domain")),
                (
                    "io.github.friendlygeorge/posthog-mcp-server",
                    ("friendlygeorge", "github")
                ),
                ("io.github.mrfentmen/posthog-mcp", ("mrfentmen", "github")),
                ("io.github.pipeworx-io/posthog", ("pipeworx-io", "github")),
                ("tech.engageable/analytics", ("engageable.tech", "domain")),
            ])
        );
    }

    #[test]
    fn where_each_posthog_result_runs() {
        let entries = entries(&posthog_six());
        let own = &entries["io.github.PostHog/mcp"];
        let hosts: Vec<Option<String>> = own.options.iter().map(|o| o.host()).collect();
        assert_eq!(hosts, vec![Some("mcp.posthog.com".to_string())]);
        assert_eq!(
            host_owner(
                "mcp.posthog.com",
                own.maker.as_ref(),
                own.publisher.as_ref()
            ),
            HostOwner::Maker
        );
        let gateway = &entries["io.github.pipeworx-io/posthog"];
        let hosts: Vec<Option<String>> = gateway.options.iter().map(|o| o.host()).collect();
        assert_eq!(hosts, vec![Some("gateway.pipeworx.io".to_string())]);
        // Judged against PostHog: the publisher's own gateway, not PostHog's servers.
        let owner = host_owner(
            "gateway.pipeworx.io",
            own.maker.as_ref(),
            gateway.publisher.as_ref(),
        );
        assert_eq!(owner, HostOwner::Publisher);
        type Package<'a> = (Option<&'a str>, Option<&'a str>);
        let packages: BTreeMap<&str, Vec<Package>> = entries
            .iter()
            .filter(|(_, e)| e.options.iter().all(|o| o.kind == Kind::Local))
            .map(|(n, e)| {
                (
                    n.as_str(),
                    e.options
                        .iter()
                        .map(|o| (o.registry.map(Registry::as_str), o.identifier.as_deref()))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            packages,
            BTreeMap::from([
                (
                    "com.mcparmory/posthog",
                    vec![
                        (Some("pypi"), Some("mcparmory-posthog")),
                        (Some("oci"), Some("ghcr.io/mcparmory/posthog:1.0.5")),
                    ]
                ),
                (
                    "io.github.friendlygeorge/posthog-mcp-server",
                    vec![(Some("npm"), Some("posthog-mcp-server"))]
                ),
                (
                    "io.github.mrfentmen/posthog-mcp",
                    vec![(Some("npm"), Some("posthog-mcp"))]
                ),
                (
                    "tech.engageable/analytics",
                    vec![(Some("pypi"), Some("engageable"))]
                ),
            ])
        );
        assert_eq!(own.updated.as_deref(), Some("2026-01-07T16:34:40.847754Z"));
    }

    #[test]
    fn the_six_fold_under_posthogs_own_and_it_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let cat = catalog(dir.path(), &posthog_six());
        let (found, total) = cat.search("posthog", 40, 0);
        assert_eq!(total, 2);
        let [lead, other] = found.as_slice() else {
            panic!("two results")
        };
        assert_eq!(lead.entry.name, "io.github.PostHog/mcp");
        let mut others = names(&lead.others);
        others.sort();
        assert_eq!(
            others,
            vec![
                "com.mcparmory/posthog",
                "io.github.friendlygeorge/posthog-mcp-server",
                "io.github.mrfentmen/posthog-mcp",
                "io.github.pipeworx-io/posthog",
            ]
        );
        // "Engageable Analytics" mentions PostHog; it is another product, so it stands alone.
        assert_eq!(other.entry.name, "tech.engageable/analytics");
        assert!(other.others.is_empty());
        let pipeworx = cat.get("io.github.pipeworx-io/posthog").unwrap();
        assert!(Arc::ptr_eq(
            &cat.product_maker(&pipeworx).unwrap(),
            &lead.entry
        ));
        assert!(Arc::ptr_eq(
            &cat.product_maker(&lead.entry).unwrap(),
            &lead.entry
        ));
        assert!(
            cat.product_maker(&cat.get("tech.engageable/analytics").unwrap())
                .is_none()
        );
    }

    /// `ai.posthog/mcp` titled "PostHog", with no remote and no repository, beside
    /// PostHog's own: it isn't the maker's, it folds under PostHog's own like any other
    /// publisher's, and PostHog's own leads.
    #[test]
    fn a_look_alike_domain_with_no_remote_neither_leads_nor_folds_the_real_one() {
        let dir = tempfile::tempdir().unwrap();
        let fake = record(
            json!({"name": "ai.posthog/mcp", "title": "PostHog", "packages": npm("ph-mcp")}),
        );
        let mut records = posthog_six();
        records.push(fake);
        let cat = catalog(dir.path(), &records);
        assert!(cat.get("ai.posthog/mcp").unwrap().maker.is_none());
        let (found, total) = cat.search("posthog", 40, 0);
        assert_eq!(total, 2);
        let lead = &found[0];
        assert_eq!(lead.entry.name, "io.github.PostHog/mcp");
        assert!(names(&lead.others).contains(&"ai.posthog/mcp"));
        assert!(Arc::ptr_eq(
            &cat.product_maker(&cat.get("ai.posthog/mcp").unwrap())
                .unwrap(),
            &lead.entry
        ));
    }

    /// Two makers' own entries for one product: the search may rank either first, but the
    /// others fold under the one `product_maker` names, so the list and the detail agree.
    #[test]
    fn the_fold_leads_with_the_maker_the_detail_names() {
        let dir = tempfile::tempdir().unwrap();
        let gh = record(json!({
            "name": "io.github.PostHog/mcp",
            "title": "PostHog",
            "remotes": remote("https://mcp.posthog.com/mcp"),
            "websiteUrl": "https://posthog.com",
        }));
        let dom = record(json!({
            "name": "com.posthog/x", "title": "PostHog", "remotes": remote("https://x.posthog.com")
        }));
        let wrapper = record(json!({"name": "io.github.someone/posthog", "packages": npm("ph")}));
        let cat = catalog(dir.path(), &[gh, dom, wrapper]);
        let (found, _) = cat.search("posthog", 40, 0);
        assert_eq!(found[0].entry.name, "com.posthog/x"); // ranked first by name
        let chosen = cat.product_maker(&cat.get("io.github.someone/posthog").unwrap());
        assert_eq!(
            chosen.map(|e| e.name.clone()).as_deref(),
            Some("io.github.PostHog/mcp")
        ); // most evidence
        let folds: BTreeMap<&str, Vec<&str>> = found
            .iter()
            .map(|f| (f.entry.name.as_str(), names(&f.others)))
            .collect();
        assert_eq!(
            folds,
            BTreeMap::from([
                ("com.posthog/x", vec![]),
                ("io.github.PostHog/mcp", vec!["io.github.someone/posthog"]),
            ])
        );
    }

    #[test]
    fn without_the_makers_own_nothing_folds() {
        let dir = tempfile::tempdir().unwrap();
        let records: Vec<Value> = posthog_six()
            .into_iter()
            .filter(|r| r["server"]["name"] != "io.github.PostHog/mcp")
            .collect();
        let cat = catalog(dir.path(), &records);
        let (found, total) = cat.search("posthog", 40, 0);
        assert_eq!(total, 5);
        assert!(found.iter().all(|f| f.others.is_empty()));
    }

    // --- the maker's own: edge cases --------------------------------------------------

    /// `io.github.PostHog` publishing a package with no host of PostHog's: nothing but the
    /// name agrees, so no mark. Its repository and website say nothing: every GitHub account
    /// owns repositories under its own name, and anyone can write any website.
    #[test]
    fn a_github_name_alone_is_not_enough() {
        assert!(maker_of("io.github.PostHog/tool", "PostHog", &[], None, None, None).is_none());
        assert!(
            maker_of(
                "io.github.PostHog/tool",
                "PostHog",
                &[],
                Some("https://github.com/PostHog/x"),
                Some("https://posthog.com"),
                None,
            )
            .is_none()
        );
        let m = maker_of(
            "io.github.PostHog/tool",
            "PostHog",
            &["mcp.posthog.com"],
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(m.evidence, vec![Evidence::Publisher, Evidence::Remote]);
        assert_eq!(m.domains, set(&["posthog.com"]));
    }

    /// `io.github.notion` is a GitHub account named notion; Notion's is makenotion. Its
    /// repository is its own, as every account's is: no mark.
    #[test]
    fn a_look_alike_github_account_with_its_own_repository_is_not_the_maker() {
        assert!(
            maker_of(
                "io.github.notion/notion",
                "Notion",
                &[],
                Some("https://github.com/notion/notion-mcp"),
                Some("https://notion.so"),
                None,
            )
            .is_none()
        );
        let real = maker_of(
            "io.github.makenotion/notion-mcp-server",
            "Notion",
            &[],
            Some("https://github.com/makenotion/notion-mcp-server"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(real.evidence, vec![Evidence::Listed]);
    }

    #[test]
    fn a_proven_domain_needs_a_host_under_it() {
        assert!(maker_of("com.notion/mcp", "Notion", &[], None, None, None).is_none()); // the name alone
        let m = maker_of(
            "com.notion/mcp",
            "Notion",
            &["mcp.notion.com"],
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(m.evidence, vec![Evidence::Publisher, Evidence::Remote]);
        // The proven domain, under any subdomain: com.figma.mcp proved mcp.figma.com.
        assert!(
            maker_of(
                "com.figma.mcp/mcp",
                "Figma",
                &["mcp.figma.com"],
                None,
                None,
                None
            )
            .is_some()
        );
        assert!(
            maker_of(
                "com.example/mcp",
                "Example",
                &["api.example.co.uk"],
                None,
                None,
                None
            )
            .is_none()
        );
    }

    /// `ai.posthog` owns posthog.ai, not posthog.com. Titled "PostHog" with no remote, or
    /// pointing at PostHog's own host, or at a host of its own: never the maker's own.
    #[test]
    fn the_brand_under_another_domain_ending_is_not_the_maker() {
        assert!(maker_of("ai.posthog/mcp", "PostHog", &[], None, None, None).is_none());
        assert!(
            maker_of(
                "ai.posthog/mcp",
                "PostHog",
                &[],
                Some("https://github.com/posthog/mcp"),
                Some("https://posthog.ai"),
                None,
            )
            .is_none()
        );
        assert!(
            maker_of(
                "ai.posthog/mcp",
                "PostHog",
                &["mcp.posthog.com"],
                None,
                None,
                None
            )
            .is_none()
        );
        let own = maker_of(
            "ai.posthog/mcp",
            "PostHog",
            &["mcp.posthog.ai"],
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(own.domains, set(&["posthog.ai"])); // the domain it proved
        // Judged against PostHog's own, posthog.ai is someone else's.
        let real = maker_of(
            "io.github.PostHog/mcp",
            "PostHog",
            &["mcp.posthog.com"],
            None,
            None,
            None,
        );
        let publisher = publisher_of("ai.posthog/mcp");
        assert_eq!(
            host_owner("mcp.posthog.ai", real.as_ref(), publisher.as_ref()),
            HostOwner::Publisher
        );
        assert_eq!(
            host_owner("mcp.posthog.com", real.as_ref(), publisher.as_ref()),
            HostOwner::Maker
        );
        assert_eq!(
            host_owner(
                "posthog.ai",
                real.as_ref(),
                publisher_of("com.x/y").as_ref()
            ),
            HostOwner::Other
        );
        // A domain publisher's own host is the domain it proved, not another with its name.
        assert_eq!(
            host_owner("posthog.com", None, publisher.as_ref()),
            HostOwner::Other
        );
    }

    #[test]
    fn the_publishers_name_must_be_the_products() {
        assert!(
            maker_of(
                "com.mcparmory/posthog",
                "Posthog",
                &["mcparmory.com"],
                None,
                None,
                None
            )
            .is_none()
        );
        // A look-alike account is not the maker, whatever its repository says.
        let look_alike = maker_of(
            "io.github.posthog-hq/posthog",
            "PostHog",
            &["mcp.posthog-hq.dev"],
            Some("https://github.com/posthog-hq/posthog"),
            None,
            None,
        );
        assert!(look_alike.is_none());
        // The title or the product the name names: "Atlassian Rovo" by atlassian.com.
        let m = maker_of(
            "com.atlassian/atlassian-mcp-server",
            "Atlassian Rovo",
            &["mcp.atlassian.com"],
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(m.product, "Atlassian Rovo");
        assert!(m.keys.contains("atlassian"));
        // The maker's own app named for another product claims nothing of that product.
        let foo = maker_of("com.foo/posthog", "Foo", &["mcp.foo.com"], None, None, None).unwrap();
        assert_eq!(foo.keys, set(&["foo"]));
    }

    /// PostHog's account with an entry served from a gateway: no host of PostHog's agrees,
    /// so no mark; judged against PostHog's own, the gateway is not PostHog's.
    #[test]
    fn the_makers_own_name_on_someone_elses_host() {
        assert!(
            maker_of(
                "io.github.PostHog/mcp",
                "PostHog",
                &["gateway.pipeworx.io"],
                Some("https://github.com/PostHog/posthog"),
                None,
                None,
            )
            .is_none()
        );
        let real = maker_of(
            "io.github.PostHog/mcp",
            "PostHog",
            &["mcp.posthog.com"],
            None,
            None,
            None,
        );
        let publisher = publisher_of("io.github.PostHog/mcp");
        assert_eq!(
            host_owner("gateway.pipeworx.io", real.as_ref(), publisher.as_ref()),
            HostOwner::Other
        );
    }

    #[test]
    fn a_shared_host_is_nobodys_brand() {
        let m = maker_of(
            "io.github.vercel-fan/vercel",
            "Vercel",
            &["vercel-fan.vercel.app"],
            None,
            None,
            None,
        );
        assert!(m.is_none());
        let publisher = publisher_of("io.github.vercel-fan/vercel");
        assert_eq!(
            host_owner("vercel-fan.vercel.app", None, publisher.as_ref()),
            HostOwner::Other
        );
    }

    #[test]
    fn ghosts_own_list_names_makers_with_unusual_names() {
        let sentry = maker_of(
            "io.github.getsentry/sentry-mcp",
            "Sentry",
            &["mcp.sentry.dev"],
            Some("https://github.com/getsentry/sentry-mcp"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(sentry.product, "Sentry");
        assert_eq!(sentry.evidence, vec![Evidence::Listed, Evidence::Remote]);
        let github = maker_of(
            "io.github.github/github-mcp-server",
            "GitHub",
            &["api.githubcopilot.com"],
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(github.evidence, vec![Evidence::Listed, Evidence::Remote]);
        // The maker's, but about another product: no mark.
        assert!(
            maker_of(
                "io.github.github/spaces",
                "Copilot Spaces",
                &[],
                None,
                None,
                None
            )
            .is_none()
        );
        // A maker whose own app is a package: the list is the evidence.
        let paypal = maker_of(
            "io.github.paypal/paypal-mcp-server",
            "Paypal",
            &[],
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(paypal.product, "PayPal");
        assert_eq!(paypal.evidence, vec![Evidence::Listed]);
        // Neon proved neon.com and runs on neon.tech; both are its own.
        let neon = maker_of("com.neon/mcp", "Neon", &["mcp.neon.tech"], None, None, None).unwrap();
        assert_eq!(neon.evidence, vec![Evidence::Listed, Evidence::Remote]);
    }

    #[test]
    fn the_registered_domain() {
        for (host, domain) in [
            ("mcp.posthog.com", Some("posthog.com")),
            ("api.example.co.uk", Some("example.co.uk")),
            ("co.uk", Some("co.uk")),
            ("x.vercel.app", None),
            ("10.0.0.1", None),
        ] {
            assert_eq!(registered_domain(host).as_deref(), domain, "{host}");
        }
    }

    // --- grouping ---------------------------------------------------------------------

    #[test]
    fn folding_is_by_exact_title_only() {
        let dir = tempfile::tempdir().unwrap();
        let linear = record(json!({
            "name": "app.linear/linear",
            "title": "Linear",
            "remotes": remote("https://mcp.linear.app/mcp"),
        }));
        let wrapper =
            record(json!({"name": "io.github.someone/linear-mcp", "packages": npm("linear-mcp")}));
        let by_them = record(json!({
            "name": "io.usefulapi/linear",
            "title": "Linear by usefulapi",
            "remotes": remote("https://linear.usefulapi.io/mcp"),
        }));
        let project = record(json!({
            "name": "org.openlyuseful/linear-project",
            "title": "Linear Project",
            "packages": npm("lp"),
        }));
        let weather_a =
            record(json!({"name": "io.github.a/weather", "packages": npm("weather-a")}));
        let weather_b =
            record(json!({"name": "io.github.b/weather", "packages": npm("weather-b")}));
        let cat = catalog(
            dir.path(),
            &[linear, wrapper, by_them, project, weather_a, weather_b],
        );
        let (found, total) = cat.search("linear", 40, 0);
        assert_eq!(found[0].entry.name, "app.linear/linear");
        assert_eq!(
            names(&found[0].others),
            vec!["io.github.someone/linear-mcp"]
        );
        assert_eq!(total, 3); // Linear with its wrapper, "Linear by usefulapi", "Linear Project"
        // Two entries titled alike with no maker's own among them stay apart.
        let (found, total) = cat.search("weather", 40, 0);
        assert_eq!(total, 2);
        assert!(found.iter().all(|f| f.others.is_empty()));
        // A gateway for Linear is judged against Linear's own.
        let usefulapi = cat.get("io.usefulapi/linear").unwrap();
        assert!(cat.product_maker(&usefulapi).is_none()); // its title isn't exactly Linear's
    }

    #[test]
    fn searching_github_does_not_find_every_github_account() {
        let dir = tempfile::tempdir().unwrap();
        let notes = record(
            json!({"name": "io.github.someone/notes", "title": "Notes", "packages": npm("notes")}),
        );
        let gh = record(json!({
            "name": "io.github.github/github-mcp-server",
            "title": "GitHub",
            "remotes": remote("https://api.githubcopilot.com/mcp/"),
        }));
        let cat = catalog(dir.path(), &[notes, gh]);
        let (found, total) = cat.search("github", 40, 0);
        assert_eq!(total, 1);
        assert_eq!(found[0].entry.name, "io.github.github/github-mcp-server");
        assert_eq!(cat.search("someone", 40, 0).1, 1); // the account still counts
    }
}
