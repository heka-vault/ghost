/**
 * Owner: settings. Which waveform shows Ghost speaking in this app: a test of three designs
 * (ui/VoiceWave.tsx), each previewed here with a stand-in voice. A preference of this
 * computer's app, not a setting of Ghost's: it applies at once, with no Save.
 */
import { RadioGroup, Section, VoiceWave, WAVE_VARIANTS, setWaveVariant, useWaveVariant } from "../../ui";
import { SECTION_TITLE, sectionDomId } from "./model";

export function WavePicker() {
  const value = useWaveVariant();
  return (
    <Section
      id={sectionDomId("wave")}
      title={SECTION_TITLE.wave}
      description="How this app draws Ghost’s voice while it talks. It follows what Ghost actually says. Changes at once."
      className="set-section"
    >
      <RadioGroup
        aria-label={SECTION_TITLE.wave}
        variant="card"
        columns={3}
        value={value}
        onChange={setWaveVariant}
        options={WAVE_VARIANTS.map((v) => ({
          value: v.value,
          label: v.label,
          description: (
            <>
              <span className="set-wave__preview">
                <VoiceWave active simulate variant={v.value} width={148} height={40} />
              </span>
              {v.description}
            </>
          ),
        }))}
      />
    </Section>
  );
}
