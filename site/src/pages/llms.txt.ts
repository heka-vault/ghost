/**
 * /llms.txt: what Ghost is in a few lines, for a model that lands on the site, pointing at the
 * setup guide an agent follows (/agents.md, docs/agent-setup.md) and the rest of the docs.
 */
import { version } from "../../../package.json";

const REPO = "https://github.com/heka-vault/ghost";

const text = `# Ghost

> Ghost is a voice-first assistant that runs on a person's own Linux desktop (x86-64; Ubuntu 24.04 and 26.04 as a .deb, other distributions as an AppImage). It listens for its name locally, sends the conversation after the wake word to the model provider the person chooses (or none, with a local model), uses the apps they connect, and answers out loud. Open source, MIT. Current version: ${version}.

## Set it up

- [Setup guide for coding agents](https://heyghost.app/agents.md): read it in full and follow it step by step; it says what to tell the person first, what to check, and what never to do.

## Docs

- [README](${REPO}#readme): install, connecting a model, sound, privacy
- [Connecting other tools to Ghost](${REPO}/blob/main/docs/connect-ghost.md): outside tools, and what they can do
- [Releases](${REPO}/releases/latest)
`;

export function GET() {
  return new Response(text, { headers: { "content-type": "text/plain; charset=utf-8" } });
}
