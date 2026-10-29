import { Check, Copy, Power } from "lucide-react";
import { useState } from "react";

import { inTauri, startDaemon } from "../transport";
import { Button, EmptyState, IconButton, Notice, Spinner, errorText } from "../ui";
import { daemonLine, useDaemon } from "./daemon";

const COMMAND = "systemctl --user enable --now ghostd";

/** Shown in place of any screen that needs the daemon while it is not running. */
export function Offline() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const daemon = useDaemon();

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      await startDaemon();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(COMMAND);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard unavailable: the command is selectable */
    }
  };

  // The app runs the daemon itself (ADR 0038): a Start button only while it is between
  // tries, where it skips the rest of the delay. In a browser, the headless mode's command.
  const canStart = inTauri && (daemon?.state === "restarting" || daemon?.state === "failed");
  // One EmptyState holds everything, so this page sits at the same optical center as every
  // other page-level state (not found, a crashed screen).
  return (
    <EmptyState
      size="page"
      className="shell-offline"
      icon={<Power />}
      title="Ghost isn’t running"
      description={inTauri ? daemonLine(daemon) : "The Ghost service isn’t answering on this computer. Start it, and this window connects on its own."}
      actions={
        <div className="shell-offline__actions">
          {canStart && (
            <Button variant="primary" size="lg" loading={busy} onClick={start}>
              Start Ghost now
            </Button>
          )}
          {!inTauri && (
            <div className="shell-offline__command">
              <code>{COMMAND}</code>
              <IconButton size="sm" label={copied ? "Copied" : "Copy command"} icon={copied ? <Check /> : <Copy />} onClick={copy} />
            </div>
          )}
          {error && (
            <Notice tone="danger" title="Ghost didn’t start" className="shell-offline__error">
              {error}
            </Notice>
          )}
          <p className="shell-offline__retry">
            <Spinner size={12} /> Checking again every few seconds
          </p>
        </div>
      }
    />
  );
}
