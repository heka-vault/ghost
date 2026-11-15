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

import { Button, Field, Input } from "../../ui";
import { openExternal } from "./model";

export interface OwnClientValue {
  client_id: string;
  client_secret: string;
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
          optional={secret === "optional"}
          help={client?.has_secret ? "A secret is kept for it. Type a new one to replace it, or leave this empty to keep none." : "A password: kept in Ghost’s vault, and never shown again."}
        >
          <Input type="password" autoComplete="off" spellCheck={false} value={value.client_secret} placeholder="Paste it here" onChange={(e) => onChange({ ...value, client_secret: e.target.value })} />
        </Field>
      )}
    </div>
  );
}

export function useOwnClient(initial?: string | null) {
  const [value, setValue] = useState<OwnClientValue>({ client_id: initial ?? "", client_secret: "" });
  const body = () => ({ client_id: value.client_id.trim() || null, client_secret: value.client_secret.trim() || null });
  return { value, setValue, body };
}
