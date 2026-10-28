/**
 * Owner: apps. Add an app that isn't in the catalog (ADR 0034): by its address, with an
 * optional key (without one Ghost signs in through the browser), or by pasting another
 * app's config, whose passwords Ghost moves into its vault (ADR 0036). Behind a quiet link, with
 * one line of caution: Ghost uses every tool an app offers.
 */
import { ApiError } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ExternalLink } from "lucide-react";
import { type FormEvent, useEffect, useId, useState } from "react";

import { useDesign } from "../../design/context";
import { Button, Dialog, Disclosure, Field, Input, Notice, SegmentedControl, Textarea, errorText, useLive, useToast } from "../../ui";
import { connectOutcome, failedText } from "./ConnectDialog";
import { sentence } from "./model";
import { expectSignIn, openSignIn } from "./signin";

type Way = "url" | "config";

const CONFIG_EXAMPLE = `{
  "mcpServers": {
    "files": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "~/Documents"]
    }
  }
}`;

export function AddOwnDialog({ open, onClose, onConnected }: { open: boolean; onClose: () => void; onConnected: () => void }) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const design = useDesign();
  const mock = import.meta.env.DEV && !!design;
  const formId = useId();
  const [way, setWay] = useState<Way>("url");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [name, setName] = useState("");
  const [config, setConfig] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ title: string; text: string; taken: boolean; failed?: boolean } | null>(null);
  /** Saved by an earlier try that then didn't connect: the next try replaces it. */
  const [saved, setSaved] = useState(false);
  const [blank, setBlank] = useState(false);
  const [signIn, setSignIn] = useState<{ url: string; opened: boolean } | null>(null);

  // A fresh dialog each time it opens.
  useEffect(() => {
    if (!open) return;
    setWay("url");
    setUrl("");
    setKey("");
    setName("");
    setConfig("");
    setError(null);
    setSaved(false);
    setBlank(false);
    setSignIn(null);
  }, [open]);

  const connect = async (replace = false) => {
    if (busy || !online) return;
    const text = way === "url" ? url.trim() : config.trim();
    if (!text) {
      setBlank(true);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const again = replace || saved || undefined;
      const body =
        way === "url"
          ? { url: text, name: name.trim() || undefined, values: key.trim() ? { key: key.trim() } : undefined, replace: again }
          : { config: text, replace: again };
      if (way === "url" && name.trim()) expectSignIn(name.trim());
      const r = await client.connectApp(body);
      for (const a of r.apps) expectSignIn(a.name);
      onConnected();
      const names = r.apps.map((a) => a.title || a.name);
      const { failed, connecting } = connectOutcome(r.apps);
      // What Ghost noticed about a paste that still connected (a key it moved, an entry it
      // left out): said with the result, never dropped.
      const notes = r.problems.length ? ` ${r.problems.map(sentence).join(" ")}` : "";
      if (failed) {
        setSaved(true);
        setError({ title: `Ghost couldn’t connect to ${failed.title || failed.name}`, text: failedText(failed), taken: false, failed: true });
      } else if (r.sign_in_url) {
        const opened = await openSignIn(r.sign_in_url, { again: true, mock });
        setSignIn({ url: r.sign_in_url, opened });
      } else if (connecting) {
        toast({ title: names.length === 1 ? `Connecting to ${names[0]}…` : `Connecting to ${names.length} apps…`, description: `Ghost is still reaching it. It shows as connected under Apps once it answers.${notes}` });
        onClose();
      } else {
        toast({ title: names.length === 1 ? `Connected ${names[0]}` : `Connected ${names.length} apps`, description: `Ghost can use it from its next turn. Its tools are on its page under Apps.${notes}` });
        onClose();
      }
    } catch (err) {
      const taken = err instanceof ApiError && err.status === 409;
      setError({ title: taken ? "Already connected under that name" : "Couldn’t add it", text: sentence(errorText(err)), taken });
    } finally {
      setBusy(false);
    }
  };

  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    void connect();
  };

  if (open && signIn) {
    return (
      <Dialog
        open
        onClose={onClose}
        size="md"
        title="Finish signing in"
        description={signIn.opened ? "Ghost opened the app’s sign-in page in your browser. Sign in there; it shows as connected under Apps once you’re done." : "Ghost couldn’t open your browser. Open the sign-in page yourself; the app shows as connected under Apps once you’re done."}
        footer={
          <>
            <Button variant="ghost" leadingIcon={<ExternalLink />} onClick={() => void openSignIn(signIn.url, { again: true, mock })}>
              Open the sign-in page again
            </Button>
            <Button variant="primary" onClick={onClose}>
              Done
            </Button>
          </>
        }
      >
        <p className="apps-signin-url t-mono t-small t-3">{new URL(signIn.url).host}</p>
      </Dialog>
    );
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title="Add your own app"
      description="Only add apps you trust: Ghost uses every tool an app offers, and tells you what it changed."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={formId} loading={busy} disabled={!online}>
            Add the app
          </Button>
        </>
      }
    >
      <form id={formId} className="apps-form" onSubmit={submit} noValidate>
        <SegmentedControl<Way>
          aria-label="How to add it"
          value={way}
          fullWidth
          onChange={(w) => {
            setWay(w);
            setError(null);
            setBlank(false);
          }}
          options={[
            { value: "url", label: "By its address" },
            { value: "config", label: "Paste a config" },
          ]}
        />
        {way === "url" ? (
          <>
            <Field label="Address" help="The app’s web address for assistants, from its docs." error={blank ? "Type the app’s address, starting with https://." : undefined}>
              <Input
                type="url"
                mono
                autoComplete="off"
                spellCheck={false}
                placeholder="https://mcp.example.com/mcp"
                value={url}
                onChange={(e) => {
                  setUrl(e.target.value);
                  setBlank(false);
                }}
              />
            </Field>
            <Field label="Password" optional help="The password the app gives you. Leave it empty to sign in through your browser instead. A password is kept in Ghost’s vault and isn’t shown again.">
              <Input type="password" autoComplete="off" placeholder="Paste it here" value={key} onChange={(e) => setKey(e.target.value)} />
            </Field>
            <Disclosure summary="Advanced" defaultOpen={!!error?.taken}>
              <Field label="Name in Ghost" optional help="Rules and the audit log call the app by this name. Lower-case letters and dashes; Ghost picks one from the address otherwise.">
                <Input mono placeholder="example" value={name} onChange={(e) => setName(e.target.value.toLowerCase())} />
              </Field>
            </Disclosure>
          </>
        ) : (
          <Field
            label="Config"
            help="From Claude, Cursor or another app: the whole file or one entry. Passwords in it are moved into Ghost’s vault."
            error={blank ? "Paste the config to add." : undefined}
          >
            <Textarea
              mono
              rows={9}
              spellCheck={false}
              placeholder={CONFIG_EXAMPLE}
              value={config}
              onChange={(e) => {
                setConfig(e.target.value);
                setBlank(false);
              }}
            />
          </Field>
        )}
        {error && (
          <Notice
            tone="danger"
            title={error.title}
            className="apps-form__notice"
            action={
              error.taken || error.failed ? (
                <Button size="sm" variant="secondary" loading={busy} onClick={() => void connect(true)}>
                  {error.failed ? "Try again" : "Replace it"}
                </Button>
              ) : undefined
            }
          >
            {error.text} {error.taken && way === "url" ? "Pick another name under Advanced, or replace the one you have." : ""}
          </Notice>
        )}
      </form>
    </Dialog>
  );
}
