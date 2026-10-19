/**
 * Owner: conversations. What a line may touch (ADR 0019): for every ability that can be
 * scoped, a checklist of the items the operator allows (the ability's whole allowlist), and
 * the line's scope is the checked subset. Nothing checked for an ability means the line sees
 * everything that ability allows, so the help line says so rather than leaving it to guess.
 * Checks apply on Save, so these are Checkboxes, not Switches.
 */
import { Card, Checkbox, Skeleton } from "../../ui";
import { type Scope, type ScopedAbility, abilityWords, kindWords } from "./threads";

export interface ScopePickerProps {
  /** The abilities that declare a scope kind; null while they load. */
  abilities: ScopedAbility[] | null;
  value: Scope;
  onChange: (scope: Scope) => void;
  disabled?: boolean;
  /** Prefix for element ids, so two pickers (a line page and the new-line dialog) never collide. */
  idPrefix: string;
}

export function ScopePicker({ abilities, value, onChange, disabled, idPrefix }: ScopePickerProps) {
  if (!abilities) {
    return (
      <Card list aria-busy="true" aria-label="Loading scope">
        {[0, 1].map((i) => (
          <div key={i} className="conv-scope-skel">
            <Skeleton width={16} height={16} radius="var(--radius-xs)" />
            <Skeleton width={i ? 140 : 120} height={12} />
          </div>
        ))}
      </Card>
    );
  }
  if (!abilities.length) {
    return <p className="t-small t-2">Nothing to scope yet: no installed ability says what it can be limited to.</p>;
  }
  return (
    <div className="conv-scope">
      {abilities.map((a) => {
        const words = kindWords(a.scope_kind);
        const picked = value[a.name] ?? [];
        // Items the line names that the allowlist no longer has stay visible until unchecked.
        const gone = picked.filter((i) => !a.scope_items.includes(i));
        const items = [...a.scope_items, ...gone];
        const toggle = (item: string, on: boolean) => {
          const next = on ? [...picked.filter((i) => i !== item), item] : picked.filter((i) => i !== item);
          const scope = { ...value };
          if (next.length) scope[a.name] = next;
          else delete scope[a.name];
          onChange(scope);
        };
        return (
          <div key={a.name} className="conv-scope__ability">
            {abilities.length > 1 && (
              <p className="conv-scope__head">
                <span className="t-medium">{abilityWords(a.name)}</span>
                <span className="t-mono t-3">{a.name}</span>
              </p>
            )}
            <Card list>
              {items.length === 0 ? (
                <p className="conv-scope__empty t-small t-2">No {words.many} are allowed yet. Add them under Settings, GitHub.</p>
              ) : (
                items.map((item) => (
                  <div key={item} className="conv-scope__item">
                    <Checkbox
                      id={`${idPrefix}-${a.name}-${item.replace(/[^A-Za-z0-9_-]+/g, "-")}`}
                      checked={picked.includes(item)}
                      disabled={disabled}
                      onChange={(on) => toggle(item, on)}
                      label={<span className="t-mono">{item}</span>}
                      description={gone.includes(item) ? "Not in the allowlist anymore. Uncheck it to drop it." : undefined}
                    />
                  </div>
                ))
              )}
            </Card>
            <p className="conv-scope__help t-small t-3">
              {picked.length === 0
                ? `Nothing checked: this line sees every ${words.one} Ghost may touch.`
                : picked.length === 1
                  ? `Only this ${words.one}. Ghost won’t need the name: ${quotedExample(picked[0])}.`
                  : `${picked.length} of ${a.scope_items.length} ${words.many}. Nothing outside them, in this line.`}
            </p>
          </div>
        );
      })}
    </div>
  );
}

/** “close seventeen” for a one-repository line: the spoken form needs no repository. */
function quotedExample(item: string): string {
  const short = item.split("/").pop() ?? item;
  return `“close seventeen” means ${short} 17`;
}
