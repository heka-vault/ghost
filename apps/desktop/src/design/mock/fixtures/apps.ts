/**
 * Owner: apps. What Ghost connects to (ADR 0034), as GET /apps and GET /catalog report it:
 * the apps in apps.json, each with every tool it offers (a read when the app marks it
 * read-only, otherwise a change), and Ghost's copy of the registry to connect more from. The
 * mock works out each tool's level from the permissions, as the daemon does, so the screens
 * show the same truth before and after a change.
 */
import type { AppView, CatalogEntryView, CatalogOption } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";

export interface AppTool {
  name: string;
  /** The app's own first sentence: untrusted text, shown and never acted on. */
  description: string;
  reads: boolean;
}

export interface AppFixture {
  /** The view without `actions`; the mock fills those from `tools` and the permissions. */
  view: Omit<AppView, "actions">;
  /** Every tool the app offers once it is connected. */
  tools: AppTool[];
}

const read = (name: string, description: string): AppTool => ({ name, description, reads: true });
const change = (name: string, description: string): AppTool => ({ name, description, reads: false });

/** The official GitHub app's issues and pull-request tools. */
export const GITHUB_TOOLS: AppTool[] = [
  read("get_me", "Get details of the authenticated GitHub user."),
  read("issue_read", "Get details of a specific issue in a GitHub repository."),
  read("list_issues", "List issues in a GitHub repository, filtered by state, labels and date."),
  read("search_issues", "Search for issues in GitHub repositories using issues search syntax."),
  read("pull_request_read", "Get details of a specific pull request, its diff, files and reviews."),
  read("list_pull_requests", "List pull requests in a GitHub repository."),
  change("issue_write", "Create a new issue or update an existing one: title, body, state, labels, assignees."),
  change("add_issue_comment", "Add a comment to a specific issue in a GitHub repository."),
  change("sub_issue_write", "Add, remove or reprioritize a sub-issue."),
  change("assign_copilot_to_issue", "Assign Copilot to an issue. It opens a pull request for it."),
  change("create_pull_request", "Create a new pull request in a GitHub repository."),
  change("merge_pull_request", "Merge a pull request in a GitHub repository."),
  change("delete_file", "Delete a file from a GitHub repository."),
];

const LINEAR_TOOLS: AppTool[] = [
  read("list_issues", "List issues in the workspace, filtered by team, state or assignee."),
  read("get_issue", "Get an issue by its identifier, with its comments."),
  read("list_projects", "List projects in the workspace."),
  change("create_issue", "Create a new issue in a team."),
  change("update_issue", "Update an issue's title, state, priority, assignee or labels."),
  change("create_comment", "Comment on an issue."),
];

const MOBBIN_TOOLS: AppTool[] = [
  read("search_screens", "Search Mobbin for UI screens using natural language."),
  read("search_flows", "Search Mobbin for multi-step user flows such as onboarding or checkout."),
  read("search_sections", "Search Mobbin for website sections such as pricing or a footer."),
];

const CALENDAR_TOOLS: AppTool[] = [
  read("today", "Today's events with times and attendees."),
  read("next_free_slot", "The next free half hour today."),
  change("create_event", "Create an event on the primary calendar."),
  change("delete_event", "Delete an event by its id."),
];

export const SENTRY_TOOLS: AppTool[] = [
  read("find_issues", "Find issues in a Sentry project by query."),
  read("get_issue_details", "Get an issue's stack trace, tags and recent events."),
  change("update_issue", "Resolve, ignore or assign a Sentry issue."),
];

/** Tools for an app the mock connects while you use it, by its catalog name. */
export const TOOLS_BY_CATALOG: Record<string, AppTool[]> = {
  "io.github.github/github-mcp-server": GITHUB_TOOLS,
  "app.linear/linear": LINEAR_TOOLS,
  "io.sentry/sentry": SENTRY_TOOLS,
  "com.notion/notion": [
    read("search", "Search pages and databases in the workspace."),
    read("fetch", "Read a page or a database by its URL."),
    change("create_pages", "Create pages in a database or under a page."),
    change("update_page", "Update a page's properties or content."),
  ],
  "com.figma/figma": [
    read("get_design_context", "Get the code and layout for a Figma frame or selection."),
    read("get_screenshot", "Get a screenshot of a Figma node."),
  ],
  "com.stripe/stripe": [
    read("list_customers", "List customers, newest first."),
    read("list_invoices", "List invoices for a customer."),
    change("create_refund", "Refund a payment, in full or in part."),
    change("create_invoice", "Create a draft invoice for a customer."),
    change("cancel_subscription", "Cancel a customer's subscription."),
  ],
};

/** A connected app's tools when the mock knows nothing about it: one read, one change. */
export const GENERIC_TOOLS: AppTool[] = [read("search", "Search what the app holds."), change("create", "Create something in the app.")];

function github(): AppFixture {
  return {
    view: { name: "github", title: "GitHub", kind: "sign_in", state: "connected", problem: null, catalog: "io.github.github/github-mcp-server", icon: ICONS.github, sign_in_url: null, settings: [] },
    tools: GITHUB_TOOLS,
  };
}

function linear(): AppFixture {
  return {
    view: {
      name: "linear",
      title: "Linear",
      kind: "sign_in",
      state: "needs_sign_in",
      problem: "Linear signed Ghost out. Sign in again to use it.",
      catalog: "app.linear/linear",
      icon: ICONS.linear,
      sign_in_url: null,
      settings: [],
    },
    tools: LINEAR_TOOLS,
  };
}

function mobbin(): AppFixture {
  return {
    view: { name: "mobbin", title: "Mobbin", kind: "key", state: "connected", problem: null, catalog: null, icon: null, sign_in_url: null, settings: ["GHOST_APP_MOBBIN_KEY"] },
    tools: MOBBIN_TOOLS,
  };
}

function notes(): AppFixture {
  return {
    view: {
      name: "notes",
      title: "Notes",
      kind: "local",
      state: "failed",
      problem: "notes-mcp: command not found (looked on PATH and in ~/.local/bin)",
      catalog: null,
      icon: null,
      sign_in_url: null,
      settings: [],
    },
    tools: [read("today", "Today's page."), change("add_note", "Add a note to today's page.")],
  };
}

/** Busy only: a second GitHub for a client's organization, with a key of its own. */
function githubWork(): AppFixture {
  return {
    view: { name: "github-acme", title: "GitHub (Acme)", kind: "key", state: "connected", problem: null, catalog: "io.github.github/github-mcp-server", icon: ICONS.github, sign_in_url: null, settings: ["GHOST_APP_GITHUB_ACME_KEY"] },
    tools: GITHUB_TOOLS,
  };
}

function calendar(): AppFixture {
  return {
    view: { name: "calendar", title: "Calendar", kind: "custom", state: "connected", problem: null, catalog: null, icon: null, sign_in_url: null, settings: [] },
    tools: CALENDAR_TOOLS,
  };
}

function sentryConnecting(): AppFixture {
  return {
    view: { name: "sentry", title: "Sentry", kind: "local", state: "connecting", problem: null, catalog: "io.sentry/sentry", icon: ICONS.sentry, sign_in_url: null, settings: ["GHOST_APP_SENTRY_SENTRY_ACCESS_TOKEN"] },
    tools: SENTRY_TOOLS,
  };
}

export default function apps(s: Scenario): AppFixture[] {
  // No apps.json yet (the empty and first-run scenarios): nothing connected.
  if (s.data === "empty") return [];
  const list = s.data === "busy" ? [github(), githubWork(), linear(), calendar(), mobbin(), sentryConnecting(), notes()] : [github(), linear(), mobbin(), notes()];
  return structuredClone(list);
}

/* ------------------------------------------------------------------- the catalog */

/** Favicons the registry lists. Only https, and shown with no referrer; a broken one falls back to the monogram. */
const ICONS = {
  github: "https://github.githubassets.com/favicons/favicon.svg",
  linear: "https://linear.app/favicon.ico",
  notion: "https://www.notion.so/images/favicon.ico",
  sentry: "https://sentry.io/favicon.ico",
  figma: "https://static.figma.com/app/icon/1/favicon.svg",
  stripe: "https://stripe.com/favicon.ico",
};

/** A remote that signs in, or takes a key, at a host; the host is the maker's own unless said otherwise. */
const signIn = (host: string, host_owner: CatalogOption["host_owner"] = "maker"): CatalogOption => ({ id: "remote", kind: "sign_in", label: "Sign in", settings: [], host, host_owner });
const apiKey = (host: string, label = "API key", host_owner: CatalogOption["host_owner"] = "maker"): CatalogOption => ({
  id: "remote-key",
  kind: "key",
  label: "Use a password",
  settings: [{ key: "key", label, secret: true, required: true, default: null, choices: [] }],
  host,
  host_owner,
});
/** A package Ghost runs on this computer. */
const pkg = (registry: "npm" | "pypi" | "oci", id: string, settings: CatalogOption["settings"] = [], n = 0): CatalogOption => ({
  id: `package-${n}`,
  kind: "local",
  label: `Run on this computer (${{ npm: "npm", pypi: "Python", oci: "a container" }[registry]})`,
  settings,
  registry,
  package: id,
});
const secret = (key: string, label: string): CatalogOption["settings"][number] => ({ key, label, secret: true, required: true, default: null, choices: [] });

type Publisher = NonNullable<CatalogEntryView["publisher"]>;
type Maker = NonNullable<CatalogEntryView["maker"]>;
const onGitHub = (name: string): Publisher => ({ name, verified: "github" });
const domain = (name: string): Publisher => ({ name, verified: "domain" });
const own = (product: string, ...evidence: Maker["evidence"]): Pick<CatalogEntryView, "maker" | "product"> => ({ maker: { product, evidence }, product });

export interface CatalogSeed extends Omit<CatalogEntryView, "connected" | "suggested_name" | "version" | "website" | "repository" | "others"> {
  /** The short name Ghost suggests (the registry entry's app_name). */
  app: string;
  version?: string;
  website?: string | null;
  repository?: string | null;
}

/**
 * Ghost's copy of the registry, as the daemon reads it (#37): who published each, whether it is
 * the product maker's own, where it runs. Search "posthog" for the six results the operator saw
 * on 2026-10-28: PostHog's own first, four more folded under it, and one other product.
 */
export const CATALOG: CatalogSeed[] = [
  {
    name: "io.github.github/github-mcp-server",
    app: "github",
    title: "GitHub",
    description: "Issues, pull requests, code search and Actions in your repositories.",
    icon: ICONS.github,
    featured: true,
    version: "0.19.1",
    updated_at: "2026-10-21T09:53:50Z",
    website: "https://github.com",
    repository: "https://github.com/github/github-mcp-server",
    publisher: onGitHub("github"),
    ...own("GitHub", "listed", "remote"),
    options: [
      signIn("api.githubcopilot.com"),
      apiKey("api.githubcopilot.com", "Personal access token"),
      pkg("oci", "ghcr.io/github/github-mcp-server:0.19.1", [
        secret("GITHUB_PERSONAL_ACCESS_TOKEN", "Personal access token"),
        { key: "GITHUB_TOOLSETS", label: "Toolsets", secret: false, required: false, default: "issues,pull_requests", choices: [] },
      ]),
    ],
  },
  {
    name: "app.linear/linear",
    app: "linear",
    title: "Linear",
    description: "Find, create and update issues, projects and comments in Linear.",
    icon: ICONS.linear,
    featured: true,
    version: "1.2.0",
    updated_at: "2026-10-22T16:47:17Z",
    website: "https://linear.app",
    publisher: domain("linear.app"),
    ...own("Linear", "publisher", "remote", "website"),
    options: [signIn("mcp.linear.app")],
  },
  {
    name: "com.notion/notion",
    app: "notion",
    title: "Notion",
    description: "Search, read and write pages and databases in your Notion workspace.",
    icon: ICONS.notion,
    featured: true,
    updated_at: "2025-09-11T22:25:50Z",
    website: "https://www.notion.so",
    publisher: domain("notion.com"),
    ...own("Notion", "publisher", "remote"),
    options: [signIn("mcp.notion.com"), pkg("npm", "@notionhq/notion-mcp-server", [secret("NOTION_TOKEN", "Integration token")], 1)],
  },
  {
    name: "io.sentry/sentry",
    app: "sentry",
    title: "Sentry",
    description: "Look into errors and performance issues, and resolve or assign them.",
    icon: ICONS.sentry,
    featured: true,
    updated_at: "2026-09-25T14:49:17Z",
    website: "https://sentry.io",
    repository: "https://github.com/getsentry/sentry-mcp",
    publisher: domain("sentry.io"),
    ...own("Sentry", "publisher", "remote", "website"),
    options: [
      signIn("mcp.sentry.dev"),
      pkg(
        "npm",
        "@sentry/mcp-server",
        [secret("SENTRY_ACCESS_TOKEN", "Access token"), { key: "SENTRY_HOST", label: "Sentry host", secret: false, required: false, default: "sentry.io", choices: [] }],
        1,
      ),
    ],
  },
  {
    name: "com.figma/figma",
    app: "figma",
    title: "Figma",
    description: "Bring frames, components and variables from Figma into the conversation.",
    icon: ICONS.figma,
    featured: true,
    updated_at: "2025-10-16T17:40:56Z",
    website: "https://www.figma.com",
    publisher: domain("figma.com"),
    ...own("Figma", "publisher", "remote", "website"),
    options: [signIn("mcp.figma.com")],
  },
  {
    name: "com.stripe/stripe",
    app: "stripe",
    title: "Stripe",
    description: "Customers, invoices, subscriptions and refunds in your Stripe account.",
    icon: ICONS.stripe,
    featured: true,
    updated_at: "2025-10-28T22:06:18Z",
    website: "https://stripe.com",
    repository: "https://github.com/stripe/agent-toolkit",
    publisher: domain("stripe.com"),
    ...own("Stripe", "publisher", "remote", "repository", "website"),
    options: [signIn("mcp.stripe.com"), apiKey("mcp.stripe.com", "Restricted key")],
  },
  {
    name: "com.slack/slack",
    app: "slack",
    title: "Slack",
    description: "Read channels and threads, and post messages as you.",
    icon: null,
    featured: false,
    publisher: domain("slack.com"),
    ...own("Slack", "publisher", "remote"),
    options: [signIn("mcp.slack.com")],
  },
  {
    name: "io.github.modelcontextprotocol/filesystem",
    app: "files",
    title: "Files",
    description: "Read and write files in folders you choose on this computer.",
    icon: null,
    featured: false,
    repository: "https://github.com/modelcontextprotocol/servers",
    publisher: onGitHub("modelcontextprotocol"),
    options: [pkg("npm", "@modelcontextprotocol/server-filesystem", [{ key: "ROOT", label: "Folder", secret: false, required: true, default: "~/Documents", choices: [] }])],
  },
  {
    name: "com.brave/search",
    app: "brave",
    title: "Brave Search",
    description: "Search the web, news and images with Brave’s index.",
    icon: null,
    featured: false,
    publisher: domain("brave.com"),
    options: [pkg("npm", "@brave/brave-search-mcp-server", [secret("BRAVE_API_KEY", "API key")])],
  },
  {
    name: "com.cloudflare/workers",
    app: "cloudflare",
    title: "Cloudflare",
    description: "Workers, KV, R2 and DNS on your Cloudflare account.",
    icon: null,
    featured: false,
    publisher: domain("cloudflare.com"),
    ...own("Cloudflare", "publisher", "remote"),
    options: [signIn("bindings.mcp.cloudflare.com")],
  },
  {
    name: "io.github.crystaldba/postgres",
    app: "postgres",
    title: "Postgres",
    description: "Query a PostgreSQL database and explain slow queries.",
    icon: null,
    featured: false,
    repository: "https://github.com/crystaldba/postgres-mcp",
    publisher: onGitHub("crystaldba"),
    options: [
      pkg("pypi", "postgres-mcp", [
        secret("DATABASE_URI", "Database address"),
        { key: "ACCESS_MODE", label: "Access", secret: false, required: false, default: "restricted", choices: ["restricted", "unrestricted"] },
      ]),
    ],
  },
  /* The six "posthog" results, as Ghost's copy of the registry had them on 2026-10-28 (#37). */
  {
    name: "io.github.PostHog/mcp",
    app: "posthog",
    title: "PostHog",
    description: "Official PostHog app for product analytics, feature flags, experiments, and more.",
    icon: null,
    featured: false,
    version: "1.0.1",
    updated_at: "2026-01-07T16:34:40Z",
    repository: "https://github.com/PostHog/posthog",
    publisher: onGitHub("PostHog"),
    ...own("PostHog", "publisher", "remote"),
    options: [apiKey("mcp.posthog.com", "Your PostHog personal API key")],
  },
  {
    name: "com.mcparmory/posthog",
    app: "posthog",
    title: "Posthog",
    description: "Capture analytics events, evaluate feature flags, and manage projects",
    icon: null,
    featured: false,
    version: "1.0.5",
    updated_at: "2026-05-11T21:42:20Z",
    repository: "https://github.com/mcparmory/registry",
    publisher: domain("mcparmory.com"),
    product: "PostHog",
    product_maker: "io.github.PostHog/mcp",
    options: [pkg("pypi", "mcparmory-posthog"), pkg("oci", "ghcr.io/mcparmory/posthog:1.0.5", [], 1)],
  },
  {
    name: "io.github.friendlygeorge/posthog-mcp-server",
    app: "posthog",
    title: "Posthog",
    description: "App for PostHog: product analytics for AI assistants. Query events, persons, insights, da...",
    icon: null,
    featured: false,
    version: "1.0.0",
    updated_at: "2026-06-06T14:47:03Z",
    repository: "https://github.com/friendlygeorge/posthog-mcp-server",
    publisher: onGitHub("friendlygeorge"),
    product: "PostHog",
    product_maker: "io.github.PostHog/mcp",
    options: [pkg("npm", "posthog-mcp-server")],
  },
  {
    name: "io.github.mrfentmen/posthog-mcp",
    app: "posthog",
    title: "Posthog",
    description: "Query PostHog: profile, projects, insights.",
    icon: null,
    featured: false,
    version: "1.1.0",
    updated_at: "2026-10-17T06:49:51Z",
    repository: "https://github.com/mrfentmen/premium-mcps",
    publisher: onGitHub("mrfentmen"),
    product: "PostHog",
    product_maker: "io.github.PostHog/mcp",
    options: [pkg("npm", "posthog-mcp")],
  },
  {
    name: "io.github.pipeworx-io/posthog",
    app: "posthog",
    title: "Posthog",
    description: "PostHog app: wraps PostHog Analytics API (app.posthog.com/api)",
    icon: null,
    featured: false,
    version: "0.1.2",
    updated_at: "2026-09-26T21:04:50Z",
    website: "https://pipeworx.io/packs/posthog",
    repository: "https://github.com/pipeworx-io/mcp-posthog",
    publisher: onGitHub("pipeworx-io"),
    product: "PostHog",
    product_maker: "io.github.PostHog/mcp",
    options: [signIn("gateway.pipeworx.io", "publisher")],
  },
  {
    name: "tech.engageable/analytics",
    app: "engageable-analytics",
    title: "Engageable Analytics",
    description: "Unified analytics app for GA4, Mixpanel, and PostHog.",
    icon: null,
    featured: false,
    version: "0.2.0",
    updated_at: "2026-03-26T23:02:26Z",
    repository: "https://github.com/engageable-tech/engageable-oss",
    publisher: domain("engageable.tech"),
    options: [pkg("pypi", "engageable", [secret("POSTHOG_API_KEY", "PostHog personal API key"), { key: "POSTHOG_PROJECT_ID", label: "PostHog project ID", secret: false, required: false, default: null, choices: [] }])],
  },
  /* GitHub's wrappers: searching "github" folds them under GitHub's own. */
  {
    name: "io.github.pipeworx-io/github",
    app: "github",
    title: "Github",
    description: "GitHub app: wraps the GitHub REST API.",
    icon: null,
    featured: false,
    updated_at: "2026-10-26T04:00:10Z",
    repository: "https://github.com/pipeworx-io/mcp-github",
    publisher: onGitHub("pipeworx-io"),
    product: "GitHub",
    product_maker: "io.github.github/github-mcp-server",
    options: [signIn("gateway.pipeworx.io", "publisher")],
  },
  {
    name: "com.mcparmory/github",
    app: "github",
    title: "Github",
    description: "Repositories, issues and pull requests.",
    icon: null,
    featured: false,
    updated_at: "2026-05-11T21:37:47Z",
    repository: "https://github.com/mcparmory/registry",
    publisher: domain("mcparmory.com"),
    product: "GitHub",
    product_maker: "io.github.github/github-mcp-server",
    options: [pkg("pypi", "mcparmory-github")],
  },
];
