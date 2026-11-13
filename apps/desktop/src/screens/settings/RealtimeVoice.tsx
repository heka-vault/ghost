/**
 * Owner: settings. The voice mode on Sound (ADR 0062): the voice on this machine, the default,
 * or a realtime voice in front of Ghost's agent. Turning realtime on shows what changes and
 * asks first; going back never asks. The provider's settings, the Test and the day's spend
 * appear once it is chosen. Each change saves at once, like the waveform above: it is live
 * from the next session, and the key stays a password in Ghost's vault.
 */
import type { RealtimeView, SettingsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useCallback, useEffect, useState } from "react";

import { navigate } from "../../app/router";
import { Badge, Button, Card, ConfirmDialog, Field, Input, ListRow, Notice, RadioGroup, Section, Select, errorText, useLive, useToast } from "../../ui";
import { Row } from "./parts";
import {
  SPEND_NOTE,
  type VoiceMode,
  checkLine,
  checkTone,
  modeValues,
  needsConsent,
  parseCap,
  providerOf,
  spendLine,
} from "./realtime";

export function RealtimeVoiceSection({ onSettings }: { onSettings?: (s: SettingsView) => void }) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [view, setView] = useState<RealtimeView | null>(null);
  const [asking, setAsking] = useState(false);
  const [testing, setTesting] = useState(false);
  const [cap, setCap] = useState("");
  const [capError, setCapError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const v = await client.realtimeVoice();
      setView(v);
      setCap(v.daily_cap_usd != null ? String(v.daily_cap_usd) : "");
    } catch {
      setView(null);
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async (values: Record<string, string | null>) => {
    try {
      const r = await client.updateSettings({ values });
      onSettings?.(r.settings);
      await load();
      return true;
    } catch (e) {
      toast({ title: "Couldn’t save the voice", description: errorText(e), tone: "danger" });
      return false;
    }
  };

  if (!view) return null;
  const mode = view.mode as VoiceMode;
  const provider = providerOf(view);

  const choose = (next: VoiceMode) => {
    if (next === mode) return;
    if (needsConsent(mode, next)) setAsking(true);
    else void save(modeValues(next));
  };

  const test = async () => {
    setTesting(true);
    try {
      await client.checkRealtimeVoice();
      await load();
    } catch (e) {
      toast({ title: "Couldn’t test the realtime voice", description: errorText(e), tone: "danger" });
    } finally {
      setTesting(false);
    }
  };

  const saveCap = () => {
    const p = parseCap(cap);
    if ("error" in p) {
      setCapError(p.error);
      return;
    }
    setCapError(null);
    const was = view.daily_cap_usd != null ? String(view.daily_cap_usd) : null;
    if (p.value !== was) void save({ GHOST_REALTIME_DAILY_CAP_USD: p.value });
  };

  return (
    <Section
      id="set-voice-mode"
      title="Voice mode"
      description="Who hears you and speaks for Ghost during a session. Ghost’s agent does the work either way."
      className="set-section"
    >
      <RadioGroup<VoiceMode>
        aria-label="Voice mode"
        variant="card"
        columns={2}
        value={mode}
        disabled={!online}
        onChange={choose}
        options={[
          {
            value: "local",
            label: "On this machine",
            description: "Parakeet hears you, your model connection answers, Kokoro speaks. Your voice never leaves this computer.",
            aside: <Badge size="sm">Default</Badge>,
          },
          {
            value: "realtime",
            label: "Realtime",
            description: `${provider.title} hears you and answers within a moment, and hands every real request to Ghost’s agent. Your voice goes to ${provider.title} during a session.`,
          },
        ]}
      />
      {view.problem && <Notice tone="danger">{view.problem}</Notice>}
      {mode === "realtime" && (
        <Card list>
          <Row>
            <Field layout="inline" id="rt-model" label="Model" detail="GHOST_REALTIME_MODEL" help="The mini model is cheaper and quick; the full one reads more carefully.">
              <Select
                id="rt-model"
                value={view.model}
                disabled={!online}
                onChange={(e) => void save({ GHOST_REALTIME_MODEL: e.target.value === provider.default_model ? null : e.target.value })}
                options={provider.models.map((m) => ({ value: m.id, label: m.title }))}
              />
            </Field>
          </Row>
          <Row>
            <Field layout="inline" id="rt-voice" label="Its voice" detail="GHOST_REALTIME_VOICE" help="An agent’s own voice applies on this machine only.">
              <Select
                id="rt-voice"
                value={view.voice}
                disabled={!online}
                onChange={(e) => void save({ GHOST_REALTIME_VOICE: e.target.value === provider.default_voice ? null : e.target.value })}
                options={provider.voices.map((v) => ({ value: v, label: v[0].toUpperCase() + v.slice(1) }))}
              />
            </Field>
          </Row>
          <ListRow
            title="Password"
            description={
              <>
                {view.password_set ? "In Ghost’s vault." : "Not in Ghost’s vault yet: sessions use the voice on this machine until it is."}{" "}
                <span className="set-detail">{view.password}</span>
              </>
            }
            meta={view.password_set ? <Badge tone="ok" size="sm" dot>Set</Badge> : <Badge tone="warn" size="sm" dot>Not set</Badge>}
            trailing={
              <Button size="sm" variant="secondary" onClick={() => navigate("/settings/passwords")}>
                {view.password_set ? "Passwords" : "Add it"}
              </Button>
            }
          />
          <Row>
            <Field
              layout="inline"
              id="rt-cap"
              label="Daily cap"
              detail="GHOST_REALTIME_DAILY_CAP_USD"
              help={capError ?? "After this much in a day, Ghost uses the voice on this machine until tomorrow. Empty: no cap."}
              error={capError ?? undefined}
            >
              <Input
                id="rt-cap"
                inputMode="decimal"
                placeholder="No cap"
                leading="$"
                value={cap}
                disabled={!online}
                onChange={(e) => setCap(e.target.value)}
                onBlur={saveCap}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    saveCap();
                  }
                }}
              />
            </Field>
          </Row>
          <ListRow title="Spent" description={<span title={SPEND_NOTE(view.spend)}>{spendLine(view.spend)}</span>} />
          <ListRow
            title="Test"
            description={
              <span aria-live="polite">
                {testing ? "Connecting…" : view.last_check ? checkLine(view.last_check) : "Connects, says one line and hands one request on. A fraction of a cent."}
              </span>
            }
            meta={view.last_check && !testing ? <Badge tone={checkTone(view.last_check)} size="sm" dot>{view.last_check.reachable ? "Works" : "Failed"}</Badge> : undefined}
            trailing={
              <Button size="sm" variant="secondary" disabled={!online || testing} onClick={() => void test()}>
                Test
              </Button>
            }
          />
        </Card>
      )}
      <ConfirmDialog
        open={asking}
        title="Use the realtime voice?"
        description={
          <span className="set-disclosure">
            {provider.disclosure.map((line) => (
              <span key={line} className="set-disclosure__line">
                {line}
              </span>
            ))}
          </span>
        }
        confirmLabel="Use the realtime voice"
        cancelLabel="Keep this machine’s voice"
        onCancel={() => setAsking(false)}
        onConfirm={async () => {
          await save(modeValues("realtime"));
          setAsking(false);
        }}
      />
    </Section>
  );
}
