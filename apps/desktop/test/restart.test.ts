import { describe, expect, it } from "vitest";

import { DEV_RESTART_COMMAND, RESTART_COMMAND, START_RESTART_COMMAND, restartCommand } from "../src/screens/settings/restart";

describe("restartCommand(): the restart the running Ghost needs", () => {
  it("is the service for a deploy, a release, or a daemon that doesn't say", () => {
    expect(restartCommand({ kind: "deploy", commit: null, dirty: false })).toBe(RESTART_COMMAND);
    expect(restartCommand({ kind: "release", commit: null, dirty: false })).toBe(RESTART_COMMAND);
    expect(restartCommand(null)).toBe(RESTART_COMMAND);
  });

  it("is the make target that started a checkout's daemon", () => {
    expect(restartCommand({ kind: "dev", commit: null, dirty: false, launcher: "make-dev" })).toBe(DEV_RESTART_COMMAND);
    expect(restartCommand({ kind: "dev", commit: null, dirty: false, launcher: "make-start" })).toBe(START_RESTART_COMMAND);
  });

  it("is no command for a checkout started some other way", () => {
    expect(restartCommand({ kind: "dev", commit: null, dirty: false })).toBeNull();
    expect(restartCommand({ kind: "dev", commit: null, dirty: false, launcher: null })).toBeNull();
  });
});
