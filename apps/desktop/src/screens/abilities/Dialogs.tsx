/** Owner: abilities. The two dialogs: letting an ability hear idle audio, and how to add one. */
import { ArrowUpRight, Cpu, Ear, FolderInput, Package, Undo2 } from "lucide-react";
import { type ReactNode, type RefObject, useEffect, useRef } from "react";

import { Button, Dialog } from "../../ui";
import { CommandLine } from "./CommandLine";
import { RESTART_COMMAND, andList } from "./model";

/**
 * First focus on a footer button, not the header's Close. Dialog has no initialFocus prop yet
 * (see shared_requests); this runs after its showModal(), since a child's effects run first.
 */
function useInitialFocus(open: boolean, ref: RefObject<HTMLButtonElement | HTMLAnchorElement | null>) {
  useEffect(() => {
    if (open) ref.current?.focus();
  }, [open, ref]);
}

const DOCS_URL = "https://github.com/heka-vault/ghost/blob/main/docs/abilities.md";

function Point({ icon, title, children }: { icon: ReactNode; title: ReactNode; children: ReactNode }) {
  return (
    <li className="ab-point">
      <span className="ab-point__icon" aria-hidden>
        {icon}
      </span>
      <div className="ab-point__text">
        <p className="ab-point__title">{title}</p>
        <div className="ab-point__body">{children}</div>
      </div>
    </li>
  );
}

/**
 * Consent for a passive ability (ADR 0012): it runs before the wake phrase, so it hears idle
 * audio. Ghost refuses to start with one enabled unless it is also listed under
 * [abilities] passive, so turning it on writes both lists at once, after this explicit okay.
 */
export function PassiveDialog({
  open,
  name,
  pkg,
  hearing,
  busy,
  onAllow,
  onClose,
}: {
  open: boolean;
  name: string;
  pkg: string;
  /** Other abilities that hear idle audio right now (loaded and passive), by display name. */
  hearing: string[];
  busy: boolean;
  onAllow: () => void;
  onClose: () => void;
}) {
  const cancel = useRef<HTMLButtonElement>(null);
  useInitialFocus(open, cancel);
  const today = hearing.length === 0 ? "Until now, idle audio has gone only to Ghost’s own wake detection." : `Today idle audio goes to Ghost’s wake detection and ${andList(hearing)}.`;
  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title={`Let ${name} hear idle audio?`}
      description={`${name} would start listening before you say the wake word. ${today}`}
      footer={
        <>
          <Button ref={cancel} variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={onAllow}>
            Allow and turn on
          </Button>
        </>
      }
    >
      <ul className="ab-points">
        <Point icon={<Ear />} title="It hears everything, all the time">
          Not only what you say to Ghost: anything the microphone picks up while Ghost is idle.
        </Point>
        <Point icon={<Cpu />} title="It runs inside Ghost, on this computer">
          With the same access Ghost has. Ghost can’t stop an ability from sending what it hears, so allow only ones you trust.
        </Point>
        <Point icon={<Undo2 />} title="You can take it back">
          Turn it off here and it stops hearing anything once Ghost restarts.
        </Point>
      </ul>
      <p className="ab-dialog-detail">
        Adds <code>{pkg}</code> to <code>[abilities] passive</code> and <code>enabled</code> in <code>~/.config/ghost/policy.toml</code>.
      </p>
    </Dialog>
  );
}

export function AddDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const done = useRef<HTMLButtonElement>(null);
  useInitialFocus(open, done);
  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title="Add an ability"
      description="An ability is a Python package with one exported object. Ghost finds it in one of two places."
      footer={
        <>
          <Button variant="ghost" href={DOCS_URL} target="_blank" rel="noreferrer" trailingIcon={<ArrowUpRight />} className="ab-dialog-docs">
            How to write one
          </Button>
          <Button ref={done} variant="secondary" onClick={onClose}>
            Done
          </Button>
        </>
      }
    >
      <ol className="ab-steps">
        <li className="ab-step">
          <span className="ab-step__num t-num" aria-hidden>1</span>
          <div className="ab-step__body">
            <p className="ab-step__title">Install it</p>
            <div className="ab-step__ways">
              <div className="ab-way">
                <p className="ab-way__label">
                  <Package aria-hidden /> As a package, in Ghost’s environment
                </p>
                <CommandLine command="uv add <package>" template />
              </div>
              <div className="ab-way">
                <p className="ab-way__label">
                  <FolderInput aria-hidden /> Or as a folder in
                </p>
                <CommandLine command="~/.config/ghost/abilities/" label="Copy path" />
                <p className="ab-way__help">One folder per ability. A folder with the same name as an installed package takes its place.</p>
              </div>
            </div>
          </div>
        </li>
        <li className="ab-step">
          <span className="ab-step__num t-num" aria-hidden>2</span>
          <div className="ab-step__body">
            <p className="ab-step__title">Restart Ghost</p>
            <p className="ab-step__help">It shows up on this page, off.</p>
            <CommandLine command={RESTART_COMMAND} />
          </div>
        </li>
        <li className="ab-step">
          <span className="ab-step__num t-num" aria-hidden>3</span>
          <div className="ab-step__body">
            <p className="ab-step__title">Turn it on, then restart once more</p>
            <p className="ab-step__help">Ghost loads abilities when it starts. One that hears idle audio asks for your okay first.</p>
          </div>
        </li>
      </ol>
    </Dialog>
  );
}
