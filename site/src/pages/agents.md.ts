/**
 * /agents.md: the guide a person's coding agent reads to set Ghost up (docs/agent-setup.md),
 * served whole as Markdown. The repository's file is the one source: it is read at build, so the
 * site and the repository can't say different things (`site:verify` checks the built copy).
 */
import guide from "../../../docs/agent-setup.md?raw";

export function GET() {
  return new Response(guide, { headers: { "content-type": "text/markdown; charset=utf-8" } });
}
