# 0076: An app signs in as a registered client; Ghost lists the apps the registry lacks

- **Status:** accepted
- **Date:** 2026-11-15
- **Amends:** ADR 0034 (how Ghost signs in to an app, and where Apps' entries come from)

Ghost signed in to an app by registering itself with the app's authorization server
(dynamic client registration), and nothing else. Notion, Linear, Atlassian, Sentry and Stripe
take that; GitHub, Google (Gmail, Calendar, Drive) and Slack do not. They let in only a client
registered with them beforehand, and Ghost could neither ship one nor take the person's own
(#165). The sign-in then failed with the bare kind `RegistrationFailed`. Google's and Slack's
first-party apps are not in the public registry either, so Apps never offered them. A
competitive review put connecting an app at about 6 out of 10 for this.

What the vendors say (checked 2026-11-15; the PR that made this ADR has the links):

- **GitHub.** `api.githubcopilot.com/mcp/` names `https://github.com/login/oauth` as its
  authorization server, whose metadata has no registration endpoint and offers the device
  flow. GitHub's web flow needs the client secret at the token endpoint even with PKCE; its
  device flow needs only the client ID, and a device-flow token refreshes without the secret.
  GitHub's own guide says a host must register a GitHub App or an OAuth App to sign in there.
- **Google.** The Workspace apps (`gmailmcp`, `calendarmcp`, `drivemcp` `.googleapis.com/mcp/v1`)
  name `accounts.google.com`, with no registration. Google's guides ask for a "Web application"
  client marked "for an AI-powered agent", its secret, the Developer Preview, and give the
  scopes to ask for. Without `access_type=offline` Google gives no refresh token.
- **Slack.** `mcp.slack.com/mcp` is its own authorization server, with no registration; a Slack
  app of the workspace (internal, or in the Marketplace), approved by its admin. Its guide asks
  for the client ID only; its metadata takes a secret in the body only.

## Decision

**A sign-in chooses its client in a fixed order** (`ghost_core::signin_clients::choose`):

1. **The person's own client** for this app: a client ID they registered with the service and
   gave Ghost on the app's page or when connecting it. It is `client_id` in the app's entry in
   `apps.json`. Its secret, where the service wants one, is a password in the vault (ADR 0036),
   named by `client_secret` as `${NAME}` and nothing else; a value written there is refused.
2. **Ghost's own client for that authorization server**, from `KNOWN_SERVERS`: a typed, closed
   table keyed by the server's issuer, matched exactly (one trailing slash aside, since Google's
   resource metadata writes one), never by substring. A row names the service, Ghost's public
   client ID once one is registered, how that client proves itself (`Pkce`, or `DeviceFlow` for
   a server whose code flow wants a secret), what the person's own client needs (no secret, an
   optional one, a required one), whether the secret goes in the body, the vendor's page where a
   person makes a client, and parameters a server needs on the browser's address (Google's
   offline access). Ghost ships no secret: anything in the binary is public. No row has a client
   ID until the maker registers one; adding it is one line and the table's test.
3. **Ghost's client metadata document**, where the server says it takes one
   (`client_id_metadata_document_supported`): `https://heyghost.app/oauth/client.json`, a static
   file in the site (`site/public/oauth/client.json`), whose URL is the client ID. It names Ghost
   and its loopback ways back, which the specification allows for a native client; a test holds
   it to what the sign-in sends.
4. **Dynamic registration**, where the server offers it, as before.

**When none is possible, the sign-in says so in words** (`AppError::NeedsClient`, the app's
state `needs_client`): which service, that it lets in only apps registered with it, and what to
do ("GitHub only lets in apps registered with it, and Ghost isn't yet. Register one of your own
with GitHub, then give Ghost its client ID and secret on GitHub's page and sign in again."). A
registration the server refused reads the same way. The sentence has no address in it; the
app's page links the vendor's page from the table. The desktop, the waiting list
(`app:<name>:client`) and `ghostd check` say it. The desktop shows the client ID field only
then, or under "I have my own client ID": never as a field everyone sees.

**A lapsed sign-in is one action.** A stored sign-in whose refresh the server refuses is
`needs_sign_in`, never an error's kind, and its waiting-list item (`app:<name>:sign-in`) has
one button, Sign in again.

**Apps the registry lacks are listed in-tree** (`catalog::NOT_IN_REGISTRY`): Gmail, Google
Calendar, Google Drive and Slack, each with its address checked against the vendor's guide,
its maker's domains, the scopes its guide asks for, and its authorization server's issuer.
Their names start `ghost/`, which no registry name can, so the registry never shadows one, and
they say they come from Ghost's list (`listed_by_ghost`), with no publisher, since nobody's
namespace was checked. They sort after `FEATURED`.

**The choice is Ghost's and the operator's only.** No tool, model, run, outside client, the
phone or an app's text sets a client ID or a secret (`no_tool_sets_a_sign_in_client`); the
operator's route is `PUT /apps/{name}/sign-in-client` and the client fields of `POST /apps`. A
pasted config's client is not taken, since another tool's client is that tool's (ADR 0068).
Vendor metadata is untrusted: a device flow's code must look like one, its page must be on the
issuer's own host over https, every answer is bounded, and its waits are capped.

## What leaves the machine

Nothing new. A sign-in talks to the app's authorization server as before; the client ID
document is fetched by that server from heyghost.app, not by Ghost. The secret stays in the
vault and in the daemon's memory while a sign-in or a refresh uses it, and is never written to
`oauth.json`, a log, the audit or a response.

## Costs (ADR 0018)

Nothing on a turn: the choice is made at sign-in. Four more catalog entries, built in memory
when the index is.

## Considered options

- **Ship Ghost's own GitHub and Google clients with their secrets.** Rejected: a secret in a
  desktop binary is not secret, and GitHub's device flow needs none. Google's agent clients
  are "Web application" ones with a confidential secret, so Google waits on the person's own
  client until Google offers something else.
- **A field for the client ID on every app.** Rejected: most apps need none, and a field that
  does nothing for them is noise.
- **Read the client from another tool's config** (Claude Code's `oauth.clientId`). Rejected by
  ADR 0068: that client is the other product's.
