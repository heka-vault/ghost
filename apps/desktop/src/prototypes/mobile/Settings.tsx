/**
 * Settings for this phone: the desk it's paired with, how a session behaves in the car, and
 * the one privacy fact that matters (the mic opens only after Start). Rows are 56dp and the
 * whole row is the control.
 */
import { Lock, Monitor } from "lucide-react";
import { type ReactNode, useRef, useState } from "react";

import type { MobileFixture } from "../../design/mock/fixtures/mobile";
import { navigate } from "../../app/router";
import { Dialog, relativeTime } from "../../ui";
import { CompactBar, LinkDot, linkLabel, Scroller, TabBar, TouchButton, useLink } from "./parts";
import { setPref, update, type PhoneState } from "./store";

export function Settings({ phone, pairing, version }: { phone: PhoneState; pairing: MobileFixture["pairing"]; version: string }) {
  const link = useLink();
  const { tone, title } = linkLabel(link);
  const [asking, setAsking] = useState(false);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const p = phone.prefs;

  const unpair = () => {
    setAsking(false);
    update({ paired: false });
    navigate("/mobile/pair");
  };
  const reached = pairing.lastSeen ? relativeTime(pairing.lastSeen) : null;

  return (
    <div className="mob-screen mob-settings">
      <CompactBar title="Settings" watch={titleRef} />
      <Scroller className="mob-scroll--pad">
        <header className="mob-largehead">
          <h1 className="mob-largehead__title" ref={titleRef}>Settings</h1>
        </header>

        <Group title="Your desk">
          <div className="mob-row">
            <span className="mob-row__hit mob-row__hit--top is-static">
              <span className="mob-row__leading"><Monitor size={20} aria-hidden /></span>
              <span className="mob-row__text">
                <span className="mob-row__title mob-nowrap">{pairing.workstation}</span>
                <span className="mob-row__support mob-linkstate"><LinkDot tone={tone} />{title}</span>
                <span className="mob-row__support">
                  <span className="t-mono">{pairing.address}</span> · {pairing.via}
                </span>
                <span className="mob-row__support">
                  Paired {relativeTime(pairing.pairedAt)}
                  {reached ? <> · reached {reached === "now" ? "just now" : reached}</> : null}
                </span>
              </span>
            </span>
          </div>
        </Group>

        <Group title="During a session">
          <ToggleRow title="Captions" support="Show what Ghost says on screen. Off keeps the screen calm while you drive." checked={p.captions} onChange={(v) => setPref("captions", v)} />
          <ToggleRow title="Keep the screen on" support="Until you tap End." checked={p.keepAwake} onChange={(v) => setPref("keepAwake", v)} />
          <ToggleRow title="Buzz when Ghost is listening" support="A short vibration each time it’s your turn." checked={p.haptics} onChange={(v) => setPref("haptics", v)} />
        </Group>

        <Group title="Buttons for Ghost’s questions" description="Ghost always asks out loud and you can always answer by voice.">
          <RadioRow name="mob-buttons" title="Only when parked" support="Hidden while the phone thinks you’re driving." checked={p.decisionButtons === "parked"} onChange={() => setPref("decisionButtons", "parked")} />
          <RadioRow name="mob-buttons" title="Always" support="Confirm and Cancel show for every question." checked={p.decisionButtons === "always"} onChange={() => setPref("decisionButtons", "always")} />
        </Group>

        <Group title="Microphone">
          <div className="mob-row">
            <span className="mob-row__hit mob-row__hit--top is-static">
              <span className="mob-row__leading"><Lock size={20} aria-hidden /></span>
              <span className="mob-row__text">
                <span className="mob-row__title">Only after you tap Start</span>
                <span className="mob-row__support">The mic closes when you tap End. This phone never listens in the background.</span>
              </span>
            </span>
          </div>
        </Group>

        <div className="mob-settings__foot">
          <TouchButton variant="danger-ghost" fullWidth onClick={() => setAsking(true)}>Unpair this phone</TouchButton>
          <p className="mob-quiet">Ghost for Android {version} · prototype</p>
        </div>
      </Scroller>
      <TabBar current="settings" />
      {/* Like ConfirmDialog (Cancel first, so Enter never unpairs by accident), with 48dp buttons. */}
      <Dialog
        open={asking}
        onClose={() => setAsking(false)}
        hideClose
        title="Unpair this phone?"
        description={<>It won’t be able to start sessions on <span className="mob-nowrap">{pairing.workstation}</span> until you pair it again with a new code.</>}
        footer={
          <div className="mob-dialog-actions">
            <TouchButton variant="secondary" fullWidth onClick={() => setAsking(false)}>Cancel</TouchButton>
            <TouchButton variant="danger" fullWidth onClick={unpair}>Unpair this phone</TouchButton>
          </div>
        }
        className="mob-dialog"
      />
    </div>
  );
}

function Group({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return (
    <section className="mob-group">
      <h2 className="mob-group__title">{title}</h2>
      {description && <p className="mob-group__desc">{description}</p>}
      <div className="mob-list mob-list--card">{children}</div>
    </section>
  );
}

/** The whole row is the switch: a native checkbox with role="switch", 56dp tall. */
function ToggleRow({ title, support, checked, onChange }: { title: string; support?: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="mob-row mob-row--control">
      <span className="mob-row__text">
        <span className="mob-row__title">{title}</span>
        {support && <span className="mob-row__support">{support}</span>}
      </span>
      <input type="checkbox" role="switch" className="mob-switch" checked={checked} onChange={(e) => onChange(e.target.checked)} />
    </label>
  );
}

function RadioRow({ name, title, support, checked, onChange }: { name: string; title: string; support?: string; checked: boolean; onChange: () => void }) {
  return (
    <label className="mob-row mob-row--control">
      <input type="radio" name={name} className="mob-radio" checked={checked} onChange={onChange} />
      <span className="mob-row__text">
        <span className="mob-row__title">{title}</span>
        {support && <span className="mob-row__support">{support}</span>}
      </span>
    </label>
  );
}
