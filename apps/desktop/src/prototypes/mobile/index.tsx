/**
 * Owner: mobile. PROTOTYPE: Ghost's Android phone app (Expo later), at #/mobile/:screen.
 * Rendered without the app shell. At phone widths the phone fills the viewport; on a desktop
 * window it sits in a device frame beside an index of every screen and state.
 *
 * Live state (connection, phase, pending, audit) comes from the mock daemon via useGhost()
 * and useLive(), so Start, End, Confirm, Cancel and Undo really run. Pairing and the phone's
 * own preferences have no API yet; they come from useFixture("mobile") and ./store.
 * Screenshot with --mobile (390x844). States: see the index, or docs in mobile.css's header.
 */
import { Smartphone } from "lucide-react";
import { type CSSProperties, useEffect, useState } from "react";

import { hrefFor, navigate } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { scenarioHref, useDesign, useFixture } from "../../design/context";
import type { MobileFixture, MobileScreenId, MobileStateLink } from "../../design/mock/fixtures/mobile";
import { Badge, Button, EmptyState, SegmentedControl, cx } from "../../ui";
import { Activity } from "./Activity";
import { Home } from "./Home";
import { PairCode, Paired, PairScan } from "./Pair";
import { GestureBar, StatusBar } from "./parts";
import { Session } from "./Session";
import { Settings } from "./Settings";
import { seed, setPref, update, usePhone } from "./store";
import { Summary } from "./Summary";
import "./mobile.css";

const PHONE_W = 390;
const PHONE_H = 844;

export default function MobilePrototype({ params }: RouteProps) {
  const data = useFixture<MobileFixture>("mobile");
  if (data) seed(data);
  const phone = usePhone();
  const design = useDesign();
  const framed = useFramed();

  if (!data || !phone) {
    return (
      <div className="mob-nomock">
        <EmptyState
          size="page"
          icon={<Smartphone />}
          title="The phone app is a prototype"
          description="It runs on mock data only, so it never starts a session on your real workstation."
          actions={<Button variant="primary" href={scenarioHref("passive", "/mobile")}>Open it with mock data</Button>}
        />
      </div>
    );
  }

  const ids = data.screens.map((s) => s.id);
  // A screen may carry its own query inside the hash: #/mobile/activity?open=<id>.
  const [rawScreen, rawQuery] = (params.screen ?? "").split("?");
  const screenQuery = new URLSearchParams(rawQuery ?? "");
  // “audit” is the desktop's name for the same screen; older links keep working.
  const asked = ((rawScreen === "audit" ? "activity" : rawScreen) || (phone.paired ? "home" : "pair")) as MobileScreenId;
  const known = ids.includes(asked);
  const screen = known ? asked : "home";

  return (
    <div className={cx("mob-stage", framed && "is-framed")}>
      {framed && <StageIndex data={data} current={screen} scenario={design?.scenario.name ?? "passive"} phone={phone} />}
      <div className="mob-device" style={framed && framed.zoom !== 1 ? ({ zoom: framed.zoom } as CSSProperties) : undefined}>
        <div className="mob-phone" data-screen={screen}>
          <StatusBar />
          <div className="mob-viewport">
            <PhoneScreen id={screen} data={data} phone={phone} query={screenQuery} />
          </div>
          <GestureBar />
        </div>
      </div>
    </div>
  );
}

function PhoneScreen({ id, data, phone, query }: { id: MobileScreenId; data: MobileFixture; phone: NonNullable<ReturnType<typeof usePhone>>; query: URLSearchParams }) {
  // An unpaired phone can only pair. Tab screens bounce to pairing (replace, so Back works).
  const needsPair = !phone.paired && !["pair", "pair-code", "paired"].includes(id);
  useEffect(() => {
    if (needsPair) navigate("/mobile/pair", { replace: true });
  }, [needsPair]);
  if (needsPair) return null;
  const p = data.pairing;
  switch (id) {
    case "pair": return <PairScan pairing={p} />;
    case "pair-code": return <PairCode pairing={p} />;
    case "paired": return <Paired pairing={p} />;
    case "home": return <Home workstation={p.workstation} via={p.via} cached={data.cached} />;
    case "session": return <Session phone={phone} mode="session" />;
    case "decision": return <Session phone={phone} mode="decision" />;
    case "reconnecting": return <Session phone={phone} mode="reconnecting" />;
    case "summary": return <Summary phone={phone} sample={data.sampleSummary} />;
    case "activity": return <Activity key={query.get("open") ?? ""} openId={query.get("open")} cached={data.cached} />;
    case "settings": return <Settings phone={phone} pairing={p} version={data.appVersion} />;
  }
}

/** On a desktop window: frame the phone and scale it to fit the height. At phone widths: fill. */
function useFramed(): false | { zoom: number } {
  const read = () => {
    if (window.innerWidth < 600) return false as const;
    const zoom = Math.min(1, (window.innerHeight - 48) / PHONE_H);
    return { zoom: Math.round(zoom * 1000) / 1000 };
  };
  const [v, setV] = useState(read);
  useEffect(() => {
    const on = () => setV(read());
    window.addEventListener("resize", on);
    return () => window.removeEventListener("resize", on);
  }, []);
  return v;
}

/** Desktop-only: every screen and state, one click away. Prototype controls live here, not on the phone. */
function StageIndex({ data, current, scenario, phone }: { data: MobileFixture; current: MobileScreenId; scenario: string; phone: NonNullable<ReturnType<typeof usePhone>> }) {
  const here = `${window.location.search}`;
  const href = (l: MobileStateLink) => `${window.location.pathname}?mock=${encodeURIComponent(l.scenario)}${l.query ? `&${l.query}` : ""}#${l.path}`;
  const isHere = (l: MobileStateLink) => l.scenario === scenario && here === `?mock=${encodeURIComponent(l.scenario)}${l.query ? `&${l.query}` : ""}` && l.path === `/mobile/${current}`;
  return (
    <aside className="mob-index" aria-label="Phone prototype">
      <div className="mob-index__head">
        <Badge tone="accent" size="sm">Prototype</Badge>
        <p className="mob-index__title">Ghost for Android</p>
        <p className="mob-index__sub">{PHONE_W} by {PHONE_H}. Mock data only; Start, End and Confirm run against the mock Ghost.</p>
      </div>
      <nav aria-label="Phone screens">
        <ul className="mob-index__list">
          {data.screens.map((s) => (
            <li key={s.id}>
              <a href={hrefFor(`/mobile/${s.id}`)} aria-current={s.id === current ? "page" : undefined}>
                <span>{s.title}</span>
                <span className="mob-index__note">{s.note}</span>
              </a>
            </li>
          ))}
        </ul>
      </nav>
      <div className="mob-index__group">
        <p className="mob-index__label">The phone thinks you’re</p>
        <SegmentedControl
          aria-label="The phone thinks you’re"
          fullWidth
          value={phone.motion}
          onChange={(v) => update({ motion: v })}
          options={[{ value: "driving", label: "Driving" }, { value: "parked", label: "Parked" }]}
        />
        <SegmentedControl
          aria-label="Captions"
          fullWidth
          value={phone.prefs.captions ? "on" : "off"}
          onChange={(v) => setPref("captions", v === "on")}
          options={[{ value: "off", label: "Captions off" }, { value: "on", label: "Captions on" }]}
        />
      </div>
      {data.states.map((g) => (
        <div key={g.group} className="mob-index__group">
          <p className="mob-index__label">{g.group}</p>
          <ul className="mob-index__states">
            {g.items.map((l) => (
              <li key={l.label}>
                <a href={href(l)} aria-current={isHere(l) ? "true" : undefined}>{l.label}</a>
              </li>
            ))}
          </ul>
        </div>
      ))}
      <p className="mob-index__note-block">Mute and Open settings are phone-only prototypes: mic routing and Android settings have no API yet.</p>
    </aside>
  );
}
