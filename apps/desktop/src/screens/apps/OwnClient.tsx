/**
 * Owner: apps. The person's own client for signing in to an app (ADR 0076). Most apps let Ghost
 * in by themselves; a few (GitHub, Google, Slack) only let in apps registered with them, and
 * until Ghost has its own there, the person registers one and gives Ghost its client ID (and
 * its secret, where the service wants one). So the field is never one everybody sees: it shows
 * when a sign-in was refused for lack of one, or under "I have my own client ID".
 *
 * The secret is a password: sent once into Ghost's vault and never shown again.
 */
import type { SignInClientView } from "@ghost/client-ts";
import { ExternalLink } from "lucide-react";
import { useState } from "react";

import { Button, Field, Input, Switch } from "../../ui";
import { openExternal } from "./model";

export interface OwnClientValue {
  client_id: string;
  client_secret: string;
  /** Take the stored secret away. An empty secret field keeps it. */
  forget_secret: boolean;
}

/** What PUT /apps/{name}/sign-in-client is sent. */
export interface OwnClientBody {
  client_id: string | null;
  client_secret: string | null;
  forget_secret?: boolean;
}

/** The two fields, and the vendor's page where a client is made when Ghost knows it. */
export function OwnClientFields({
  client,
  value,
  onChange,
  missing = false,
  said = false,
}: {
  client: SignInClientView | null | undefined;
  value: OwnClientValue;
  onChange: (v: OwnClientValue) => void;
  missing?: boolean;
  /** The sentence above already says to register one: only the button to the vendor's page. */
  said?: boolean;
}) {
  const secret = client?.secret ?? "optional";
  const service = client?.service ?? null;
  return (
    <div className="apps-own-client">
      {client?.make_a_client && service && (
        <p className="t-small t-2">
          {said ? "" : `Register an app of your own with ${service}, then copy what it gives you here. `}
          <Button size="sm" variant="ghost" leadingIcon={<ExternalLink />} onClick={() => void openExternal(client.make_a_client!)}>
            Open {service}’s page for it
          </Button>
        </p>
      )}
      <Field label="Client ID" error={missing ? "A client ID is needed to sign in this way." : undefined} help={client?.client_id ? "Ghost signs in as this one now." : undefined}>
        <Input mono autoComplete="off" spellCheck={false} value={value.client_id} placeholder={client?.client_id ?? "Paste it here"} onChange={(e) => onChange({ ...value, client_id: e.target.value })} />
      </Field>
      {secret !== "none" && (
        <Field
          label="Client secret"
          optional={secret === "optional" || !!client?.has_secret}
          help={
            client?.has_secret
              ? value.forget_secret
                ? "The stored secret is taken away when you sign in with it."
                : "A secret is kept for it. Leave this empty to keep it, or type a new one to replace it."
              : "A password: kept in Ghost’s vault, and never shown again."
          }
        >
          <Input
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={value.client_secret}
            disabled={value.forget_secret}
            placeholder={client?.has_secret ? "Kept. Type a new one to replace it" : "Paste it here"}
            onChange={(e) => onChange({ ...value, client_secret: e.target.value })}
          />
        </Field>
      )}
      {client?.has_secret && (
        <Switch
          size="sm"
          checked={value.forget_secret}
          label="Remove the stored secret"
          onChange={(on) => onChange({ ...value, forget_secret: on, client_secret: on ? "" : value.client_secret })}
        />
      )}
    </div>
  );
}

export const emptyOwnClient = (client_id = ""): OwnClientValue => ({ client_id, client_secret: "", forget_secret: false });

/**
 * The fields' state and what to send. An empty secret field keeps the stored secret; only the
 * switch takes it away.
 */
export function useOwnClient(initial?: string | null) {
  const [value, setValue] = useState<OwnClientValue>(emptyOwnClient(initial ?? ""));
  const body = () => ownClientBody(value);
  return { value, setValue, body };
}

/** What the fields send: an empty secret is "keep the stored one", never "none". */
export function ownClientBody(value: OwnClientValue): OwnClientBody {
  return {
    client_id: value.client_id.trim() || null,
    client_secret: value.forget_secret ? null : value.client_secret.trim() || null,
    ...(value.forget_secret ? { forget_secret: true } : {}),
  };
}
