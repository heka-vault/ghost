/** Owner: abilities. One shell command, selectable, with a copy button when it can run as written. */
import { Check, Copy } from "lucide-react";
import { Fragment, useEffect, useRef, useState } from "react";

import { IconButton, cx } from "../../ui";

/**
 * `template` marks a command with placeholders in angle brackets (`uv add <package>`): the
 * placeholders read quieter and there is no copy button, so nothing made up can be pasted and run.
 */
export function CommandLine({ command, label = "Copy command", template, className }: { command: string; label?: string; template?: boolean; className?: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard unavailable: the command is selectable */
    }
  };

  return (
    <div className={cx("ab-cmd", template && "ab-cmd--template", className)}>
      <code className="ab-cmd__text">
        {template
          ? command.split(/(<[^>]+>)/).map((part, i) =>
              part.startsWith("<") ? (
                <span key={i} className="ab-cmd__slot">
                  {part}
                </span>
              ) : (
                <Fragment key={i}>{part}</Fragment>
              ),
            )
          : command}
      </code>
      {!template && <IconButton size="sm" variant="ghost" label={copied ? "Copied" : label} icon={copied ? <Check /> : <Copy />} onClick={copy} />}
    </div>
  );
}
