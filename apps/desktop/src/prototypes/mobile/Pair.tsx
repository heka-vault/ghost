/**
 * Pairing, before anything else works: scan the code your desk shows, or type its address
 * and the six-digit code. The camera is the only sensor this screen uses; the mic stays off.
 */
import { CameraOff, Check, Hand, Lock, ScrollText } from "lucide-react";
import { type FormEvent, useState } from "react";

import type { MobileFixture } from "../../design/mock/fixtures/mobile";
import { hrefFor, navigate } from "../../app/router";
import { Badge, Field, GhostMark, Notice } from "../../ui";
import { Scroller, TopBar, TouchButton, TouchInput } from "./parts";
import { update } from "./store";

export function PairScan({ pairing }: { pairing: MobileFixture["pairing"] }) {
  const [scanning, setScanning] = useState(false);
  // ?camera=denied: Android said no (or there is no camera). Typing the address becomes the way in.
  const denied = new URLSearchParams(window.location.search).get("camera") === "denied";
  if (denied) return <PairNoCamera />;
  return (
    <div className="mob-screen mob-pair">
      <Scroller className="mob-scroll--pad">
        <header className="mob-pair__head">
          <GhostMark size={40} />
          <h1 className="mob-pair__title">Pair with your desk</h1>
          <p className="mob-pair__body">On your workstation, open Ghost, go to Settings, and choose Pair a phone. Point the camera at the code it shows.</p>
        </header>

        {/* The camera feed is dark in both themes. In the prototype, tapping it stands in for a scan. */}
        <button
          type="button"
          className="mob-finder"
          data-theme="dark"
          aria-label="Camera. Prototype: tap to simulate scanning the code"
          onClick={() => {
            setScanning(true);
            window.setTimeout(() => {
              update({ paired: true });
              navigate("/mobile/paired");
            }, 900);
          }}
        >
          <span className="mob-finder__corners" aria-hidden><i /><i /><i /><i /></span>
          <span className="mob-finder__hint">
            {scanning ? <>Pairing with {pairing.workstation}…</> : <>Tap to simulate a scan <Badge tone="accent" size="sm">Prototype</Badge></>}
          </span>
        </button>
      </Scroller>
      <div className="mob-cta">
        <TouchButton size="lg" fullWidth href={hrefFor("/mobile/pair-code")}>Enter the address instead</TouchButton>
      </div>
    </div>
  );
}

export function PairCode({ pairing }: { pairing: MobileFixture["pairing"] }) {
  // ?pair=fail: the code didn't match. ?pair=unreachable: nothing answered at that address.
  const preset = new URLSearchParams(window.location.search).get("pair");
  const [address, setAddress] = useState(preset ? pairing.address : "");
  const [code, setCode] = useState(preset ? "482 913" : "");
  const [busy, setBusy] = useState(false);
  const [codeError, setCodeError] = useState<string | null>(preset === "fail" ? MISMATCH : null);
  const [netError, setNetError] = useState<string | null>(preset === "unreachable" ? unreachable(pairing.address) : null);
  const digits = code.replace(/\D/g, "");
  const ready = address.trim().length > 0 && digits.length === 6;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!ready || busy) return;
    setBusy(true);
    setCodeError(null);
    setNetError(null);
    // Prototype: 000000 shows the code failure, an address starting 10.0.0. shows the network one; anything else pairs.
    window.setTimeout(() => {
      setBusy(false);
      if (address.trim().startsWith("10.0.0.")) return setNetError(unreachable(address.trim()));
      if (digits === "000000") return setCodeError(MISMATCH);
      update({ paired: true });
      navigate("/mobile/paired");
    }, 1100);
  };

  return (
    <form className="mob-screen mob-pair" onSubmit={submit} noValidate>
      <TopBar title="Enter the address" back="/mobile/pair" />
      <Scroller className="mob-scroll--pad mob-form">
        <p className="mob-pair__body">Both are under Settings, Pair a phone on your workstation. Works on the same Wi-Fi or over Tailscale.</p>
        {netError && <Notice tone="danger" title="Couldn’t reach your desk">{netError}</Notice>}
        <Field className="mob-field" label="Workstation address">
          <TouchInput mono value={address} onChange={(e) => setAddress(e.target.value)} placeholder={pairing.address} autoComplete="off" autoCapitalize="off" spellCheck={false} inputMode="url" />
        </Field>
        <Field className="mob-field" label="Pairing code" help="Six digits. A new one appears every few minutes." error={codeError ?? undefined}>
          <TouchInput
            mono
            value={code}
            onChange={(e) => {
              setCode(groupCode(e.target.value));
              if (codeError) setCodeError(null);
            }}
            placeholder="482 913"
            autoComplete="one-time-code"
            inputMode="numeric"
          />
        </Field>
      </Scroller>
      <div className="mob-cta">
        <TouchButton type="submit" variant="primary" size="xl" fullWidth loading={busy} disabled={!ready}>
          Pair
        </TouchButton>
      </div>
    </form>
  );
}

const MISMATCH = "That code didn’t match. Show a new code on your desk and type it again.";
const unreachable = (address: string) => `Nothing answered at ${address}. Check that Ghost is running and that this phone is on the same Wi-Fi or on Tailscale.`;

/** Six digits as 3+3, the way the desk shows them: “482913” → “482 913”. */
function groupCode(raw: string): string {
  const d = raw.replace(/\D/g, "").slice(0, 6);
  return d.length > 3 ? `${d.slice(0, 3)} ${d.slice(3)}` : d;
}

export function Paired({ pairing }: { pairing: MobileFixture["pairing"] }) {
  return (
    <div className="mob-screen mob-pair">
      <Scroller className="mob-scroll--pad">
        <header className="mob-pair__head">
          <span className="mob-pair__ok" aria-hidden><Check size={28} /></span>
          <h1 className="mob-pair__title">Paired</h1>
          <p className="mob-pair__with t-mono">{pairing.workstation}</p>
          <p className="mob-pair__body">This phone can start sessions with Ghost and answer its questions.</p>
        </header>
        <ul className="mob-facts">
          <li><Lock size={20} aria-hidden /><span><strong>The mic opens only after you tap Start.</strong> It closes when you tap End. Nothing listens in the background.</span></li>
          <li><Hand size={20} aria-hidden /><span><strong>Ghost asks out loud before it changes anything you’ve set to ask first.</strong> Buttons show when you’re parked.</span></li>
          <li><ScrollText size={20} aria-hidden /><span><strong>Everything Ghost does is in Activity,</strong> on this phone and at your desk.</span></li>
        </ul>
      </Scroller>
      <div className="mob-cta">
        <TouchButton variant="primary" size="xl" fullWidth href={hrefFor("/mobile/home")}>Start using Ghost</TouchButton>
      </div>
    </div>
  );
}

/** No camera, or the operator said no to it: say so on the dark viewfinder and make typing primary. */
function PairNoCamera() {
  return (
    <div className="mob-screen mob-pair">
      <Scroller className="mob-scroll--pad">
        <header className="mob-pair__head">
          <GhostMark size={40} />
          <h1 className="mob-pair__title">Pair with your desk</h1>
          <p className="mob-pair__body">Type the address and code your workstation shows under Settings, Pair a phone.</p>
        </header>
        <div className="mob-finder is-off" data-theme="dark" role="group" aria-labelledby="mob-nocam">
          <span className="mob-finder__off">
            <CameraOff size={32} aria-hidden />
            <span id="mob-nocam" className="mob-finder__offtitle">Ghost can’t use the camera</span>
            <span className="mob-finder__offbody">Allow it in Android settings to scan the code, or type it instead.</span>
            {/* On the phone this opens Android's app settings; the prototype has nowhere to go. */}
            <button type="button" className="mob-finder__settings" aria-describedby="mob-nocam-proto">
              Open settings
            </button>
            <span id="mob-nocam-proto"><Badge tone="accent" size="sm">Prototype</Badge></span>
          </span>
        </div>
      </Scroller>
      <div className="mob-cta">
        <TouchButton variant="primary" size="xl" fullWidth href={hrefFor("/mobile/pair-code")}>Enter the address instead</TouchButton>
      </div>
    </div>
  );
}
