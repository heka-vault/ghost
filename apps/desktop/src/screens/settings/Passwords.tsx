/**
 * Owner: settings. Settings › Passwords (#/settings/passwords): Ghost's vault (ADR 0036).
 *
 * Every password Ghost knows of, by name: what uses it, when Ghost last filled it in, and
 * Set or Not set. Add, replace and delete each save at once on their own row; a value goes
 * in and never comes back (the page never holds one after its Save). Passwords still in
 * plain files are offered for a one-time import at the top, and a locked keyring says so
 * with an Unlock.
 *
 * States by URL (mock): `?vault=locked|import|broken` on any scenario; `busy` has files to
 * import, `firstrun` has nothing set.
 */
import type { PasswordImportResult, PasswordInfo, PasswordsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { KeyRound, MoreHorizontal, Plus, ShieldCheck, Trash2 } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";

import { Badge, Button, Card, IconButton, Input, Menu, Notice, Skeleton, errorText, relativeTime, useConfirm, useLive, useToast } from "../../ui";
import { SECRET_INFO, isAppSecret, secretLabel, tildePath } from "./model";
import { secretActionId } from "./parts";

const NAME_RE = /^[A-Z][A-Z0-9_]{0,63}$/;

/** A row's title: the service the password is for. Its name stays as mono detail. */
export function passwordTitle(p: Pick<PasswordInfo, "name" | "used_by" | "ghost">): string {
  if (TITLES[p.name]) return TITLES[p.name];
  if (p.used_by[0]) return p.used_by[0];
  return isAppSecret(p.name) ? secretLabel(p.name) : p.name;
}
const TITLES: Record<string, string> = {
  ANTHROPIC_AUTH_TOKEN: "CLIProxy",
  ANTHROPIC_API_KEY: "Anthropic",
  OPENAI_API_KEY: "OpenAI",
  LLM_API_KEY: "Your model address",
  OPENROUTER_API_KEY: "OpenRouter",
  TYPESAFE_API_KEY: "TypeSafe",
  ELEVENLABS_API_KEY: "ElevenLabs",
  HF_TOKEN: "Hugging Face",
  GITHUB_TOKEN: "GitHub",
};

/** "The model connection · last used 3 min ago": what uses it, then when. */
function usedWords(p: PasswordInfo): string {
  const by = p.ghost ? p.used_by.join("; ") : p.used_by.length ? `Used by ${p.used_by.join(", ")}` : "Nothing names it yet";
  if (!p.set) return by;
  return `${by} · ${p.last_used ? `last used ${relativeTime(p.last_used)}` : "not used yet"}`;
}

interface Props {
  /** A password Ghost itself reads changed: the save bar's restart pill takes it. */
  onRestart: (names: string[]) => void;
  /** Names saved but not yet in use (a restart owed). */
  waiting: Set<string>;
  /** Something changed that GET /settings reports (set or not). */
  onChanged: () => void;
}

export function PasswordsSection({ onRestart, waiting, onChanged }: Props) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [view, setView] = useState<PasswordsView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null); // the row being edited, or "+" for a new one
  const [unlocking, setUnlocking] = useState(false);
  const [importing, setImporting] = useState(false);
  const [imported, setImported] = useState<PasswordImportResult | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await client.passwords());
      setLoadError(null);
    } catch (e) {
      setLoadError(errorText(e));
    }
  }, [client]);
  useEffect(() => {
    void load();
  }, [load]);

  const after = (v: PasswordsView, restart: boolean, names: string[]) => {
    setView(v);
    onChanged();
    if (restart) onRestart(names);
  };

  const unlock = async () => {
    setUnlocking(true);
    setActionError(null);
    try {
      const v = await client.unlockPasswords();
      setView(v);
      onChanged();
      if (v.vault.state !== "open") setActionError("Your keyring is still locked. Unlock it in your keyring app, or log in again, then try here.");
    } catch (e) {
      setActionError(errorText(e));
    } finally {
      setUnlocking(false);
    }
  };

  const runImport = async () => {
    setImporting(true);
    setActionError(null);
    try {
      const r = await client.importPasswords();
      setImported(r);
      after(r.passwords, r.restart_required, r.imported);
    } catch (e) {
      setActionError(errorText(e));
    } finally {
      setImporting(false);
    }
  };

  const remove = async (p: PasswordInfo) => {
    const title = passwordTitle(p);
    const ok = await confirm({
      title: `Delete the ${title} password?`,
      description: p.used_by.length
        ? `${p.used_by.join(", ")} can’t use it after this. You’d paste it again to bring it back.`
        : "Nothing in Ghost names it. You’d paste it again to bring it back.",
      confirmLabel: `Delete the ${title} password`,
      tone: "danger",
    });
    if (!ok) return;
    try {
      const r = await client.deletePassword(p.name);
      after(r.passwords, r.restart_required, [p.name]);
      toast({ title: "Deleted", description: r.restart_required ? "Ghost stops using it at its next start." : `${title} is gone from the vault.` });
    } catch (e) {
      toast({ title: "Couldn’t delete it", description: errorText(e), tone: "danger" });
    }
  };

  if (!view) {
    return loadError ? (
      <Notice tone="danger" title="Couldn’t read the passwords" action={<Button size="sm" onClick={() => void load()}>Try again</Button>}>
        {loadError} Check that Ghost is running, then try again.
      </Notice>
    ) : (
      <Card list>
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="set-secret">
            <Skeleton width={180} height={12} />
          </div>
        ))}
      </Card>
    );
  }

  const vault = view.vault;
  const closed = vault.state !== "open";
  const importable = view.importable ?? [];
  const own = view.passwords.filter((p) => p.ghost);
  const others = view.passwords.filter((p) => !p.ghost);

  return (
    <div className="set-pw">
      {dialog}
      {vault.state === "locked" && (
        <Notice
          tone="warn"
          title="Your keyring is locked"
          action={
            <Button size="sm" loading={unlocking} disabled={!online} onClick={() => void unlock()}>
              Unlock
            </Button>
          }
        >
          Ghost’s passwords are in a vault your keyring unlocks. Until it does, Ghost can’t reach the model or the apps that need one; it keeps trying on its own.
        </Notice>
      )}
      {(vault.state === "broken" || vault.state === "unavailable") && (
        <Notice tone="danger" title="Ghost can’t open its passwords">
          {vault.problem ? `${vault.problem[0].toUpperCase()}${vault.problem.slice(1)}.` : null} Nothing was changed or overwritten. The vault is <span className="t-mono">{vault.path}</span>.
        </Notice>
      )}
      {actionError && (
        <Notice tone="danger" title="That didn’t work" onDismiss={() => setActionError(null)}>
          {actionError}
        </Notice>
      )}
      {imported && (
        <Notice tone="ok" title={imported.imported.length ? `Imported ${imported.imported.length} password${imported.imported.length === 1 ? "" : "s"}` : "Nothing new to import"} onDismiss={() => setImported(null)}>
          {importWords(imported)}
        </Notice>
      )}
      {importable.length > 0 && !closed && (
        <Notice
          tone="info"
          title="Some passwords are still in plain files"
          action={
            <Button size="sm" variant="primary" loading={importing} disabled={!online} onClick={() => void runImport()}>
              Import
            </Button>
          }
        >
          Import moves them into Ghost’s vault, once, then deletes the plain copies:{" "}
          {importable.map((s, i) => (
            <span key={s.path}>
              {i > 0 && (i === importable.length - 1 ? " and " : ", ")}
              <span className="t-mono">{tildePath(s.path)}</span>
              {s.kind === "heka_vault" ? " (read from heka-vault, then left for you to delete)" : ""}
            </span>
          ))}
          .
        </Notice>
      )}

      <Card list>
        {own.map((p) => (
          <PasswordRow key={p.name} p={p} open={open === p.name} onOpen={() => setOpen(p.name)} onClose={() => setOpen(null)} onSaved={after} onDelete={remove} waiting={waiting.has(p.name)} disabled={!online || closed} />
        ))}
      </Card>
      {(others.length > 0 || open === "+") && (
        <Card list>
          {others.map((p) => (
            <PasswordRow key={p.name} p={p} open={open === p.name} onOpen={() => setOpen(p.name)} onClose={() => setOpen(null)} onSaved={after} onDelete={remove} waiting={waiting.has(p.name)} disabled={!online || closed} />
          ))}
          {open === "+" && <NewPassword taken={view.passwords.filter((p) => p.set).map((p) => p.name)} onClose={() => setOpen(null)} onSaved={after} />}
        </Card>
      )}
      <div className="set-pw__foot">
        {open !== "+" && (
          <Button variant="ghost" size="sm" leadingIcon={<Plus size={14} aria-hidden />} disabled={!online || closed} onClick={() => setOpen("+")}>
            Add a password…
          </Button>
        )}
        <p className="set-promise-line">
          <ShieldCheck size={14} aria-hidden />
          Once saved, a password is never shown again: not here, not to Ghost’s model, not in a log.
        </p>
      </div>
    </div>
  );
}

function importWords(r: PasswordImportResult): string {
  const parts: string[] = [];
  if (r.imported.length) parts.push(`${r.imported.join(", ")} ${r.imported.length === 1 ? "is" : "are"} in the vault now.`);
  if (r.kept.length) parts.push(`${r.kept.length} the vault already had ${r.kept.length === 1 ? "was" : "were"} kept as they were.`);
  if (r.settings.length) parts.push(`Moved ${r.settings.join(", ")} to settings.env, which isn’t secret.`);
  if (r.removed.length) parts.push(`Deleted ${r.removed.map(tildePath).join(" and ")}.`);
  parts.push(...r.notes);
  if (r.restart_required) parts.push("Restart Ghost so it uses them.");
  return parts.join(" ");
}

/* ------------------------------------------------------------------------------ a row */

export function PasswordRow({
  p,
  open,
  onOpen,
  onClose,
  onSaved,
  onDelete,
  waiting,
  disabled,
}: {
  p: PasswordInfo;
  open: boolean;
  onOpen: () => void;
  onClose: () => void;
  onSaved: (v: PasswordsView, restart: boolean, names: string[]) => void;
  onDelete: (p: PasswordInfo) => void;
  waiting: boolean;
  disabled: boolean;
}) {
  const { client } = useGhost();
  const toast = useToast();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const action = useRef<HTMLButtonElement>(null);
  const title = passwordTitle(p);
  const inputId = `set-secret-${p.name}`;
  const helpId = `${inputId}-help`;
  const info = SECRET_INFO[p.name];
  // Required and missing is the one that asks for attention: the model's, before setup is done.
  const variant = !p.set && p.ghost && !info?.optional ? "secondary" : "ghost";

  const close = () => {
    setValue("");
    setError(null);
    onClose();
    requestAnimationFrame(() => action.current?.focus({ preventScroll: true }));
  };
  const save = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!value.trim()) return setError("Paste the password, or press Cancel.");
    setBusy(true);
    setError(null);
    try {
      const r = await client.setPassword(p.name, value.trim());
      setValue("");
      onSaved(r.passwords, r.restart_required, [p.name]);
      onClose();
      toast({ title: "Saved", description: r.restart_required ? "Ghost uses it from its next start." : `Ghost uses the ${title} password now.`, tone: "ok" });
      requestAnimationFrame(() => document.getElementById(secretActionId(p.name))?.focus({ preventScroll: true }));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="set-secret" onSubmit={save} autoComplete="off" aria-label={title}>
      <div className="set-secret__main">
        <div className="set-secret__text">
          <p className="set-secret__label">
            {open ? <label htmlFor={inputId}>{title}</label> : <span>{title}</span>}
            {title !== p.name && <span className="set-detail">{p.name}</span>}
          </p>
          <p className="set-help" id={helpId}>
            {waiting && <span className="set-waits">Waits for a restart. </span>}
            {usedWords(p)}
          </p>
        </div>
        <div className="set-secret__state">
          {p.set ? (
            <Badge tone="ok" size="sm" dot>
              Set
            </Badge>
          ) : (
            <Badge size="sm">Not set</Badge>
          )}
          <span className="set-pw__actions">
            {open ? (
              <Button size="sm" variant="ghost" className="set-secret__btn" onClick={close} aria-label={`Cancel ${title}`}>
                Cancel
              </Button>
            ) : (
              <Button ref={action} id={secretActionId(p.name)} size="sm" variant={variant} className="set-secret__btn" onClick={onOpen} disabled={disabled} aria-label={`${p.set ? "Replace" : "Add"} the ${title} password`}>
                {p.set ? "Replace…" : "Add…"}
              </Button>
            )}
            {p.set && !open ? (
              <Menu
                align="end"
                trigger={<IconButton size="sm" variant="ghost" label={`More for the ${title} password`} icon={<MoreHorizontal />} disabled={disabled} />}
                items={[{ label: "Delete…", icon: <Trash2 />, danger: true, onSelect: () => onDelete(p) }]}
              />
            ) : (
              <span aria-hidden />
            )}
          </span>
        </div>
      </div>
      {open && (
        <div className="set-secret__edit">
          <div className="set-secret__field">
            <Input
              id={inputId}
              type="password"
              autoComplete="off"
              autoFocus
              spellCheck={false}
              mono
              invalid={!!error}
              leading={<KeyRound size={14} aria-hidden />}
              placeholder={info?.placeholder ?? "Paste the password"}
              aria-describedby={helpId}
              value={value}
              onChange={(e) => (setValue(e.target.value), setError(null))}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  e.stopPropagation();
                  close();
                }
              }}
            />
            {error && <p className="set-error">{error}</p>}
          </div>
          <Button type="submit" size="sm" variant="secondary" loading={busy}>
            Save
          </Button>
        </div>
      )}
    </form>
  );
}

/* --------------------------------------------------------------------- a new one */

function NewPassword({ taken, onClose, onSaved }: { taken: string[]; onClose: () => void; onSaved: (v: PasswordsView, restart: boolean, names: string[]) => void }) {
  const { client } = useGhost();
  const toast = useToast();
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ field: "name" | "value" | "save"; text: string } | null>(null);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    const n = name.trim();
    if (!NAME_RE.test(n)) return setError({ field: "name", text: "Capital letters, digits and _, starting with a letter: LINEAR_TOKEN." });
    if (taken.includes(n)) return setError({ field: "name", text: `${n} is already saved. Use its Replace… instead.` });
    if (!value.trim()) return setError({ field: "value", text: "Paste the password." });
    setBusy(true);
    setError(null);
    try {
      const r = await client.setPassword(n, value.trim());
      setValue("");
      onSaved(r.passwords, r.restart_required, [n]);
      onClose();
      toast({ title: "Saved", description: `Anything that names \${${n}} uses it now.`, tone: "ok" });
    } catch (err) {
      setError({ field: "save", text: errorText(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="set-secret set-pw__new" onSubmit={save} autoComplete="off" aria-label="Add a password">
      <p className="set-secret__label">Add a password</p>
      <p className="set-help">For a password an app or a webhook you set up by hand names as <span className="t-mono">{"${NAME}"}</span>.</p>
      <div className="set-secret__edit">
        <div className="set-secret__field">
          <Input
            aria-label="Name"
            mono
            autoFocus
            spellCheck={false}
            placeholder="LINEAR_TOKEN"
            value={name}
            invalid={error?.field === "name"}
            onChange={(e) => (setName(e.target.value.toUpperCase()), setError(null))}
          />
        </div>
        <div className="set-secret__field">
          <Input
            aria-label="Password"
            type="password"
            autoComplete="off"
            mono
            spellCheck={false}
            leading={<KeyRound size={14} aria-hidden />}
            placeholder="Paste the password"
            value={value}
            invalid={error?.field === "value"}
            onChange={(e) => (setValue(e.target.value), setError(null))}
          />
        </div>
        <Button type="submit" size="sm" variant="secondary" loading={busy}>
          Save
        </Button>
        <Button size="sm" variant="ghost" onClick={onClose}>
          Cancel
        </Button>
      </div>
      {error && <p className="set-error">{error.text}</p>}
    </form>
  );
}
