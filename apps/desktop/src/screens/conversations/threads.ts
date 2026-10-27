/**
 * Owner: conversations. The agent fields the generated client carries (ADR 0024) and the
 * helpers every screen reads them through: which department an agent works in, whether it
 * is the department's own, retired or not, and how a list groups by department.
 */
import type { ConversationCreate, ConversationDetail, ConversationUpdate, ConversationView, GhostClient } from "@ghost/client-ts";

export type AgentView = ConversationView;
export type AgentDetail = ConversationDetail;
export type AgentCreate = ConversationCreate;
export type AgentUpdate = ConversationUpdate;

/** When the agent was retired (left the roster and voice routing), or null. */
export function retiredAt(d: Pick<AgentView, "retired_at"> | null | undefined): string | null {
  const v = d?.retired_at;
  return typeof v === "string" && v ? v : null;
}

export const isRetired = (d: Pick<AgentView, "retired_at"> | null | undefined) => retiredAt(d) !== null;

/** The department an agent works in: its slug ("ghost" for Ghost's own). */
export const departmentOf = (a: Pick<AgentView, "department"> | null | undefined): string => a?.department || "ghost";

/** The department's own agent, named for it: where a transfer lands; it cannot move, retire or be deleted. */
export const isDefault = (a: Pick<AgentView, "is_default"> | null | undefined): boolean => !!a?.is_default;

/** A department's title as the operator says it, from the departments list when loaded, else from the slug. */
export function departmentTitle(slug: string | null | undefined, departments: Pick<DepartmentLike, "slug" | "title">[] = []): string {
  if (!slug || slug === "ghost") return "Ghost";
  return departments.find((d) => d.slug === slug)?.title ?? slug.replace(/-/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

/** What this screen needs of a department: the generated DepartmentView carries these. */
export interface DepartmentLike {
  slug: string;
  title: string;
}

export interface DepartmentGroup<T extends AgentView> {
  /** The department's slug. */
  slug: string;
  /** Its own agent, when it is in the list. */
  head: T | null;
  /** The agents hired into it, in the order given. */
  agents: T[];
}

/**
 * Agents grouped the way the sidebar and the phone show them: Ghost first, then each
 * department with its own agent at the head and the agents hired into it under it. Order
 * follows the input: a group sits where its first member would have.
 */
export function groupByDepartment<T extends AgentView>(agents: T[]): DepartmentGroup<T>[] {
  const groups = new Map<string, DepartmentGroup<T>>();
  for (const a of agents) {
    const slug = departmentOf(a);
    let g = groups.get(slug);
    if (!g) {
      g = { slug, head: null, agents: [] };
      groups.set(slug, g);
    }
    if (isDefault(a)) g.head = a;
    else g.agents.push(a);
  }
  const out = [...groups.values()];
  return out.sort((a, b) => (a.slug === "ghost" ? -1 : b.slug === "ghost" ? 1 : firstIndex(a, agents) - firstIndex(b, agents)));
}
const firstIndex = <T extends AgentView>(g: DepartmentGroup<T>, agents: T[]) => Math.min(...[g.head, ...g.agents].filter((x): x is T => !!x).map((a) => agents.indexOf(a)));

/** "GitHub" for github: the source's name in words, used as a card heading. */
export function sourceWords(name: string): string {
  if (name === "conversations") return "Agents";
  if (name === "departments") return "Departments";
  const s = name.replace(/[_-]+/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1).replace(/\bgithub\b/i, "GitHub");
}

/** Retire an agent: it leaves voice routing and the roster; everything stays readable. */
export const retireAgent = (client: GhostClient, slug: string) => client.retireConversation(slug);
/** Bring a retired agent back. */
export const rehireAgent = (client: GhostClient, slug: string) => client.rehireConversation(slug);
/** Move an agent's folder into another department. */
export const moveAgent = (client: GhostClient, slug: string, department: string) => client.moveConversation(slug, department);
