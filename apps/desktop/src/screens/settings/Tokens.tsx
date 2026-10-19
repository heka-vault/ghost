/**
 * Owner: settings. GitHub tokens by owner (ADR 0019): a small table of owner -> the name of
 * the environment variable holding that owner's token. Names only; the values are secrets
 * that live in the environment (the vault) and are never shown or typed here. Rows apply on
 * Save like every other setting. Stored as GHOST_GITHUB_TOKENS="owner=ENV_NAME,owner2=ENV_NAME2".
 */
import { Plus, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Button, IconButton, Input, Table, useFieldControl } from "../../ui";
import { type TokenRow, joinTokenMap, parseTokenMap } from "./model";

export interface TokenMapEditorProps {
  value: string;
  onChange: (next: string) => void;
  disabled?: boolean;
  id?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: unknown;
}

export function TokenMapEditor({ value, onChange, disabled, ...rest }: TokenMapEditorProps) {
  const a = useFieldControl(rest);
  // Rows are kept locally so an owner typed before its variable name doesn't vanish on the
  // way through the serialized value; the value is re-read when it changes from outside.
  const [rows, setRows] = useState<TokenRow[]>(() => parseTokenMap(value));
  const last = useRef(joinTokenMap(rows));
  useEffect(() => {
    if (value !== last.current) {
      last.current = value;
      setRows(parseTokenMap(value));
    }
  }, [value]);
  const update = (next: TokenRow[]) => {
    setRows(next);
    const joined = joinTokenMap(next);
    last.current = joined;
    onChange(joined);
  };
  // Focus lands in the owner cell of a row just added, and on Add after a removal.
  const focusRow = useRef<number | null>(null);
  const addRef = useRef<HTMLButtonElement>(null);
  const table = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const i = focusRow.current;
    if (i == null) return;
    focusRow.current = null;
    if (i < 0) addRef.current?.focus({ preventScroll: true });
    else table.current?.querySelectorAll<HTMLInputElement>("input")[i * 2]?.focus({ preventScroll: true });
  }, [rows.length]);

  return (
    <div className="set-tokens" id={a.id} aria-describedby={a["aria-describedby"]} ref={table}>
      {rows.length > 0 && (
        <Table density="compact" className="set-tokens__table">
          <thead>
            <tr>
              <th>Owner</th>
              <th>Variable with its token</th>
              <th className="is-shrink">
                <span className="sr-only">Remove</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r, i) => (
              <tr key={i}>
                <td>
                  <Input
                    size="sm"
                    value={r.owner}
                    placeholder="heka-vault"
                    spellCheck={false}
                    autoCapitalize="off"
                    disabled={disabled}
                    aria-label={`Owner, row ${i + 1}`}
                    aria-invalid={a["aria-invalid"] as boolean | undefined}
                    onChange={(e) => update(rows.map((x, j) => (j === i ? { ...x, owner: e.target.value } : x)))}
                  />
                </td>
                <td>
                  <Input
                    size="sm"
                    mono
                    value={r.env}
                    placeholder="HEKA_ORG_TOKEN"
                    spellCheck={false}
                    autoCapitalize="characters"
                    disabled={disabled}
                    aria-label={`Variable name, row ${i + 1}`}
                    aria-invalid={a["aria-invalid"] as boolean | undefined}
                    onChange={(e) => update(rows.map((x, j) => (j === i ? { ...x, env: e.target.value } : x)))}
                  />
                </td>
                <td className="is-shrink">
                  <IconButton
                    size="sm"
                    variant="ghost"
                    icon={<X />}
                    label={`Remove ${r.owner.trim() || `row ${i + 1}`}`}
                    disabled={disabled}
                    onClick={() => {
                      focusRow.current = -1;
                      update(rows.filter((_, j) => j !== i));
                    }}
                  />
                </td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
      <Button
        ref={addRef}
        size="sm"
        variant="secondary"
        leadingIcon={<Plus />}
        disabled={disabled}
        onClick={() => {
          focusRow.current = rows.length;
          update([...rows, { owner: "", env: "" }]);
        }}
      >
        {rows.length ? "Add an owner" : "Add a token for an owner"}
      </Button>
    </div>
  );
}
