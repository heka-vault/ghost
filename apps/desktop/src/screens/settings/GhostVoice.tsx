/**
 * Owner: settings. Ghost's voice on Sound: the same voice as on Ghost's own agent page, kept
 * on Ghost's agent (ADR 0014), heard with a sample and chosen in the agents' voice picker.
 * One store, two ways in. The page keeps the choice in its draft and Save writes it.
 */
import { useGhost } from "@ghost/ui";
import { Play } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { PlayingBars, VoicePicker, voiceTitle } from "../conversations/VoicePicker";
import { type Voice, parseVoice, voiceKind } from "../conversations/voices";
import { Button, IconButton, ListRow, errorText, useLive, useToast } from "../../ui";

export function GhostVoice({ value, speed, onChange }: { value: string; speed: number; onChange: (id: string) => void }) {
  const { client, status } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [voices, setVoices] = useState<Voice[]>([]);
  const [picker, setPicker] = useState(false);
  const [hearing, setHearing] = useState<string | null>(null);
  const hearingRef = useRef<string | null>(null);

  useEffect(() => {
    client
      .voices()
      .then((v) => setVoices(v.map(parseVoice)))
      .catch(() => setVoices([]));
  }, [client, status?.models_ready]);

  const voice = voices.find((v) => v.id === value) ?? parseVoice(value);
  const pickable = voices.some((v) => v.id === value) ? voices : [...voices, voice];
  const title = voiceTitle(voice, pickable);
  const hearBlocked = !online ? "Ghost isn’t answering right now." : !status?.models_ready ? "Hear it works once the voice models finish loading." : null;

  // One sample at a time; a second ask while one plays is dropped.
  const hear = async (id: string) => {
    if (hearBlocked || hearingRef.current) return;
    hearingRef.current = id;
    setHearing(id);
    try {
      const r = await client.sampleVoice(id, speed);
      if (!r.ok) toast({ title: "Couldn’t play a sample", description: "Ghost may be speaking. Try again in a moment.", tone: "danger" });
    } catch (e) {
      toast({ title: "Couldn’t play a sample", description: errorText(e), tone: "danger" });
    } finally {
      hearingRef.current = null;
      setHearing(null);
    }
  };

  return (
    <>
      <ListRow
        leading={
          <IconButton
            variant="secondary"
            size="sm"
            label={hearing === value ? `Playing ${title}` : `Hear ${title}`}
            icon={hearing === value ? <PlayingBars /> : <Play />}
            disabled={!!hearBlocked || (hearing != null && hearing !== value)}
            onClick={() => void hear(value)}
          />
        }
        title="Ghost’s voice"
        description={
          <span aria-live="polite">
            {hearing === value && !picker ? (
              "Playing on your speakers…"
            ) : (
              <>
                {title} · {voiceKind(voice)} <span className="set-detail">{value}</span>
              </>
            )}
          </span>
        }
        trailing={
          <Button size="sm" variant="secondary" disabled={!online} onClick={() => setPicker(true)}>
            Change voice
          </Button>
        }
      />
      <VoicePicker
        open={picker}
        onClose={() => setPicker(false)}
        lineName="Ghost"
        voices={pickable}
        value={value}
        onPick={onChange}
        hearBlocked={hearBlocked}
        hearing={hearing}
        onHear={(id) => void hear(id)}
      />
    </>
  );
}
