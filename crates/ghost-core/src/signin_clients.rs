//! Which client Ghost signs in to an app as (ADR 0076).
//!
//! An app's authorization server only hands a sign-in to a client it knows. There are four
//! ways to be known, tried in this order:
//!
//! 1. **The person's own client**: a client ID they registered with the service themselves
//!    and gave Ghost for this app (`client_id` in `apps.json`, its secret, where the service
//!    demands one, a password in the vault named by `client_secret`, ADR 0036).
//! 2. **Ghost's own client** for that authorization server, from [`KNOWN_SERVERS`]: a
//!    public client ID registered by Ghost's maker, matched to the server's issuer exactly.
//!    Ghost ships no secret, so its own client is a public one (PKCE) or uses the device
//!    flow ([`ClientAuth`]).
//! 3. **Dynamic registration**, when the server offers it: what works for Notion, Linear,
//!    Atlassian and Sentry today, even where they also take a metadata document.
//! 4. **Ghost's client metadata document** ([`CIMD_URL`]), when the server takes one
//!    (`client_id_metadata_document_supported`) and offers no registration, and only when the
//!    document names this sign-in's way back: a static file on heyghost.app that names Ghost
//!    and its loopback ways back, so the URL is the client ID.
//!
//! When none of them is possible, the sign-in does not fail with an error's kind: it says,
//! in the product's words, which service needs a client and where the person makes one
//! ([`needs_a_client`]).
//!
//! The table and the choice are Ghost's and the operator's. Nothing an app, a model, a
//! tool, a run, an outside client or the phone sends sets a client ID or a secret, and
//! nothing from a vendor's metadata is shown without bounds.

/// Where Ghost's client metadata document is published (`site/public/oauth/client.json`).
pub const CIMD_URL: &str = "https://heyghost.app/oauth/client.json";

/// The document itself, as the site publishes it: a test holds its fields to what the
/// sign-in sends.
pub const CIMD_DOCUMENT: &str = include_str!("../../../site/public/oauth/client.json");

/// The ways back a client metadata document names (`redirect_uris`); none when it doesn't
/// read.
pub fn document_redirects(document: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(document)
        .ok()
        .and_then(|d| d.get("redirect_uris").cloned())
        .and_then(|r| serde_json::from_value::<Vec<String>>(r).ok())
        .unwrap_or_default()
}

/// How one of Ghost's own clients proves itself. There is no secret: whatever Ghost ships,
/// anyone can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuth {
    /// A public client: the authorization code with PKCE and no secret.
    Pkce,
    /// The device flow (RFC 8628): the person types a short code on the service's own
    /// page; the token is fetched with the client ID alone.
    DeviceFlow,
}

/// What the person's own client needs besides its ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretNeed {
    /// The service takes a public client: no secret.
    None,
    /// Some clients of this service have one; give it if yours does.
    Optional,
    /// The service's token endpoint refuses a client without its secret.
    Required,
}

impl SecretNeed {
    pub fn as_str(self) -> &'static str {
        match self {
            SecretNeed::None => "none",
            SecretNeed::Optional => "optional",
            SecretNeed::Required => "required",
        }
    }
}

/// A client Ghost's maker registered with a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhostClient {
    pub client_id: &'static str,
    pub auth: ClientAuth,
}

/// An authorization server Ghost knows by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownServer {
    /// The issuer, exactly as the server's metadata names it.
    pub issuer: &'static str,
    /// The service as the person knows it: "GitHub".
    pub service: &'static str,
    /// Ghost's own client there, where one is registered.
    pub ghost_client: Option<GhostClient>,
    /// What the person's own client needs.
    pub own_secret: SecretNeed,
    /// The secret goes in the token request's body, not as HTTP Basic.
    pub secret_in_body: bool,
    /// The vendor's page where the person makes a client of their own.
    pub make_a_client: &'static str,
    /// Added to the address the browser opens, for this server only.
    pub authorize_params: &'static [(&'static str, &'static str)],
    /// The apps this server signs in for, by host (`api.githubcopilot.com`). Ghost's own
    /// client and this row's parameters are used only for an app at one of them, so an app
    /// elsewhere that names this server cannot borrow Ghost's client and keep its token.
    pub resources: &'static [&'static str],
}

/// The authorization servers Ghost knows. Adding Ghost's own client for one is one line:
/// its `ghost_client`. Keep every row checked against the vendor's own documentation.
pub const KNOWN_SERVERS: &[KnownServer] = &[
    // github.com/login/oauth: no dynamic registration. An OAuth App's web flow needs the
    // client secret at the token endpoint, even with PKCE; the device flow does not, and a
    // device-flow token refreshes without it. So Ghost's own client is a device-flow client:
    // the OAuth App "Ghost" of the heka-vault organisation, with the device flow on, expiring
    // tokens and no secret ever generated. Tried on 2026-11-15 with a token of no scope:
    // GitHub's endpoint took it (49 tools listed, a read answered), and its refresh token
    // was exchanged with the client ID alone for one the endpoint took as well.
    KnownServer {
        issuer: "https://github.com/login/oauth",
        service: "GitHub",
        ghost_client: Some(GhostClient {
            client_id: "Ov23liceMIaDjEZToyS8",
            auth: ClientAuth::DeviceFlow,
        }),
        own_secret: SecretNeed::Required,
        secret_in_body: true,
        make_a_client: "https://github.com/settings/applications/new",
        authorize_params: &[],
        resources: &["api.githubcopilot.com"],
    },
    // accounts.google.com: no dynamic registration. Google's Workspace apps (Gmail,
    // Calendar, Drive) take a "Web application" client marked for an AI-powered agent,
    // with its secret. Without `access_type=offline` Google gives no refresh token and the
    // person would sign in again every hour.
    KnownServer {
        issuer: "https://accounts.google.com",
        service: "Google",
        ghost_client: None,
        own_secret: SecretNeed::Required,
        secret_in_body: false,
        make_a_client: "https://console.cloud.google.com/auth/clients",
        authorize_params: &[("access_type", "offline"), ("prompt", "consent")],
        resources: &[
            "gmailmcp.googleapis.com",
            "calendarmcp.googleapis.com",
            "drivemcp.googleapis.com",
        ],
    },
    // mcp.slack.com: no dynamic registration; a Slack app of the person's workspace (an
    // internal app, or one in the Slack Marketplace), approved by its admin. Slack's
    // metadata takes a secret in the body only; its guide asks for the client ID alone.
    KnownServer {
        issuer: "https://mcp.slack.com",
        service: "Slack",
        ghost_client: None,
        own_secret: SecretNeed::Optional,
        secret_in_body: true,
        make_a_client: "https://api.slack.com/apps",
        authorize_params: &[],
        resources: &["mcp.slack.com"],
    },
];

/// An issuer as servers write it: `https://accounts.google.com/` and
/// `https://accounts.google.com` are one. Nothing else is loosened.
fn issuer_key(issuer: &str) -> &str {
    issuer.strip_suffix('/').unwrap_or(issuer)
}

/// The row for an issuer, matched exactly (one trailing slash aside), never by substring:
/// `https://github.com.evil.example/login/oauth` is not GitHub.
pub fn known_server(issuer: &str) -> Option<&'static KnownServer> {
    known_server_in(KNOWN_SERVERS, issuer)
}

/// The same, in a given table.
pub fn known_server_in(
    servers: &'static [KnownServer],
    issuer: &str,
) -> Option<&'static KnownServer> {
    let wanted = issuer_key(issuer);
    servers.iter().find(|s| issuer_key(s.issuer) == wanted)
}

/// Whether `server`'s row serves the app at `app_url`: its host (and port, when it isn't
/// the scheme's own) is one the row names, exactly. No user or password in the address, no
/// other port, no trailing dot, and https only (plain http for a test's 127.0.0.1, which no
/// real row names). The host is compared as the URL standard writes it, lower case.
pub fn serves(server: &KnownServer, app_url: &str) -> bool {
    let Ok(u) = url::Url::parse(app_url) else {
        return false;
    };
    if !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    let Some(host) = u.host_str() else {
        return false;
    };
    let loopback = host == "127.0.0.1";
    if u.scheme() != "https" && !(loopback && u.scheme() == "http") {
        return false;
    }
    let authority = match u.port() {
        None => host.to_string(),
        Some(p) => format!("{host}:{p}"),
    };
    server.resources.iter().any(|r| *r == authority)
}

/// Whether Ghost has a client of its own for the app at `app_url`: a row serves that
/// address and has one. The catalog offers a sign-in by it; the sign-in itself still
/// matches the server the app names against the row ([`server_for_app`]).
pub fn ghost_signs_in_at(app_url: &str) -> bool {
    KNOWN_SERVERS
        .iter()
        .any(|s| s.ghost_client.is_some() && serves(s, app_url))
}

/// The row for this app: the server it names, and only when that server serves the app.
/// An app that names a server it isn't one of gets no row: no Ghost client, no parameters,
/// and the plain sentence of an unknown server.
pub fn server_for_app(
    servers: &'static [KnownServer],
    issuer: Option<&str>,
    app_url: &str,
) -> Option<&'static KnownServer> {
    known_server_in(servers, issuer?).filter(|s| serves(s, app_url))
}

/// Whether the person's own client may go to `issuer`: the server they gave it for, as
/// recorded then, is this one (one trailing slash aside). Unknown on either side is no.
pub fn own_client_bound(own: &OwnClient, issuer: Option<&str>) -> bool {
    match (own.issuer.as_deref(), issuer) {
        (Some(given), Some(now)) => issuer_key(given) == issuer_key(now),
        _ => false,
    }
}

/// The sentence when the person's client was given for another server than the app names
/// now, or for one Ghost never recorded.
pub fn own_client_moved(app: &str, recorded: bool) -> String {
    if recorded {
        format!(
            "{app} now signs in somewhere other than where you gave Ghost your client ID, so \
             Ghost won't send it there. If that's expected, give Ghost the client ID again on \
             {app}'s page."
        )
    } else {
        format!(
            "Ghost doesn't know where {app} signed in when you gave it your client ID, so it \
             won't send it anywhere. Give Ghost the client ID again on {app}'s page."
        )
    }
}

/// The person's own client for one app, its secret already taken from the vault.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// The authorization server the person gave it for, as recorded then. A sign-in whose
    /// app now names another sends neither the ID nor the secret there.
    pub issuer: Option<String>,
}

impl std::fmt::Debug for OwnClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A secret never reaches a log line, even a debug one.
        f.debug_struct("OwnClient")
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "…"))
            .field("issuer", &self.issuer)
            .finish()
    }
}

/// What the authorization server says it takes, from its metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerTakes {
    /// `client_id_metadata_document_supported` is true, and Ghost's document names this
    /// sign-in's way back (it names port 47823 only; `GHOST_SIGNIN_PORT` may differ).
    pub metadata_document: bool,
    /// It names a `registration_endpoint`.
    pub registration: bool,
}

/// The client a sign-in uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Own(OwnClient),
    Ghost(GhostClient),
    MetadataDocument,
    Register,
    /// No way in: the person must register a client of their own.
    NeedsClient,
}

impl Choice {
    /// For the log: which way, never the ID or the secret.
    pub fn kind(&self) -> &'static str {
        match self {
            Choice::Own(_) => "own client",
            Choice::Ghost(GhostClient {
                auth: ClientAuth::DeviceFlow,
                ..
            }) => "Ghost's client (device)",
            Choice::Ghost(_) => "Ghost's client",
            Choice::MetadataDocument => "metadata document",
            Choice::Register => "registration",
            Choice::NeedsClient => "none",
        }
    }
}

/// The order of choice (ADR 0076): the person's own, Ghost's own for that server,
/// registration where it is offered, the metadata document where only that is taken.
pub fn choose(own: Option<&OwnClient>, server: Option<&KnownServer>, takes: ServerTakes) -> Choice {
    if let Some(own) = own.filter(|o| !o.client_id.trim().is_empty()) {
        return Choice::Own(own.clone());
    }
    if let Some(ghost) = server.and_then(|s| s.ghost_client) {
        return Choice::Ghost(ghost);
    }
    // Registration where it is offered: it is what works for those servers today, and no
    // live service has been tried with the document yet.
    if takes.registration {
        return Choice::Register;
    }
    if takes.metadata_document {
        return Choice::MetadataDocument;
    }
    Choice::NeedsClient
}

/// A client ID as the person may give it: printable, no spaces, bounded. Google's run to
/// about 75 characters; a metadata document's URL is longer.
pub fn client_id_problem(client_id: &str) -> Option<&'static str> {
    let id = client_id.trim();
    if id.is_empty() {
        return Some("a client ID can't be empty");
    }
    if id.chars().count() > 300 {
        return Some("a client ID is at most 300 characters");
    }
    if id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("a client ID has no spaces in it");
    }
    None
}

/// The sentence for a sign-in no client can make, in the product's words (app, sign in;
/// never the protocol's). `app` is the app's name as the person knows it.
pub fn needs_a_client(app: &str, server: Option<&KnownServer>) -> String {
    match server {
        Some(s) => {
            let what = match s.own_secret {
                SecretNeed::Required => "its client ID and secret",
                _ => "its client ID",
            };
            format!(
                "{service} only lets in apps registered with it, and Ghost isn't yet. \
                 Register one of your own with {service}, then give Ghost {what} on {app}'s \
                 page and sign in again.",
                service = s.service,
            )
        }
        None => format!(
            "{app} doesn't let Ghost register itself to sign in. If you have a client ID for \
             it, give it to Ghost on {app}'s page and sign in again."
        ),
    }
}

/// The sentence for a registration the server refused.
pub fn registration_refused(app: &str, server: Option<&KnownServer>) -> String {
    match server {
        Some(_) => needs_a_client(app, server),
        None => format!(
            "{app} refused to let Ghost register itself to sign in. If you have a client ID \
             for it, give it to Ghost on {app}'s page and sign in again."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn own(id: &str) -> OwnClient {
        OwnClient {
            client_id: id.into(),
            client_secret: None,
            issuer: None,
        }
    }

    #[test]
    fn the_order_of_choice() {
        let row = KnownServer {
            ghost_client: Some(GhostClient {
                client_id: "ghost-id",
                auth: ClientAuth::DeviceFlow,
            }),
            ..KNOWN_SERVERS[0]
        };
        let all = ServerTakes {
            metadata_document: true,
            registration: true,
        };
        assert_eq!(
            choose(Some(&own("mine")), Some(&row), all),
            Choice::Own(own("mine"))
        );
        assert!(matches!(
            choose(None, Some(&row), all),
            Choice::Ghost(GhostClient {
                client_id: "ghost-id",
                ..
            })
        ));
        // an empty one is not the person's choice
        assert!(matches!(
            choose(Some(&own("  ")), Some(&row), all),
            Choice::Ghost(_)
        ));
        // registration first where both are offered; the document only where it isn't
        let bare = KnownServer {
            ghost_client: None,
            ..KNOWN_SERVERS[0]
        };
        assert_eq!(choose(None, None, all), Choice::Register);
        assert_eq!(
            choose(
                None,
                None,
                ServerTakes {
                    metadata_document: true,
                    registration: false
                }
            ),
            Choice::MetadataDocument
        );
        assert_eq!(
            choose(
                None,
                Some(&bare),
                ServerTakes {
                    metadata_document: false,
                    registration: true
                }
            ),
            Choice::Register
        );
        assert_eq!(
            choose(None, Some(&bare), ServerTakes::default()),
            Choice::NeedsClient
        );
    }

    #[test]
    fn a_server_is_matched_by_its_issuer_exactly() {
        assert_eq!(
            known_server("https://github.com/login/oauth").map(|s| s.service),
            Some("GitHub")
        );
        assert_eq!(
            known_server("https://accounts.google.com/").map(|s| s.service),
            Some("Google")
        );
        for not in [
            "https://github.com.evil.example/login/oauth",
            "https://evil.example/https://github.com/login/oauth",
            "http://github.com/login/oauth",
            "https://github.com/login/oauth/x",
            "https://github.com",
            "https://accounts.google.com//",
            "",
        ] {
            assert!(known_server(not).is_none(), "{not}");
        }
    }

    #[test]
    fn ghost_signs_in_only_where_a_row_with_a_client_serves_the_address() {
        assert!(ghost_signs_in_at("https://api.githubcopilot.com/mcp/"));
        assert!(ghost_signs_in_at(
            "https://api.githubcopilot.com/mcp/readonly"
        ));
        for not in [
            "http://api.githubcopilot.com/mcp/",
            "https://api.githubcopilot.com.evil.example/mcp/",
            "https://evil.example/api.githubcopilot.com",
            "https://x@api.githubcopilot.com/mcp/",
            "https://api.githubcopilot.com:8443/mcp/",
            "https://gmailmcp.googleapis.com/mcp/v1", // a row, but no client of Ghost's
            "https://mcp.slack.com/mcp",
            "",
        ] {
            assert!(!ghost_signs_in_at(not), "{not}");
        }
    }

    #[test]
    fn the_table_ships_no_invented_client_and_every_row_is_whole() {
        let mut issuers = std::collections::HashSet::new();
        for s in KNOWN_SERVERS {
            assert!(issuers.insert(issuer_key(s.issuer)), "twice: {}", s.issuer);
            assert!(s.issuer.starts_with("https://"), "{}", s.issuer);
            assert!(s.make_a_client.starts_with("https://"), "{}", s.service);
            if let Some(c) = s.ghost_client {
                assert!(client_id_problem(c.client_id).is_none(), "{}", s.service);
            }
        }
    }

    #[test]
    fn the_sentences_say_what_to_do_in_the_products_words() {
        let github = known_server("https://github.com/login/oauth");
        let said = needs_a_client("GitHub", github);
        assert!(said.starts_with("GitHub only lets in apps registered with it"));
        assert!(
            said.contains("Register one of your own with GitHub"),
            "{said}"
        );
        // no address in a sentence: the app's page links the vendor's own
        assert!(!said.contains("https://"));
        assert!(said.contains("client ID and secret"));
        let slack = needs_a_client("Slack", known_server("https://mcp.slack.com"));
        assert!(slack.contains("its client ID on Slack's page"), "{slack}");
        let other = registration_refused("Acme", None);
        assert!(other.starts_with("Acme refused to let Ghost register"));
        for s in [said, slack, other, needs_a_client("Acme", None)] {
            let l = s.to_lowercase();
            for word in ["mcp", "server", "registrationfailed", "oauth"] {
                assert!(!l.contains(word), "{word} in {s}");
            }
        }
    }

    #[test]
    fn a_row_serves_only_the_apps_it_names_exactly() {
        let github = known_server("https://github.com/login/oauth").unwrap();
        assert!(serves(github, "https://api.githubcopilot.com/mcp/"));
        assert!(serves(github, "https://api.githubcopilot.com:443/mcp/"));
        // the same host, as a URL writes it in capitals
        assert!(serves(github, "https://API.githubcopilot.com/mcp/"));
        for not in [
            "https://evil.example/mcp",
            "https://api.githubcopilot.com.evil.example/mcp/",
            "https://evil.example/api.githubcopilot.com/mcp/",
            "https://evil.example/?u=https://api.githubcopilot.com/",
            "https://user@api.githubcopilot.com/mcp/",
            "https://user:pw@api.githubcopilot.com/mcp/",
            "https://api.githubcopilot.com:8443/mcp/",
            "https://api.githubcopilot.com./mcp/",
            "http://api.githubcopilot.com/mcp/",
            "https://githubcopilot.com/mcp/",
            "https://x.api.githubcopilot.com/mcp/",
            "http://127.0.0.1:47823/mcp",
            "not a url",
            "",
        ] {
            assert!(!serves(github, not), "{not}");
        }
        let google = known_server("https://accounts.google.com").unwrap();
        for app in crate::catalog::NOT_IN_REGISTRY
            .iter()
            .filter(|t| t.issuer == google.issuer)
        {
            assert!(serves(google, app.url), "{}", app.url);
        }
        assert!(!serves(google, "https://api.githubcopilot.com/mcp/"));
        let slack = known_server("https://mcp.slack.com").unwrap();
        assert!(serves(slack, "https://mcp.slack.com/mcp"));
    }

    #[test]
    fn a_client_id_is_checked_before_it_is_kept() {
        assert!(client_id_problem("Iv23liABC.apps").is_none());
        assert!(client_id_problem("").is_some());
        assert!(client_id_problem("has space").is_some());
        assert!(client_id_problem(&"x".repeat(301)).is_some());
    }

    #[test]
    fn a_secret_never_reaches_a_debug_line() {
        let c = OwnClient {
            client_id: "id".into(),
            client_secret: Some("very-secret-value".into()),
            issuer: None,
        };
        assert!(!format!("{c:?}").contains("very-secret-value"));
        assert!(!format!("{:?}", Choice::Own(c)).contains("very-secret-value"));
    }

    #[test]
    fn the_document_names_its_ways_back() {
        let ways = document_redirects(CIMD_DOCUMENT);
        assert!(ways.contains(&"http://127.0.0.1:47823/callback".to_string()));
        assert!(!ways.contains(&"http://127.0.0.1:50000/callback".to_string()));
        assert!(document_redirects("not json").is_empty());
    }

    #[test]
    fn the_published_document_is_what_the_sign_in_sends() {
        let doc: Value = serde_json::from_str(CIMD_DOCUMENT).unwrap();
        assert_eq!(doc["client_id"], CIMD_URL);
        assert_eq!(doc["client_name"], crate::signin::CLIENT_NAME);
        assert_eq!(doc["token_endpoint_auth_method"], "none");
        assert_eq!(doc["application_type"], "native");
        let redirects: Vec<&str> = doc["redirect_uris"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let s = crate::signin::SignIns::new("/nowhere");
        assert!(
            redirects.contains(&s.redirect_uri().as_str()),
            "{redirects:?}"
        );
        // loopback only: the browser comes back to this computer and nowhere else
        for r in &redirects {
            let u = url::Url::parse(r).unwrap();
            assert_eq!(u.scheme(), "http");
            assert!(
                matches!(u.host_str(), Some("127.0.0.1" | "localhost")),
                "{r}"
            );
        }
        assert!(
            doc["grant_types"]
                .as_array()
                .unwrap()
                .contains(&Value::from("authorization_code"))
        );
        // nothing that looks like a secret is published
        assert!(doc.get("client_secret").is_none());
    }
}
