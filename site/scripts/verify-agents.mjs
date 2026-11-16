#!/usr/bin/env node
/**
 * After a build: the site says what the repository says. /agents.md is docs/agent-setup.md byte
 * for byte, /llms.txt points at it, and the home page carries the prompt (src/agent-prompt.txt)
 * and its Copy button, and no link names a release's file. Run by `pnpm --dir site site:verify` and by the site workflow after the
 * build; tools/tests/test_agent_setup.py holds the README's copy of the prompt to the same file.
 */
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const at = (p) => fileURLToPath(new URL(p, import.meta.url));
const dist = at("../dist/");
const problems = [];
const read = (p) => (existsSync(p) ? readFileSync(p, "utf8") : (problems.push(`${p} is missing (build the site first)`), ""));

const guide = readFileSync(at("../../docs/agent-setup.md"), "utf8");
if (read(`${dist}agents.md`) !== guide) problems.push("dist/agents.md is not docs/agent-setup.md");
if (!read(`${dist}llms.txt`).includes("https://heyghost.app/agents.md")) problems.push("dist/llms.txt does not point at /agents.md");
const prompt = readFileSync(at("../src/agent-prompt.txt"), "utf8").trim();
const home = read(`${dist}index.html`);
if (!home.includes(prompt)) problems.push("the home page does not carry src/agent-prompt.txt word for word");
if (!home.includes("data-copy")) problems.push("the home page has no Copy button");
if (!prompt.includes("https://heyghost.app/agents.md")) problems.push("the prompt does not name https://heyghost.app/agents.md");
// Downloads go to the latest release's page, never to a file: a file's address carries a version
// and breaks at every release (docs/site.md).
if (!home.includes("https://github.com/heka-vault/ghost/releases/latest\"")) problems.push("the home page has no link to the latest release");
for (const page of [home, read(`${dist}llms.txt`)]) {
  const file = page.match(/releases\/(?:latest\/)?download\/[^"\s)]*/);
  if (file) problems.push(`a link names a release's file (${file[0]}): link the latest release's page instead`);
}

if (problems.length) {
  for (const p of problems) console.error(`site:verify: ${p}`);
  process.exit(1);
}
console.log("site:verify: /agents.md, /llms.txt and the prompt match the repository");
