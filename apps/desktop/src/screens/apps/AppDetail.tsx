/**
 * Owner: apps. #/apps/:name: one app (ADR 0034). Whether Ghost is connected to it, and each
 * of its tools with what Ghost may do: a change is Allow (it runs and Ghost says so), Ask
 * first or Block, with Quiet in the row's menu; a read is Allow or Block. A choice moves the
 * tool's own rule in the operator's permissions (`PUT /permissions`, `set`); choosing what
 * the tool would get without a rule clears it, so the file holds only exceptions. A broader
 * rule that still decides the tool is named under it.
 */
import type { AppAction, AppView, PermissionsView } from "@ghost/client-ts";
import { permissionLabel, useGhost } from "@ghost/ui";
import { ChevronLeft, ExternalLink, Megaphone, MoreHorizontal, RotateCw, SearchX, VolumeX } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { Link, hrefFor, navigate } from "../../app/router";
import { Badge, Button, Card, EmptyState, Field, IconButton, LiveStatus, Menu, Notice, Page, SegmentedControl, Section, Skeleton, SkeletonText, Switch, errorText, quoted, useLive, useToast } from "../../ui";
import { useAppActions } from "./actions";
import { AppIcon } from "./AppIcon";
import { type Level, appConditionalRules, choose, conditionalRules, coversWholeApp, decidingRule, kindWords, listOf, listVerb, sentence, stateWords, toolLabel } from "./model";

const MODE_WORDS: Record<PermissionsView["mode"], string> = { open: "Open", ask: "Ask before changes", "read-only": "Read only" };

export function AppDetail({ name }: { name: string }) {
  const { client, appsVersion } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [app, setApp] = useState<AppView | null>(null);
  const [perms, setPerms] = useState<PermissionsView | null>(null);
  const [missing, setMissing] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** The rule being saved, so only its row waits. */
  const [saving, setSaving] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [a, p] = await Promise.all([client.app(name), client.permissions()]);
      setApp(a);
      setPerms(p);
      setLoadError(null);
    } catch (e) {
      if ((e as { status?: number }).status === 404) setMissing(true);
      else setLoadError(errorText(e));
    }
  }, [client, name]);

  useEffect(() => {
    void load();
  }, [load, appsVersion]);

  const actions = useAppActions({ onChanged: () => void load(), onDisconnected: () => navigate("/apps") });

  /** Move one rule, then read the app back: its levels are the gate's, not this screen's guess. */
  const setRule = async (rule: string, level: Level | null, opts: { what: string; undo?: Level | null }) => {
    if (!online || saving) return;
    setSaving(rule);
    setSaveError(null);
    try {
      const p = await client.updatePermissions({ set: { rule, level } });
      setPerms(p);
      setApp(await client.app(name));
      toast({
        tone: "ok",
        title: "Saved",
        description: `${opts.what} Ghost follows it from its next turn.`,
        action: opts.undo !== undefined ? { label: "Undo", onClick: () => void setRule(rule, opts.undo ?? null, { what: "Put back as it was." }) } : undefined,
      });
    } catch (e) {
      setSaveError(sentence(errorText(e)));
    } finally {
      setSaving(null);
    }
  };

  const pick = (action: AppAction, level: Level) => {
    if (!perms || !app) return;
    const next = choose(perms, action, app.name, level);
    const before = listOf(perms, action.rule);
    if (next.level === before) return;
    void setRule(next.rule, next.level, { what: `${toolLabel(action.name)}: ${permissionLabel(level).toLowerCase()}.`, undo: before });
  };

  if (missing) {
    return (
      <Page width="default" title="App not found" bare>
        <EmptyState
          size="page"
          icon={<SearchX />}
          title={`There’s no app called ${quoted(name)}`}
          description="It may have been disconnected, or the link has a typo."
          actions={<Button href={hrefFor("/apps")}>See all apps</Button>}
        />
      </Page>
    );
  }

  const title = app?.title ?? name;
  const state = app ? stateWords(app) : null;
  const changes = app?.actions.filter((a) => !a.reads) ?? [];
  const reads = app?.actions.filter((a) => a.reads) ?? [];
  const wholeBlocked = !!perms?.block.includes(name);
  const appConds = perms ? appConditionalRules(perms, name) : [];
  const busy = actions.busy === name;

  return (
    <Page
      width="default"
      title={title}
      leading={<AppIcon title={title} icon={app?.icon} size="lg" />}
      crumb={
        <Link to="/apps">
          <ChevronLeft size={14} aria-hidden />
          Apps
        </Link>
      }
      status={<LiveStatus />}
      description={
        app && state ? (
          <span className="apps-meta">
            <Badge tone={state.tone} dot>
              {state.label}
            </Badge>
            <span>{kindWords(app.kind)}</span>
            <span aria-hidden>·</span>
            <span className="t-mono">{app.name}</span>
          </span>
        ) : (
          <Skeleton width={260} height={14} />
        )
      }
      actions={
        app && (
          <>
            {app.sign_in_url ? (
              <Button variant="secondary" leadingIcon={<ExternalLink />} onClick={() => void actions.reopen(app)}>
                Open sign-in page
              </Button>
            ) : app.state === "needs_sign_in" ? (
              <Button variant="primary" loading={busy} disabled={!online} onClick={() => void actions.signIn(app)}>
                Sign in
              </Button>
            ) : app.state === "failed" || app.state === "stopped" ? (
              <Button variant="primary" leadingIcon={<RotateCw />} loading={busy} disabled={!online} onClick={() => void actions.signIn(app)}>
                Try again
              </Button>
            ) : null}
            <Menu
              align="end"
              trigger={<IconButton icon={<MoreHorizontal />} label={`More for ${title}`} />}
              items={[
                ...(app.kind === "sign_in" && app.state === "connected" ? [{ label: "Sign in again", onSelect: () => void actions.signIn(app), disabled: !online }] : []),
                { label: `Disconnect ${title}…`, danger: true, onSelect: () => void actions.disconnect(app), disabled: !online, reason: online ? undefined : "Ghost isn’t answering" },
              ]}
            />
          </>
        )
      }
    >
      {loadError && (
        <Notice tone="danger" title={app ? `Couldn’t refresh ${title}` : `Couldn’t load ${title}`} action={<Button size="sm" variant="secondary" onClick={() => void load()}>Try again</Button>} className="apps-notice">
          {sentence(loadError)}
        </Notice>
      )}
      {actions.problem && (
        <Notice tone="danger" title={actions.problem.title} onDismiss={actions.clearProblem} className="apps-notice">
          {actions.problem.text}
        </Notice>
      )}
      {saveError && (
        <Notice tone="danger" title="Couldn’t change the permission" onDismiss={() => setSaveError(null)} className="apps-notice">
          {saveError}
        </Notice>
      )}

      {!app || !perms ? (
        !loadError && <DetailSkeleton />
      ) : (
        <>
          <StateNotice app={app} onReopen={() => void actions.reopen(app)} />

          {perms.mode !== "open" && (
            <Notice tone="info" title={`Permissions are set to ${MODE_WORDS[perms.mode].toLowerCase()}`} className="apps-notice" action={<Button size="sm" variant="ghost" href={hrefFor("/permissions")}>Permissions</Button>}>
              {perms.mode === "ask" ? "Every change asks first unless you allow it here." : "Every change is blocked unless you allow it here. Reads still run."}
            </Notice>
          )}

          <Card className="apps-block-card">
            <Field
              layout="inline"
              controlWidth="auto"
              label={`Block ${title}`}
              detail={name}
              help={wholeBlocked ? `Ghost can’t see or use anything in ${title}, in any department, until you turn this off.` : `Turn on to keep Ghost out of ${title} entirely, without disconnecting it.`}
            >
              <Switch
                checked={wholeBlocked}
                disabled={!online || saving === name}
                onChange={(on) => void setRule(name, on ? "block" : null, { what: on ? `${title} is blocked.` : `${title} is no longer blocked.`, undo: on ? null : "block" })}
              />
            </Field>
            {appConds.length > 0 && (
              <p className="apps-whole-note">
                {appConds.length === 1 ? "A rule decides some calls to every tool: " : "Rules decide some calls to every tool: "}
                {appConds.map((c, i) => (
                  <span key={c.rule}>
                    {i > 0 && ", "}
                    <span className="t-mono apps-code">{c.rule}</span> {listVerb[c.list]}
                  </span>
                ))}
                . Change {appConds.length === 1 ? "it" : "them"} on <Link to="/permissions">Permissions</Link>.
              </p>
            )}
          </Card>

          <Section
            id="apps-changes"
            title={
              <>
                Makes changes <span className="apps-count t-num">{changes.length}</span>
              </>
            }
            description="Each goes through Ghost’s gate and into the audit log. Allow runs it and Ghost tells you; Ask first waits for your yes. To reverse one, ask Ghost for the opposite change."
          >
            {app.state !== "connected" ? (
              <NotYet title={title} />
            ) : changes.length === 0 ? (
              <p className="apps-none">{title} offers nothing that changes anything. Every tool is a read.</p>
            ) : (
              <Card list>
                {changes.map((a) => (
                  <ActionRow key={a.name} app={app} action={a} perms={perms} saving={saving === a.rule} disabled={!online || !!saving} onPick={(lv) => pick(a, lv)} />
                ))}
              </Card>
            )}
          </Section>

          <Section
            id="apps-reads"
            title={
              <>
                Looks things up <span className="apps-count t-num">{reads.length}</span>
              </>
            }
            description={`Reads never change anything. ${title} marks these as read-only.`}
          >
            {app.state !== "connected" ? (
              <NotYet title={title} />
            ) : reads.length === 0 ? (
              <p className="apps-none">{title} marks none of its tools read-only, so every one counts as a change.</p>
            ) : (
              <Card list>
                {reads.map((a) => (
                  <ActionRow key={a.name} app={app} action={a} perms={perms} saving={saving === a.rule} disabled={!online || !!saving} onPick={(lv) => pick(a, lv)} />
                ))}
              </Card>
            )}
          </Section>

          <p className="apps-foot t-small t-3">
            Rules call it <span className="t-mono">{name}</span>, and a tool <span className="t-mono">{name}.&lt;tool&gt;</span>.{" "}
            {app.settings.length > 0 && (
              <>
                Its {app.settings.length === 1 ? "key is" : "keys are"} in Ghost’s private settings as <span className="t-mono">{app.settings.join(", ")}</span>.{" "}
              </>
            )}
            Descriptions are {title}’s own words; Ghost never follows instructions in them.
          </p>
        </>
      )}
      {actions.dialog}
    </Page>
  );
}

/** What's in the way: a sign-in waiting in the browser, one to do again, or why it isn't connected. */
function StateNotice({ app: a, onReopen }: { app: AppView; onReopen: () => void }) {
  if (a.sign_in_url)
    return (
      <Notice tone="warn" title="Finish signing in in your browser" className="apps-notice" action={<Button size="sm" variant="secondary" leadingIcon={<ExternalLink />} onClick={onReopen}>Open it again</Button>}>
        {a.title} connects once you’ve signed in there.
      </Notice>
    );
  if (a.state === "needs_sign_in")
    return (
      <Notice tone="warn" title={`${a.title} needs you to sign in again`} className="apps-notice">
        <span data-user-content>{a.problem ? sentence(a.problem) : ""}</span> Until then Ghost can’t use it.
      </Notice>
    );
  if (a.state === "failed" || a.state === "stopped")
    return (
      <Notice tone="danger" title={`Ghost couldn’t connect to ${a.title}`} className="apps-notice">
        <span data-user-content>{a.problem ? sentence(a.problem) : "It didn’t answer."}</span> Nothing from it reaches Ghost until it connects.
      </Notice>
    );
  if (a.state === "connecting")
    return (
      <Notice tone="info" title={`Connecting to ${a.title}…`} className="apps-notice">
        Its tools show here once it answers.
      </Notice>
    );
  return null;
}

function NotYet({ title }: { title: string }) {
  return <p className="apps-none">Ghost sees what {title} can do once it connects.</p>;
}

const CHANGE_OPTIONS: { value: Level; label: string }[] = [
  { value: "allow", label: "Allow" },
  { value: "ask", label: "Ask first" },
  { value: "block", label: "Block" },
];
const READ_OPTIONS: { value: Level; label: string }[] = [
  { value: "allow", label: "Allow" },
  { value: "block", label: "Block" },
];

/**
 * One tool: its name as words and as the rule, the app's own short description (untrusted,
 * plain text), what decides it when that isn't its own rule, and the control. Quiet shows as
 * Allow with a “Quietly” tag: it runs, and Ghost doesn't mention it.
 */
function ActionRow({ app, action: a, perms, saving, disabled, onPick }: { app: AppView; action: AppAction; perms: PermissionsView; saving: boolean; disabled: boolean; onPick: (level: Level) => void }) {
  const decided = decidingRule(perms, app.name, a.name, a.reads);
  const fromOther = decided && decided.rule !== a.rule ? decided : null;
  const fromMode = !decided && !a.reads && perms.mode !== "open";
  // Rules over the whole app are said once at the top of the page.
  const conditions = a.conditional ? conditionalRules(perms, app.name, a.name).filter((c) => !coversWholeApp(c.rule)) : [];
  const shown: Level = a.level === "quiet" ? "allow" : a.level;
  const label = toolLabel(a.name);
  return (
    <div className="apps-action">
      <div className="apps-action__text">
        <p className="apps-action__title">
          {label}
          <span className="apps-action__rule">{a.rule}</span>
          {a.level === "quiet" && (
            <Badge size="sm" variant="outline" icon={<VolumeX size={12} aria-hidden />}>
              Quietly
            </Badge>
          )}
        </p>
        {a.description && (
          <p className="apps-action__desc" data-user-content>
            {a.description}
          </p>
        )}
        {fromOther && (
          <p className="apps-action__note">
            {permissionLabel(fromOther.list)} by the rule <span className="t-mono apps-code">{fromOther.rule}</span>
            {fromOther.list === "block" ? ", which wins over anything set here." : "."}
          </p>
        )}
        {fromMode && <p className="apps-action__note">{perms.mode === "ask" ? "Asks first, as every change does in Ask before changes." : "Blocked, as every change is in Read only."}</p>}
        {conditions.length > 0 && (
          <p className="apps-action__note">
            A rule decides some calls:{" "}
            {conditions.map((c, i) => (
              <span key={c.rule}>
                {i > 0 && ", "}
                <span className="t-mono apps-code">{c.rule}</span> {listVerb[c.list]}
              </span>
            ))}
            .
          </p>
        )}
      </div>
      <div className={a.reads ? "apps-action__control apps-action__control--read" : "apps-action__control"}>
        <SegmentedControl<Level>
          size="sm"
          aria-label={`What Ghost may do with ${label}`}
          value={shown}
          disabled={disabled}
          options={a.reads ? READ_OPTIONS : CHANGE_OPTIONS}
          onChange={onPick}
        />
        {!a.reads && (
          <Menu
            align="end"
            trigger={<IconButton size="sm" icon={<MoreHorizontal />} label={`More for ${label}`} loading={saving} disabled={disabled} />}
            items={[
              a.level === "quiet"
                ? { label: "Say when it runs", icon: <Megaphone />, onSelect: () => onPick("allow") }
                : { label: "Run it quietly", icon: <VolumeX />, onSelect: () => onPick("quiet") },
            ]}
          />
        )}
      </div>
    </div>
  );
}

function DetailSkeleton() {
  return (
    <div className="stack gap-6" aria-busy="true" aria-label="Loading">
      <Skeleton height={64} radius="var(--radius-lg)" />
      <Skeleton width={160} height={16} />
      <Card list>
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="apps-action">
            <div className="apps-action__text">
              <SkeletonText lines={2} />
            </div>
            <Skeleton width={200} height={28} radius="var(--radius-md)" />
          </div>
        ))}
      </Card>
    </div>
  );
}

