/**
 * Owner: settings. What Ghost and this window are (docs/releasing.md): the daemon's version
 * and build, the desktop app's own version, and a quiet notice when the two differ. General's
 * About card holds the rows; the notice sits above it.
 */
import type { Status } from "@ghost/client-ts";

import { DESKTOP_VERSION, otherVersion } from "../../app/version";
import { ListRow, Notice, buildText } from "../../ui";
import "./settings.css";

/** What a build is, in the operator's words; the commands are what made it. */
function buildDescription(build: Status["build"] | undefined): string {
  if (!build) return "This Ghost doesn’t say how it was built. It is older than this window.";
  if (build.kind === "release") return "Installed from the heka-ghost package.";
  if (build.kind === "deploy") return "A fixed build of one commit, made by make deploy.";
  return build.dirty
    ? "Running from a checkout (make dev). It had uncommitted changes when it started."
    : "Running from a checkout (make dev).";
}

/** The About card's version rows: Ghost's version, its build and this window's version. */
export function VersionRows({ status }: { status: Status | null }) {
  const build = status ? buildText(status.build) : null;
  return (
    <>
      <ListRow title="Version" description="The Ghost running on this computer." meta={<span className="set-value is-mono">{status?.version ?? "–"}</span>} />
      {status && (
        <ListRow title="Build" description={buildDescription(status.build)} meta={<span className="set-value is-mono">{build ?? "–"}</span>} />
      )}
      <ListRow title="Desktop app" description="This window." meta={<span className="set-value is-mono">{DESKTOP_VERSION}</span>} />
    </>
  );
}

/** Said once, above the card, when this window and the daemon are different versions. */
export function VersionNotice({ status }: { status: Status | null }) {
  const other = otherVersion(status?.version);
  if (!other) return null;
  return (
    <Notice title="This window and Ghost are different versions">
      The desktop app is {DESKTOP_VERSION} and Ghost is {other}. That’s expected while you run the app from a checkout against your deployed Ghost; otherwise, update the one that’s behind.
    </Notice>
  );
}
