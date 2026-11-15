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
//! 3. **Ghost's client metadata document** ([`CIMD_URL`]), when the server says it takes one
//!    (`client_id_metadata_document_supported`): a static file on heyghost.app that names
//!    Ghost and its loopback ways back, so the URL is the client ID.
//! 4. **Dynamic registration**, when the server offers it.
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
    /// Ghost's own client there, once one is registered. None until then.
    pub ghost_client: Option<GhostClient>,
    /// What the person's own client needs.
    pub own_secret: SecretNeed,
    /// The secret goes in the token request's body, not as HTTP Basic.
    pub secret_in_body: bool,
    /// The vendor's page where the person makes a client of their own.
    pub make_a_client: &'static str,
    /// Added to the address the browser opens, for this server only.
    pub authorize_params: &'static [(&'static str, &'static str)],
}

/// The authorization servers Ghost knows. Adding Ghost's own client for one is one line:
/// its `ghost_client`. Keep every row checked against the vendor's own documentation.
pub const KNOWN_SERVERS: &[KnownServer] = &[
    // github.com/login/oauth: no dynamic registration. An OAuth App's web flow needs the
    // client secret at the token endpoint, even with PKCE; the device flow does not, and a
    // device-flow token refreshes without it. So Ghost's own client, once registered, is a
    // device-flow client.
    KnownServer {
        issuer: "https://github.com/login/oauth",
        service: "GitHub",
        ghost_client: None,
        own_secret: SecretNeed::Required,
        secret_in_body: true,
        make_a_client: "https://github.com/settings/applications/new",
        authorize_params: &[],
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

/// The person's own client for one app, its secret already taken from the vault.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnClient {
    pub client_id: String,
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for OwnClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A secret never reaches a log line, even a debug one.
        f.debug_struct("OwnClient")
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "…"))
            .finish()
    }
}

/// What the authorization server says it takes, from its metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerTakes {
    /// `client_id_metadata_document_supported` is true.
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

/// The order of choice (ADR 0076): the person's own, Ghost's own for that server, the
/// metadata document where the server takes one, registration where it offers it.
pub fn choose(own: Option<&OwnClient>, server: Option<&KnownServer>, takes: ServerTakes) -> Choice {
    if let Some(own) = own.filter(|o| !o.client_id.trim().is_empty()) {
        return Choice::Own(own.clone());
    }
    if let Some(ghost) = server.and_then(|s| s.ghost_client) {
        return Choice::Ghost(ghost);
    }
    if takes.metadata_document {
        return Choice::MetadataDocument;
    }
    if takes.registration {
        return Choice::Register;
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
        assert_eq!(choose(None, None, all), Choice::MetadataDocument);
        assert_eq!(
            choose(
                None,
                Some(&KNOWN_SERVERS[0]),
                ServerTakes {
                    metadata_document: false,
                    registration: true
                }
            ),
            Choice::Register
        );
        assert_eq!(
            choose(None, Some(&KNOWN_SERVERS[0]), ServerTakes::default()),
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
        };
        assert!(!format!("{c:?}").contains("very-secret-value"));
        assert!(!format!("{:?}", Choice::Own(c)).contains("very-secret-value"));
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
