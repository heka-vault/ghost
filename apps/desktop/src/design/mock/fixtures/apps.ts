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

const signIn: CatalogOption = { id: "remote", kind: "sign_in", label: "Sign in", settings: [] };
const apiKey = (label = "API key"): CatalogOption => ({ id: "remote-key", kind: "key", label: "Use a password", settings: [{ key: "key", label, secret: true, required: true, default: null, choices: [] }] });

export interface CatalogSeed extends Omit<CatalogEntryView, "connected" | "suggested_name" | "version" | "website" | "repository"> {
  /** The short name Ghost suggests (the registry entry's app_name). */
  app: string;
  version?: string;
  website?: string | null;
  repository?: string | null;
}

export const CATALOG: CatalogSeed[] = [
  {
    name: "io.github.github/github-mcp-server",
    app: "github",
    title: "GitHub",
    description: "Issues, pull requests, code search and Actions in your repositories.",
    icon: ICONS.github,
    featured: true,
    version: "0.19.1",
    website: "https://github.com",
    repository: "https://github.com/github/github-mcp-server",
    options: [
      signIn,
      apiKey("Personal access token"),
      {
        id: "oci",
        kind: "local",
        label: "Run on this computer (Docker)",
        settings: [
          { key: "GITHUB_PERSONAL_ACCESS_TOKEN", label: "Personal access token", secret: true, required: true, default: null, choices: [] },
          { key: "GITHUB_TOOLSETS", label: "Toolsets", secret: false, required: false, default: "issues,pull_requests", choices: [] },
        ],
      },
    ],
  },
  { name: "app.linear/linear", app: "linear", title: "Linear", description: "Find, create and update issues, projects and comments in Linear.", icon: ICONS.linear, featured: true, website: "https://linear.app", options: [signIn] },
  {
    name: "com.notion/notion",
    app: "notion",
    title: "Notion",
    description: "Search, read and write pages and databases in your Notion workspace.",
    icon: ICONS.notion,
    featured: true,
    website: "https://www.notion.so",
    options: [
      signIn,
      { id: "npm", kind: "local", label: "Run on this computer (npm)", settings: [{ key: "NOTION_TOKEN", label: "Integration token", secret: true, required: true, default: null, choices: [] }] },
    ],
  },
  {
    name: "io.sentry/sentry",
    app: "sentry",
    title: "Sentry",
    description: "Look into errors and performance issues, and resolve or assign them.",
    icon: ICONS.sentry,
    featured: true,
    website: "https://sentry.io",
    options: [
      signIn,
      {
        id: "npm",
        kind: "local",
        label: "Run on this computer (npm)",
        settings: [
          { key: "SENTRY_ACCESS_TOKEN", label: "Access token", secret: true, required: true, default: null, choices: [] },
          { key: "SENTRY_HOST", label: "Sentry host", secret: false, required: false, default: "sentry.io", choices: [] },
        ],
      },
    ],
  },
  { name: "com.figma/figma", app: "figma", title: "Figma", description: "Bring frames, components and variables from Figma into the conversation.", icon: ICONS.figma, featured: true, website: "https://www.figma.com", options: [signIn] },
  {
    name: "com.stripe/stripe",
    app: "stripe",
    title: "Stripe",
    description: "Customers, invoices, subscriptions and refunds in your Stripe account.",
    icon: ICONS.stripe,
    featured: true,
    website: "https://stripe.com",
    options: [signIn, apiKey("Restricted key")],
  },
  {
    name: "com.slack/slack",
    app: "slack",
    title: "Slack",
    description: "Read channels and threads, and post messages as you.",
    icon: null,
    featured: false,
    options: [signIn],
  },
  {
    name: "io.github.modelcontextprotocol/filesystem",
    app: "files",
    title: "Files",
    description: "Read and write files in folders you choose on this computer.",
    icon: null,
    featured: false,
    options: [{ id: "npm", kind: "local", label: "Run on this computer (npm)", settings: [{ key: "ROOT", label: "Folder", secret: false, required: true, default: "~/Documents", choices: [] }] }],
  },
  {
    name: "com.brave/search",
    app: "brave",
    title: "Brave Search",
    description: "Search the web, news and images with Brave’s index.",
    icon: null,
    featured: false,
    options: [{ id: "npm", kind: "local", label: "Run on this computer (npm)", settings: [{ key: "BRAVE_API_KEY", label: "API key", secret: true, required: true, default: null, choices: [] }] }],
  },
  {
    name: "com.cloudflare/workers",
    app: "cloudflare",
    title: "Cloudflare",
    description: "Workers, KV, R2 and DNS on your Cloudflare account.",
    icon: null,
    featured: false,
    options: [signIn],
  },
  {
    name: "io.github.crystaldba/postgres",
    app: "postgres",
    title: "Postgres",
    description: "Query a PostgreSQL database and explain slow queries.",
    icon: null,
    featured: false,
    options: [
      {
        id: "pypi",
        kind: "local",
        label: "Run on this computer (PyPI)",
        settings: [
          { key: "DATABASE_URI", label: "Database address", secret: true, required: true, default: null, choices: [] },
          { key: "ACCESS_MODE", label: "Access", secret: false, required: false, default: "restricted", choices: ["restricted", "unrestricted"] },
        ],
      },
    ],
  },
];
