/**
 * Owner: foundation. Every primitive in every state, in both themes side by side, plus an
 * index of every route in every mock scenario. Development builds only.
 *
 * States marked hover, pressed and focus are forced with the primitives' .is-hover,
 * .is-active and .is-focus classes, which exist for this page and screenshots only.
 */
import {
  ArrowUp,
  ArrowUpRight,
  Bell,
  CircleCheck,
  Copy,
  Ear,
  Eraser,
  Hand,
  Inbox,
  Megaphone,
  Mic,
  Moon,
  MoreHorizontal,
  Pencil,
  Pin,
  Plus,
  Search,
  Settings2,
  ShieldOff,
  RotateCcw,
  Trash2,
  VolumeX,
} from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";

import { ROUTES } from "../../app/routes";
import { scenarioHref, useDesign } from "../../design/context";
import {
  Badge,
  Button,
  ButtonSizeContext,
  Card,
  Checkbox,
  ConfirmDialog,
  Dialog,
  Disclosure,
  EmptyState,
  Field,
  GhostGlyph,
  GhostMark,
  Mascot,
  type GhostPhase,
  IconButton,
  Input,
  Kbd,
  LineGlyph,
  ListRow,
  Menu,
  Notice,
  Page,
  PageHeader,
  PendingChange,
  Progress,
  RadioGroup,
  SaveBar,
  SegmentedControl,
  Select,
  Skeleton,
  SkeletonText,
  Slider,
  Spinner,
  StatusDot,
  Switch,
  Table,
  Tabs,
  Textarea,
  Tooltip,
  VoicePresence,
  cx,
  formatShortcut,
  liveLabel,
  phaseText,
  quoted,
  statusLabel,
  statusTone,
  useToast,
} from "../../ui";
import { StatusCard } from "../../app/Sidebar";
import "./gallery.css";

const SCENARIO_FALLBACK = ["passive", "session", "pending", "speaking", "thinking", "starting", "disconnected", "firstrun", "empty", "expiring", "reconnecting", "busy"];
const PHASES: GhostPhase[] = ["starting", "passive", "listening", "thinking", "speaking", "offline"];
const SECTIONS = ["Checks", "Routes", "Color", "Type", "Space", "Buttons", "Inputs", "Choices", "Menus", "Voice", "Status", "Pending", "Surfaces", "Lists", "Feedback", "Overlays", "Brand"];
type Autonomy = "confirm" | "announce" | "quiet" | "deny";

export default function Gallery() {
  const design = useDesign();
  const [layout, setLayout] = useState<"both" | "dark" | "light">("both");
  const scenarios = design?.scenarios.map((s) => s.name) ?? SCENARIO_FALLBACK;

  return (
    <Page
      width="wide"
      title="Gallery"
      className="gal-page"
      description={<>Every primitive in every state, in both themes. The contract is <code>docs/design-system.md</code>.</>}
      actions={<SegmentedControl aria-label="Themes shown" value={layout} onChange={setLayout} options={[{ value: "both", label: "Both" }, { value: "dark", label: "Dark" }, { value: "light", label: "Light" }]} />}
    >
      <div className="gal-wrap">

        <nav className="gal-toc" aria-label="Gallery sections">
          {SECTIONS.map((s) => (
            <a
              key={s}
              href={`#gal-${s.toLowerCase()}`}
              onClick={(e) => {
                e.preventDefault();
                document.getElementById(`gal-${s.toLowerCase()}`)?.scrollIntoView({ behavior: "smooth" });
              }}
            >
              {s}
            </a>
          ))}
        </nav>

        <section className="gal-block" id="gal-checks">
          <h2 className="gal-h2">Checks</h2>
          <p className="gal-lead">Contracts this page asserts on every load. A failure turns its row red and logs a console error, which the screenshot tool reports as a page error.</p>
          <Card list>
            <CheckRow title="Keyboard" description="Enter on a menu trigger opens on the first item; Escape returns focus.">
              <MenuCheck />
            </CheckRow>
            <CheckRow title="Contrast, dark theme" description="Field edges 3:1 at rest and hovered; white on every fill 4.5:1.">
              <ContrastCheck theme="dark" />
            </CheckRow>
            <CheckRow title="Contrast, light theme" description="The same measurements in the light theme.">
              <ContrastCheck theme="light" />
            </CheckRow>
            <CheckRow title="Typography" description="Curly apostrophes and a real ellipsis (…) in copy, placeholders and labels.">
              <TypographyCheck />
            </CheckRow>
            <CheckRow title="Headings" description="Exactly one h1 on the page. The shell checks every route the same way.">
              <HeadingCheck />
            </CheckRow>
          </Card>
        </section>

        <section className="gal-block" id="gal-routes">
          <h2 className="gal-h2">Routes × scenarios</h2>
          <p className="gal-lead">
            Open any screen against any mock scenario. Current: <strong>{design?.scenario.label ?? "real daemon"}</strong>. Any scenario can drop its connection with <code>&amp;drop=2000</code>, or hold its first connection with <code>&amp;connect=600</code> to see Connecting.
          </p>
          <Card list>
            <Table density="compact">
              <thead>
                <tr>
                  <th>Route</th>
                  <th className="is-shrink">Owner</th>
                  <th>Scenarios</th>
                </tr>
              </thead>
              <tbody>
                {ROUTES.map((r) => {
                  const path = r.id === "conversations" ? "/conversations/accounting" : r.id === "mobile" ? "/mobile/home" : r.path;
                  return (
                    <tr key={r.id}>
                      <td className="is-nowrap">
                        <div className="row gap-2">
                          <span className="t-medium">{r.title}</span>
                          {r.kind === "prototype" && <Badge tone="accent" size="sm">Prototype</Badge>}
                        </div>
                        <span className="t-mono t-3">#{r.patterns.join(" ")}</span>
                      </td>
                      <td className="is-muted is-mono is-nowrap">{r.owner}</td>
                      <td>
                        <div className="gal-chips">
                          {scenarios.map((s) => {
                            const here = s === design?.scenario.name;
                            return (
                              <a key={s} className={cx("gal-chip", here && "is-current")} href={scenarioHref(s, path)} aria-current={here ? "true" : undefined}>{s}</a>
                            );
                          })}
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </Table>
          </Card>
        </section>

        <div className={`gal-themes gal-themes--${layout}`}>
          {(layout === "both" || layout === "dark") && <ThemePanel theme="dark" />}
          {(layout === "both" || layout === "light") && <ThemePanel theme="light" />}
        </div>
      </div>
    </Page>
  );
}

function CheckRow({ title, description, children }: { title: string; description: string; children: ReactNode }) {
  return <ListRow density="compact" title={title} description={description} trailing={<span className="gal-check">{children}</span>} />;
}

function ThemePanel({ theme }: { theme: "dark" | "light" }) {
  return (
    <div className="gal-theme" data-theme={theme}>
      <div className="gal-theme__label t-overline">{theme} theme</div>
      <Specimens theme={theme} />
    </div>
  );
}

/** The live mascot: point at the row to play it, or play it as a tap. */
function MascotSpec() {
  const [hovered, setHovered] = useState(false);
  const [trigger, setTrigger] = useState(0);
  return (
    <Spec label="Mascot (point to play)" scroll>
      <span className="gal-mascot" onPointerEnter={(e) => e.pointerType === "mouse" && setHovered(true)} onPointerLeave={() => setHovered(false)}>
        <Mascot size={16} hovered={hovered} trigger={trigger} />
        <Mascot size={24} hovered={hovered} trigger={trigger} />
        <Mascot size={48} hovered={hovered} trigger={trigger} />
        <Mascot size={96} hovered={hovered} trigger={trigger} />
        <GhostMark size={40} hovered={hovered} trigger={trigger} />
      </span>
      <Button size="sm" variant="secondary" onClick={() => setTrigger((t) => t + 1)}>Play as a tap</Button>
    </Spec>
  );
}

function Block({ id, title, children }: { id?: string; title: string; children: ReactNode }) {
  return (
    <section className="gal-block" id={id}>
      <h2 className="gal-h2">{title}</h2>
      {children}
    </section>
  );
}

/** A labeled specimen. `scroll` keeps one row and scrolls sideways instead of wrapping. */
function Spec({ label, children, column, scroll }: { label: string; children: ReactNode; column?: boolean; scroll?: boolean }) {
  return (
    <div className="gal-spec">
      <div className="gal-spec__label">{label}</div>
      <div className={column ? "gal-spec__body gal-spec__body--col" : scroll ? "gal-spec__body gal-spec__body--scroll" : "gal-spec__body"}>{children}</div>
    </div>
  );
}

/** A state matrix: one row per variant, one column per state. */
function Matrix({ states, rows }: { states: string[]; rows: { label: string; cells: ReactNode[] }[] }) {
  return (
    <div className="gal-matrix" style={{ gridTemplateColumns: `96px repeat(${states.length}, minmax(0, 1fr))` }}>
      <span />
      {states.map((s) => (
        <span key={s} className="gal-matrix__head">{s}</span>
      ))}
      {rows.map((r) => (
        <MatrixRow key={r.label} label={r.label} cells={r.cells} />
      ))}
    </div>
  );
}
function MatrixRow({ label, cells }: { label: string; cells: ReactNode[] }) {
  return (
    <>
      <span className="gal-matrix__label">{label}</span>
      {cells.map((c, i) => (
        <span key={i} className="gal-matrix__cell">{c}</span>
      ))}
    </>
  );
}

function Swatch({ name, varName, text }: { name: string; varName: string; text?: boolean }) {
  return (
    <div className="gal-swatch">
      <span className="gal-swatch__chip" style={text ? { color: `var(${varName})` } : { background: `var(${varName})` }}>{text ? "Aa" : ""}</span>
      <span className="gal-swatch__text">
        <span className="gal-swatch__name">{name}</span>
        <span className="gal-swatch__var">{varName}</span>
      </span>
    </div>
  );
}

const noop = () => {};

/**
 * The loading contract, asserted: every variant and size measures the same at rest and
 * loading, with and without a leading icon. A mismatch logs a console error, which the
 * screenshot tool reports as a page error.
 */
function WidthCheck() {
  const [rows, setRows] = useState<{ name: string; rest: number; loading: number }[]>([]);
  const refs = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = refs.current;
    if (!el) return;
    const measure = () => {
      const out: { name: string; rest: number; loading: number }[] = [];
      for (const pair of Array.from(el.querySelectorAll<HTMLElement>("[data-pair]"))) {
        const [a, b] = Array.from(pair.querySelectorAll<HTMLElement>(".g-btn"));
        out.push({ name: pair.dataset.pair!, rest: a.getBoundingClientRect().width, loading: b.getBoundingClientRect().width });
      }
      setRows(out);
      const bad = out.filter((r) => Math.abs(r.rest - r.loading) > 0.5);
      if (bad.length) console.error(`Button loading changes width: ${bad.map((r) => `${r.name} ${r.rest} -> ${r.loading}`).join(", ")}`);
    };
    void document.fonts.ready.then(measure);
  }, []);
  const pairs: { name: string; el: (loading: boolean) => ReactNode }[] = [
    { name: "Primary md", el: (l) => <Button variant="primary" loading={l}>Confirm</Button> },
    { name: "Ghost md", el: (l) => <Button variant="ghost" loading={l}>Cancel</Button> },
    { name: "Secondary sm, icon", el: (l) => <Button size="sm" loading={l} leadingIcon={<Plus />}>New line</Button> },
    { name: "Danger lg", el: (l) => <Button size="lg" variant="danger" loading={l} trailingIcon={<Trash2 />}>Delete Accounting</Button> },
  ];
  const ok = rows.length > 0 && rows.every((r) => Math.abs(r.rest - r.loading) <= 0.5);
  return (
    <div ref={refs} className="gal-widths">
      {pairs.map((p) => {
        const r = rows.find((x) => x.name === p.name);
        return (
          <div key={p.name} className="gal-widths__row" data-pair={p.name}>
            {p.el(false)}
            {p.el(true)}
            <span className="t-small t-3 t-num">{r ? `${Math.round(r.rest)} = ${Math.round(r.loading)} px` : "…"}</span>
          </div>
        );
      })}
      {rows.length > 0 && (
        <Badge tone={ok ? "ok" : "danger"} size="sm" dot>
          {ok ? "Rest and loading widths match" : "Widths differ"}
        </Badge>
      )}
    </div>
  );
}

/**
 * The menu keyboard contract, asserted on load: Enter on a trigger opens the menu with
 * focus on the first item, and Escape puts focus back on the trigger. Runs once on a hidden
 * menu, only while nothing else has focus, and logs a console error on failure, which the
 * screenshot tool reports as a page error.
 */
function MenuCheck() {
  const host = useRef<HTMLSpanElement>(null);
  const [result, setResult] = useState<"pending" | "ok" | "fail" | "skipped">("pending");
  const [mounted, setMounted] = useState(true);
  useEffect(() => {
    const frame = () => new Promise((r) => requestAnimationFrame(() => r(null)));
    let cancelled = false;
    const run = async () => {
      await document.fonts.ready;
      await new Promise((r) => setTimeout(r, 120));
      const trigger = host.current?.querySelector<HTMLButtonElement>("button");
      if (cancelled || !trigger) return;
      if (document.activeElement && document.activeElement !== document.body) return setResult("skipped");
      const key = (el: Element, k: string) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
      key(trigger, "Enter");
      await frame();
      await frame();
      const opened = document.activeElement?.getAttribute("role") === "menuitem";
      if (document.activeElement) key(document.activeElement, "Escape");
      await frame();
      const returned = document.activeElement === trigger;
      setMounted(false);
      // Hand Tab order back to the top of the document: focusing the body for a moment
      // resets Chrome's starting point, so the first Tab still reaches "Skip to content".
      document.body.tabIndex = -1;
      document.body.focus({ preventScroll: true });
      document.body.removeAttribute("tabindex");
      const ok = opened && returned;
      setResult(ok ? "ok" : "fail");
      if (!ok) console.error(`Menu keyboard check failed: Enter ${opened ? "opened" : "did not open with focus on an item"}, Escape ${returned ? "returned focus" : "did not return focus to the trigger"}.`);
    };
    void run();
    return () => {
      cancelled = true;
    };
  }, []);
  return (
    <>
      {mounted && (
        <span ref={host} className="gal-selfcheck" aria-hidden>
          <Menu
            className="gal-selfcheck"
            trigger={<IconButton label="Menu self-check" icon={<MoreHorizontal />} size="sm" tabIndex={-1} />}
            items={[{ label: "First", onSelect: noop }, { label: "Second", onSelect: noop }]}
          />
        </span>
      )}
      <Badge tone={result === "ok" ? "ok" : result === "fail" ? "danger" : "neutral"} size="sm" dot>
        {result === "ok" ? "Enter opens on the first item, Escape returns focus" : result === "fail" ? "Menu keyboard check failed" : result === "skipped" ? "Skipped: something had focus" : "Checking…"}
      </Badge>
    </>
  );
}

/**
 * The typography rules, asserted on load over the whole gallery: no straight apostrophe
 * between letters and no three-period ellipsis, in visible copy and in placeholder,
 * aria-label and title attributes, outside code, mono and user data ([data-user-content]).
 * A hit logs a console error, which the screenshot tool reports as a page error.
 */
function TypographyCheck() {
  const [hits, setHits] = useState<string[] | null>(null);
  useEffect(() => {
    const t = window.setTimeout(() => {
      const root = document.querySelector(".gal-page");
      if (!root) return;
      const skip = "code, pre, kbd, samp, script, style, .t-mono, .is-mono, .g-badge--mono, [data-user-content]";
      const bad = (text: string) => (/[A-Za-z]'[A-Za-z]/.test(text) ? "straight apostrophe" : /\.\.\./.test(text) ? "three periods" : null);
      const found: string[] = [];
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const text = n.nodeValue ?? "";
        const why = bad(text);
        if (why && !n.parentElement?.closest(skip)) found.push(`${why}: “${text.trim().slice(0, 60)}”`);
      }
      for (const el of Array.from(root.querySelectorAll<HTMLElement>("[placeholder], [aria-label], [title]"))) {
        if (el.closest(skip.replace("code, pre, kbd, samp, ", ""))) continue;
        for (const attr of ["placeholder", "aria-label", "title"]) {
          const v = el.getAttribute(attr);
          const why = v ? bad(v) : null;
          if (why) found.push(`${why} in ${attr}: “${v!.slice(0, 60)}”`);
        }
      }
      setHits(found);
      if (found.length) console.error(`Typography in gallery copy (use ’ and …): ${found.slice(0, 5).join(", ")}`);
    }, 300);
    return () => window.clearTimeout(t);
  }, []);
  return (
    <Badge tone={hits == null ? "neutral" : hits.length ? "danger" : "ok"} size="sm" dot>
      {hits == null ? "Checking…" : hits.length ? `${hits.length} to fix` : "Quotes and ellipses correct"}
    </Badge>
  );
}

/** One h1 on this page, counted in the page panel after it settles. */
function HeadingCheck() {
  const [count, setCount] = useState<number | null>(null);
  useEffect(() => {
    const t = window.setTimeout(() => {
      const n = (document.getElementById("main") ?? document.body).querySelectorAll("h1").length;
      setCount(n);
      if (n !== 1) console.error(`The gallery has ${n} h1 elements; a page has exactly one.`);
    }, 300);
    return () => window.clearTimeout(t);
  }, []);
  return (
    <Badge tone={count == null ? "neutral" : count === 1 ? "ok" : "danger"} size="sm" dot>
      {count == null ? "Checking…" : count === 1 ? "One h1" : `${count} h1 elements`}
    </Badge>
  );
}

type RGBA = [number, number, number, number];
function parseColor(c: string): RGBA {
  const m = c.match(/rgba?\(([^)]+)\)/);
  if (m) {
    const [r, g, b, a = "1"] = m[1].split(/[ ,/]+/).filter(Boolean);
    return [+r, +g, +b, +a];
  }
  const n = c.match(/color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)(?: \/ ([\d.]+))?\)/);
  if (n) return [+n[1] * 255, +n[2] * 255, +n[3] * 255, n[4] == null ? 1 : +n[4]];
  return [0, 0, 0, 0];
}
function over(fg: RGBA, bg: RGBA): RGBA {
  const a = fg[3];
  return [fg[0] * a + bg[0] * (1 - a), fg[1] * a + bg[1] * (1 - a), fg[2] * a + bg[2] * (1 - a), 1];
}
function luminance([r, g, b]: RGBA): number {
  const l = (v: number) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  return 0.2126 * l(r) + 0.7152 * l(g) + 0.0722 * l(b);
}
function ratio(a: RGBA, b: RGBA): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

/**
 * WCAG contrast, asserted on load in this theme from the computed tokens: text-field edges
 * (rest and hover, on the canvas and inside a card, against the outside and the field's own
 * fill) at 3:1, and white labels on every button fill, rest and hover, at 4.5:1. A failure
 * logs a console error, which the screenshot tool reports as a page error.
 */
function ContrastCheck({ theme }: { theme: string }) {
  const host = useRef<HTMLSpanElement>(null);
  const [low, setLow] = useState<string[] | null>(null);
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const bad: string[] = [];
    for (const field of Array.from(el.querySelectorAll<HTMLElement>("[data-probe-field]"))) {
      const ground = parseColor(getComputedStyle(field.parentElement!).backgroundColor);
      const cs = getComputedStyle(field);
      const fill = over(parseColor(cs.backgroundColor), ground);
      const edge = over(parseColor(cs.borderTopColor), ground);
      const r = Math.min(ratio(edge, ground), ratio(edge, fill));
      if (r < 3) bad.push(`${field.dataset.probeField} ${r.toFixed(2)}:1`);
    }
    for (const fill of Array.from(el.querySelectorAll<HTMLElement>("[data-probe-fill]"))) {
      const r = ratio([255, 255, 255, 1], parseColor(getComputedStyle(fill).backgroundColor));
      if (r < 4.5) bad.push(`white on ${fill.dataset.probeFill} ${r.toFixed(2)}:1`);
    }
    setLow(bad);
    if (bad.length) console.error(`Contrast below AA in the ${theme} theme: ${bad.join(", ")}`);
  }, [theme]);
  const ground = (bg: string, name: string) => (
    <span style={{ background: `var(${bg})` }}>
      <span className="g-input" data-probe-field={`field edge on ${name}`} />
      <span className="g-input is-hover" data-probe-field={`hovered field edge on ${name}`} />
    </span>
  );
  return (
    <>
      <span ref={host} className="gal-contrast-probe" data-theme={theme} aria-hidden>
        {ground("--bg-canvas", "canvas")}
        {ground("--bg-surface", "surface")}
        {["--accent", "--accent-hover", "--accent-active", "--danger-fill", "--danger-fill-hover"].map((v) => (
          <span key={v} data-probe-fill={v} style={{ background: `var(${v})` }} />
        ))}
      </span>
      <Badge tone={low == null ? "neutral" : low.length ? "danger" : "ok"} size="sm" dot>
        {low == null ? "Checking…" : low.length ? `${low.length} below AA` : "Field edges 3:1, button labels 4.5:1"}
      </Badge>
    </>
  );
}

/** Every PendingChange state, with the clock pinned so the numbers hold still. */
function PendingSpecimens() {
  const toast = useToast();
  const [t0] = useState(() => Date.now());
  const at = (s: number) => new Date(t0 + s * 1000).toISOString();
  const props = {
    summary: "Close ghost-mock/app#151 as a duplicate of #148, with a comment linking the two.",
    rationale: "You said 151 is the same bug as 148. Closing is set to ask first in your policy.",
    operation: "github_issues.close",
    onConfirm: () => toast({ title: "Confirm pressed", description: "In the app this calls client.confirm() and nothing else." }),
    onCancel: () => toast({ title: "Cancel pressed" }),
    now: t0,
  };
  return (
    <>
      <Spec label="Fresh" column>
        <PendingChange {...props} expiresAt={at(42)} />
      </Spec>
      <Spec label="Under 10 seconds: the clock turns danger; one announcement at 10" column>
        <PendingChange {...props} expiresAt={at(8)} />
      </Spec>
      <Spec label="Busy, after Confirm" column>
        <PendingChange {...props} expiresAt={at(37)} busy="confirm" />
      </Spec>
      <Spec label="Expired: both buttons off until the daemon clears it" column>
        <PendingChange {...props} expiresAt={at(0)} />
      </Spec>
      <Spec label="Compact, 320 wide (tray)" column>
        <div className="gal-pending-compact">
          <PendingChange {...props} expiresAt={at(42)} compact />
          <PendingChange {...props} rationale={null} expiresAt={at(0)} compact />
        </div>
      </Spec>
      <Spec label="Touch, 358 wide (the phone): 16/24 summary, two 56px buttons" column>
        <div className="gal-pending-touch">
          <PendingChange {...props} expiresAt={at(42)} touch />
          <PendingChange {...props} expiresAt={at(6)} busy="confirm" touch />
        </div>
      </Spec>
      <p className="t-small t-3">
        In development every PendingChange checks on load that, whenever its Confirm is on screen, its summary is fully on screen too: not under the compact bar, the dock or the panel’s edge. See it in the <a href={scenarioHref("pending", "/")}>Waiting for okay</a> and <a href={scenarioHref("expiring", "/")}>Expiring</a> scenarios.
      </p>
    </>
  );
}

function SaveBarSpecimens() {
  const toast = useToast();
  const onSave = () => toast({ title: "Saved", description: "Ghost uses the new levels from its next change.", tone: "ok", action: { label: "Undo", onClick: noop } });
  return (
    <>
      <Spec label="Dirty: a neutral dot, the shortcut while Save can run, ghost Discard and filled Save 12px apart" column>
        <SaveBar title="3 unsaved changes" detail="closing, labels, everything else" onSave={onSave} onDiscard={noop} />
      </Spec>
      <Spec label="Saving: the caps go, Discard waits" column>
        <SaveBar title="3 unsaved changes" detail="closing, labels, everything else" busy onSave={noop} onDiscard={noop} />
      </Spec>
      <Spec label="Error: red edge, --danger dot, the reason on its own line" column>
        <SaveBar variant="error" title="Couldn’t save" detail="closing, labels" description="Ghost couldn’t write ~/.config/ghost/policy.toml: permission denied." onSave={onSave} onDiscard={noop} />
      </Spec>
      <Spec label="Offline: Save is off, the words say the changes wait here" column>
        <SaveBar variant="offline" title="3 unsaved changes" detail="they wait here until Ghost is back" onSave={noop} onDiscard={noop} />
      </Spec>
      <Spec label="Restart: the one thing left to do, the command in the mono slot, Dismiss" column>
        <SaveBar variant="restart" title="Restart Ghost to apply" detail="2 changes wait for it" command="systemctl --user restart ghostd" onDismiss={noop} />
      </Spec>
      <p className="t-small t-3">
        One silhouette on every screen that commits a change (Permissions, Settings, a line). It lives in the Page dock, at least 480 wide, growing with its words to the page width. After a save the toast is titled “Saved”, its body says when the change takes effect, with Undo where possible; the restart bar is the only exception and toasts nothing.
      </p>
    </>
  );
}

function Specimens({ theme }: { theme: string }) {
  const toast = useToast();
  const [sw, setSw] = useState(true);
  const [seg, setSeg] = useState<"day" | "week" | "month">("week");
  const [speed, setSpeed] = useState(1.05);
  const [dialog, setDialog] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [checks, setChecks] = useState({ app: true, infra: false });
  const [autonomy, setAutonomy] = useState<Autonomy>("confirm");
  const [classifier, setClassifier] = useState("auto");
  const [tab, setTab] = useState<"persona" | "memory" | "notes" | "transcript">("persona");
  const [level, setLevel] = useState(0.45);
  const [live, setLive] = useState(0.3);
  const id = (s: string) => (theme === "dark" ? `gal-${s}` : undefined);

  // A fake mic level for the live VoicePresence demo.
  useEffect(() => {
    let t = 0;
    const h = setInterval(() => {
      t += 1;
      setLive(Math.max(0, Math.min(1, 0.35 + 0.3 * Math.sin(t / 2.3) + 0.25 * Math.sin(t / 0.9))));
    }, 90);
    return () => clearInterval(h);
  }, []);

  const lineMenu = (
    <Menu
      align="end"
      trigger={<IconButton label="More for Accounting" icon={<MoreHorizontal />} size="sm" />}
      items={[
        { label: "Rename", icon: <Pencil />, onSelect: () => toast({ title: "Rename Accounting" }) },
        { label: "Pin a note", icon: <Pin />, shortcut: "mod+p", onSelect: () => toast({ title: "Pin a note" }) },
        { label: "Compact now", icon: <Eraser />, disabled: true, reason: "Nothing new to fold in", onSelect: noop },
        "separator",
        { label: "Reset memory", icon: <RotateCcw />, onSelect: () => toast({ title: "Reset Accounting" }) },
        { label: "Delete Accounting", icon: <Trash2 />, danger: true, onSelect: () => setConfirmOpen(true) },
      ]}
    />
  );

  return (
    <div className="gal-specimens">
      <Block id={id("color")} title="Color">
        <Spec label="Surfaces" column>
          <div className="gal-swatches">
            <Swatch name="App" varName="--bg-app" />
            <Swatch name="Canvas" varName="--bg-canvas" />
            <Swatch name="Surface" varName="--bg-surface" />
            <Swatch name="Raised" varName="--bg-raised" />
            <Swatch name="Sunken" varName="--bg-sunken" />
            <Swatch name="Border" varName="--border-strong" />
            <Swatch name="Track" varName="--control-track" />
          </div>
        </Spec>
        <Spec label="Text" column>
          <div className="gal-swatches">
            <Swatch name="Primary" varName="--text-1" text />
            <Swatch name="Secondary" varName="--text-2" text />
            <Swatch name="Tertiary" varName="--text-3" text />
            <Swatch name="Accent" varName="--accent-text" text />
          </div>
        </Spec>
        <Spec label="Accent and semantic" column>
          <div className="gal-swatches">
            <Swatch name="Accent" varName="--accent" />
            <Swatch name="Focus" varName="--focus-ring-color" />
            <Swatch name="OK" varName="--ok" />
            <Swatch name="Warn" varName="--warn" />
            <Swatch name="Danger" varName="--danger" />
            <Swatch name="Info" varName="--info" />
          </div>
        </Spec>
        <Spec label="Phases" column>
          <div className="gal-swatches">
            {PHASES.map((p) => (
              <Swatch key={p} name={p} varName={`--phase-${p}`} />
            ))}
          </div>
        </Spec>
        <Spec label="Line hues" scroll>
          {Array.from({ length: 8 }, (_, i) => (
            <LineGlyph key={i} slug={i === 0 ? "ghost" : `h${i}`} name={"GABCDEFG"[i]} hue={i} size="lg" />
          ))}
        </Spec>
      </Block>

      <Block id={id("type")} title="Type">
        <div className="gal-type">
          <p className="t-display">Display 32</p>
          <p className="t-title-lg">Page title 24</p>
          <p className="t-title">Title 20</p>
          <p className="t-heading">Heading 16</p>
          <p className="t-subheading">Subheading 14 semibold</p>
          <p className="t-body-lg">Reading text 14. Ghost closed app 151 as a duplicate of 148.</p>
          <p className="t-body">UI text 13. The default for labels, rows and controls.</p>
          <p className="t-small t-2">Small 12. Secondary detail and help text.</p>
          <p className="t-caption t-3">Caption 11. Timestamps and meta.</p>
          <p className="t-overline">Overline 11</p>
          <p className="t-mono">GHOST_WAKE_PHRASES · ghost-mock/app#151 · 0123456789</p>
        </div>
      </Block>

      <Block id={id("space")} title="Space, radius, elevation">
        <Spec label="Spacing (4px grid)" scroll>
          <div className="gal-spaces">
            {[1, 2, 3, 4, 5, 6, 8, 10, 12, 16].map((n) => (
              <div key={n} className="gal-space">
                <span style={{ width: `var(--space-${n})`, height: `var(--space-${n})` }} />
                <small>{n * 4}</small>
              </div>
            ))}
          </div>
        </Spec>
        <Spec label="Radius" scroll>
          {["xs", "sm", "md", "lg", "xl", "full"].map((r) => (
            <div key={r} className="gal-radius" style={{ borderRadius: `var(--radius-${r})` }}>{r}</div>
          ))}
        </Spec>
        <Spec label="Elevation" scroll>
          <div className="gal-elev-patch">
            {["xs", "sm", "md", "lg"].map((e) => (
              <div key={e} className="gal-elev" style={{ boxShadow: `var(--shadow-${e})` }}>{e}</div>
            ))}
          </div>
        </Spec>
      </Block>

      <Block id={id("buttons")} title="Buttons">
        <Matrix
          states={["Rest", "Hover", "Pressed", "Focus", "Disabled", "Loading"]}
          rows={(["primary", "secondary", "ghost", "danger", "danger-ghost"] as const).map((v) => ({
            label: v,
            cells: [
              <Button key="r" variant={v} size="sm">Save</Button>,
              <Button key="h" variant={v} size="sm" className="is-hover">Save</Button>,
              <Button key="a" variant={v} size="sm" className="is-active">Save</Button>,
              <Button key="f" variant={v} size="sm" className="is-focus">Save</Button>,
              <Button key="d" variant={v} size="sm" disabled>Save</Button>,
              <Button key="l" variant={v} size="sm" loading>Save</Button>,
            ],
          }))}
        />
        <Spec label="Sizes">
          <Button size="sm">Small</Button>
          <Button size="md">Medium</Button>
          <Button size="lg">Large</Button>
        </Spec>
        <Spec label="With icons">
          <Button leadingIcon={<Plus />}>New line</Button>
          <Button variant="ghost" leadingIcon={<Pin />}>Pin a note</Button>
          <Button trailingIcon={<ArrowUpRight />}>Open on GitHub</Button>
        </Spec>
        <Spec label="Link form">
          <Button href="https://github.com/ghost-mock/app/issues/151" target="_blank" trailingIcon={<ArrowUpRight />}>Open on GitHub</Button>
          <Button variant="ghost" href="#/audit">See the audit log</Button>
          <IconButton href="https://github.com/ghost-mock/app" target="_blank" label="Open the repository" icon={<ArrowUpRight />} />
        </Spec>
        <Spec label="Live">
          <Button
            variant="primary"
            loading={loading}
            onClick={() => {
              setLoading(true);
              setTimeout(() => setLoading(false), 1500);
            }}
          >
            Save
          </Button>
          <span className="t-small t-3" role="status" aria-live="polite">{loading ? "Saving…" : "Click it: the width stays put."}</span>
        </Spec>
        <Spec label="Loading keeps the width" column>
          <WidthCheck />
        </Spec>
        <Spec label="Icon buttons, by state" column>
        <Matrix
          states={["Rest", "Hover", "Pressed", "Focus", "Disabled", "Loading"]}
          rows={(["ghost", "secondary", "primary"] as const).map((v) => ({
            label: v,
            cells: [
              <IconButton key="r" variant={v} label="Send" tooltip={false} icon={v === "primary" ? <ArrowUp /> : <Settings2 />} />,
              <IconButton key="h" variant={v} label="Send" tooltip={false} className="is-hover" icon={v === "primary" ? <ArrowUp /> : <Settings2 />} />,
              <IconButton key="a" variant={v} label="Send" tooltip={false} className="is-active" icon={v === "primary" ? <ArrowUp /> : <Settings2 />} />,
              <IconButton key="f" variant={v} label="Send" tooltip={false} className="is-focus" icon={v === "primary" ? <ArrowUp /> : <Settings2 />} />,
              <IconButton key="d" variant={v} label="Send" tooltip={false} disabled icon={v === "primary" ? <ArrowUp /> : <Settings2 />} />,
              <IconButton key="l" variant={v} label="Send" tooltip={false} loading icon={<Bell />} />,
            ],
          }))}
        />
        </Spec>
        <Spec label="Icon buttons">
          <IconButton label="Search" icon={<Search />} />
          <IconButton label="Mute microphone" icon={<Mic />} pressed />
          <IconButton label="Copy" icon={<Copy />} size="sm" />
          <IconButton label="Hears idle audio" icon={<Ear />} size="lg" />
        </Spec>
      </Block>

      <Block id={id("inputs")} title="Inputs and fields">
        <div className="gal-grid2">
          <Field label="Wake phrases" help="Comma-separated." detail="GHOST_WAKE_PHRASES">
            <Input defaultValue="ghost-mock/app, ghost-mock/infra" />
          </Field>
          <Field label="Search" optional>
            <Input placeholder="Search issues…" leading={<Search />} trailing={<Kbd>/</Kbd>} />
          </Field>
          {/* Secrets live in a form, so Enter saves and Chrome has nothing to warn about. */}
          <form onSubmit={(e) => e.preventDefault()} autoComplete="off">
            <Field label="GitHub token" error="That doesn’t look like a GitHub token. They start with ghp_ or github_pat_.">
              <Input type="password" autoComplete="off" defaultValue="hunter2" />
            </Field>
          </form>
          <Field label="Model endpoint" help="Disabled while Ghost is busy.">
            <Input disabled defaultValue="https://api.anthropic.com" />
          </Field>
          <Field label="Voice">
            <Select defaultValue="bm_george" options={[{ value: "af_heart", label: "American female: Heart" }, { value: "bm_george", label: "British male: George" }]} />
          </Field>
          <Field label="Speed" aside={`${speed.toFixed(2)}×`}>
            <Slider min={0.5} max={2} step={0.05} value={speed} onChange={setSpeed} />
          </Field>
        </div>
        <Field label="Persona, in your words" help="Ghost stores this verbatim.">
          <Textarea rows={3} defaultValue="You’re the accounting desk. Numbers first, no small talk." />
        </Field>
        <Spec label="States" column>
          <div className="gal-states">
            <Input placeholder="Rest" />
            <Input placeholder="Hover" className="is-hover" />
            <Input defaultValue="Focus" className="is-focus" />
            <Input defaultValue="Invalid" invalid />
            <Input defaultValue="Invalid, focus" invalid className="is-focus" />
            <Input defaultValue="Disabled" disabled />
          </div>
        </Spec>
        <Spec label="Sizes" column>
          <div className="gal-sizes">
            <Input size="sm" placeholder="Small, 28" />
            <Input size="md" placeholder="Medium, 32" />
            <Input size="lg" placeholder="Large, 40" />
          </div>
        </Spec>
        <Spec label="Inline rows at two widths. Under 560px a Field stacks; the second card is 400px. In Both, columns are narrow, so choose Dark or Light to see the first card inline." column>
          <div className="gal-inline-widths">
            {[0, 1].map((n) => (
              <Card key={n} list>
                <div className="gal-row-pad">
                  <Field layout="inline" label="Wake sensitivity" detail="GHOST_WAKE_FUZZY" help="0 to 1. Higher hears more near-misses.">
                    <Input defaultValue="0.8" />
                  </Field>
                </div>
                <div className="gal-row-pad">
                  <Field layout="inline" label="How Ghost hears your yes" help="Jev when its key is set, else the word list." detail="GHOST_JEV">
                    <Select value={classifier} onChange={(e) => setClassifier(e.target.value)} options={[{ value: "auto", label: "Automatic" }, { value: "on", label: "Always Jev" }, { value: "off", label: "Word list only" }]} />
                  </Field>
                </div>
                <div className="gal-row-pad">
                  <Field layout="inline" controlWidth="auto" label="Echo cancellation" help="Ghost hears itself less when it talks.">
                    <Switch checked={sw} onChange={setSw} />
                  </Field>
                </div>
              </Card>
            ))}
          </div>
        </Spec>
      </Block>

      <Block id={id("choices")} title="Choices">
        <Matrix
          states={["Rest", "Hover", "Pressed", "Focus", "Disabled"]}
          rows={[
            { label: "switch off", cells: [<Switch key="r" checked={false} onChange={noop} aria-label="Off" />, <Switch key="h" checked={false} onChange={noop} aria-label="Off" className="is-hover" />, <Switch key="a" checked={false} onChange={noop} aria-label="Off" className="is-active" />, <Switch key="f" checked={false} onChange={noop} aria-label="Off" className="is-focus" />, <Switch key="d" checked={false} onChange={noop} aria-label="Off" disabled />] },
            { label: "switch on", cells: [<Switch key="r" checked onChange={noop} aria-label="On" />, <Switch key="h" checked onChange={noop} aria-label="On" className="is-hover" />, <Switch key="a" checked onChange={noop} aria-label="On" className="is-active" />, <Switch key="f" checked onChange={noop} aria-label="On" className="is-focus" />, <Switch key="d" checked onChange={noop} aria-label="On" disabled />] },
            { label: "checkbox off", cells: [<Checkbox key="r" checked={false} onChange={noop} aria-label="Off" />, <Checkbox key="h" checked={false} onChange={noop} aria-label="Off" className="is-hover" />, <Checkbox key="a" checked={false} onChange={noop} aria-label="Off" className="is-active" />, <Checkbox key="f" checked={false} onChange={noop} aria-label="Off" className="is-focus" />, <Checkbox key="d" checked={false} onChange={noop} aria-label="Off" disabled />] },
            { label: "checkbox on", cells: [<Checkbox key="r" checked onChange={noop} aria-label="On" />, <Checkbox key="h" checked onChange={noop} aria-label="On" className="is-hover" />, <Checkbox key="a" checked onChange={noop} aria-label="On" className="is-active" />, <Checkbox key="f" checked onChange={noop} aria-label="On" className="is-focus" />, <Checkbox key="d" checked onChange={noop} aria-label="On" disabled />] },
          ]}
        />
        <Spec label="Switch">
          <Switch checked={sw} onChange={setSw} label="Echo cancellation" />
          <Switch checked onChange={noop} size="sm" label="Small, 16" />
        </Spec>
        <Spec label="Checkbox" column>
          <Checkbox
            checked={checks.app && checks.infra}
            indeterminate={checks.app !== checks.infra}
            onChange={(v) => setChecks({ app: v, infra: v })}
            label="All repositories"
          />
          <div className="gal-indent stack gap-3">
            <Checkbox checked={checks.app} onChange={(v) => setChecks((c) => ({ ...c, app: v }))} label="ghost-mock/app" description="214 open issues. Ghost reads and writes here." />
            <Checkbox checked={checks.infra} onChange={(v) => setChecks((c) => ({ ...c, infra: v }))} label="ghost-mock/infra" description="38 open issues." />
            <Checkbox checked={false} onChange={noop} disabled label="ghost-mock/website" description="Your token can’t see this repository." />
          </div>
        </Spec>
        <Spec label="Radio, list">
          <RadioGroup
            aria-label="How Ghost hears your yes"
            value={classifier}
            onChange={setClassifier}
            options={[
              { value: "auto", label: "Automatic", description: "Jev when its key is set, otherwise the word list." },
              { value: "on", label: "Always Jev" },
              { value: "off", label: "Word list only" },
              { value: "x", label: "Local model", description: "Not installed.", disabled: true },
            ]}
          />
        </Spec>
        <Spec label="Radio, cards" column>
          <RadioGroup<Autonomy>
            variant="card"
            label="Closing an issue"
            value={autonomy}
            onChange={setAutonomy}
            options={[
              { value: "confirm", label: "Ask me first", description: `Ghost proposes it and waits for a spoken ${quoted("yes")} or your click.`, icon: <Hand /> },
              { value: "announce", label: "Do it and tell me", description: "Ghost does it, then says what it did.", icon: <Megaphone /> },
              { value: "quiet", label: "Do it quietly", description: "Logged, not spoken.", icon: <VolumeX /> },
              { value: "deny", label: "Not allowed", description: "Ghost can’t do this, even if you ask.", icon: <ShieldOff /> },
            ]}
          />
          <RadioGroup<Autonomy>
            variant="card"
            columns={2}
            aria-label="Forced states"
            className="is-focus"
            value="confirm"
            onChange={noop}
            options={[
              { value: "confirm", label: "Selected, focus", description: "accent border, soft fill" },
              { value: "announce", label: "Disabled", description: "not offered here", disabled: true },
            ]}
          />
        </Spec>
        <Spec label="Segmented">
          <SegmentedControl aria-label="Audit range" value={seg} onChange={setSeg} options={[{ value: "day", label: "Day" }, { value: "week", label: "Week" }, { value: "month", label: "Month" }]} />
          <SegmentedControl aria-label="Size" size="sm" value="a" onChange={noop} options={[{ value: "a", label: "Small" }, { value: "b", label: "Set" }, { value: "c", label: "Off", disabled: true }]} />
        </Spec>
        <Spec label="Disclosure">
          <Disclosure summary="Devices PortAudio sees">
            <p className="t-mono t-2">16 default in:128 out:128 (default in)</p>
          </Disclosure>
        </Spec>
      </Block>

      <Block id={id("menus")} title="Menus and tabs">
        <Spec label="Menu">
          <div className="gal-menu-demo">
            <Card list className="grow">
              <ListRow leading={<LineGlyph slug="accounting" name="Accounting" />} title="Accounting" description="3 min ago" trailing={lineMenu} />
            </Card>
          </div>
        </Spec>
        <Spec label="Menu, open">
          <div className="g-menu is-placed gal-static gal-static-menu" role="presentation">
            <div className="g-menu__item"><span className="g-menu__icon"><Pencil /></span><span className="g-menu__label">Rename</span></div>
            <div className="g-menu__item is-active"><span className="g-menu__icon"><Pin /></span><span className="g-menu__label">Pin a note</span><span className="g-menu__shortcut t-num">{formatShortcut("mod+p")}</span></div>
            <div className="g-menu__item is-disabled"><span className="g-menu__icon"><Eraser /></span><span className="g-menu__label">Compact now</span></div>
            <div className="g-menu__separator" />
            <div className="g-menu__item"><span className="g-menu__icon"><RotateCcw /></span><span className="g-menu__label">Reset memory</span></div>
            <div className="g-menu__item is-danger"><span className="g-menu__icon"><Trash2 /></span><span className="g-menu__label">Delete Accounting</span></div>
          </div>
        </Spec>
        <Tabs
          aria-label="Accounting line"
          value={tab}
          onChange={setTab}
          items={[
            { value: "persona", label: "Persona" },
            { value: "memory", label: "Memory" },
            { value: "notes", label: "Pinned notes", count: 3 },
            { value: "transcript", label: "Transcript", disabled: true },
          ]}
        >
          <p className="t-body-lg t-2">
            {tab === "persona" && "You’re the accounting desk. Numbers first, no small talk."}
            {tab === "memory" && "Marta owns invoicing. Stripe renewal is November 3. Prefers issue numbers read as plain numbers."}
            {tab === "notes" && "Stripe renewal is due November 3. · Close the books on the 5th. · Halden GmbH pays by SEPA."}
          </p>
        </Tabs>
      </Block>

      <Block id={id("voice")} title="Voice presence">
        <Spec label="96, every phase" column>
          <div className="gal-voice-grid">
            {PHASES.map((p) => (
              <div key={p} className="gal-voice">
                <VoicePresence phase={p} size={96} />
                <small>{phaseText(p)}</small>
              </div>
            ))}
          </div>
        </Spec>
        <Spec label="48" scroll>
          {PHASES.map((p) => (
            <VoicePresence key={p} phase={p} size={48} />
          ))}
        </Spec>
        <Spec label="160 with level">
          <div className="gal-voice">
            <VoicePresence phase="listening" size={160} level={live} />
            <small>Listening, live mic level</small>
          </div>
          <div className="gal-voice">
            <VoicePresence phase="speaking" size={160} level={level} />
            <small>Speaking, level {level.toFixed(2)}</small>
          </div>
        </Spec>
        <Field label="Speaking level">
          <Slider min={0} max={1} step={0.05} value={level} onChange={setLevel} format={(v) => v.toFixed(2)} />
        </Field>
      </Block>

      <Block id={id("status")} title="Status">
        <Spec label="Status dot" column>
          {PHASES.map((p) => (
            <div key={p} className="row gap-2">
              <StatusDot phase={p} label={phaseText(p)} />
              <span>{phaseText(p)}</span>
              <span className="t-3 t-small">{p === "passive" || p === "offline" ? "still" : "animated"}</span>
            </div>
          ))}
        </Spec>
        <Spec label="Status card: the sidebar’s one phase signal, on the window ground" column>
          <div className="gal-status-cards">
            <StatusCard phase={liveLabel("connecting", "offline").dot} title={liveLabel("connecting", "offline").title} sub="Looking for Ghost" />
            <StatusCard phase="passive" title={liveLabel("online", "passive").title} sub="Nothing leaves this computer" />
            <StatusCard phase="listening" title={liveLabel("online", "listening").title} sub="On the Accounting line" />
            <StatusCard phase={liveLabel("reconnecting", "offline").dot} title={liveLabel("reconnecting", "offline").title} sub="Trying again now" />
            <StatusCard phase="starting" title={liveLabel("online", "starting").title} sub="Loading voice models" />
            <StatusCard phase={liveLabel("offline", "offline").dot} title={liveLabel("offline", "offline").title} sub="The Ghost service is stopped" />
          </div>
          <p className="t-small t-3">Connecting is neutral and still. It turns to Not running only when the shell gives up (about 900ms), the same moment the Offline page appears. Every page’s <code>&lt;LiveStatus /&gt;</code> uses the same <code>liveLabel()</code>, so a page never contradicts this card.</p>
        </Spec>
        <Spec label="Change status (statusLabel, statusTone)">
          {["pending", "claimed", "executed", "expired", "cancelled", "stale", "failed"].map((st) => (
            <Badge key={st} tone={statusTone(st)} dot>{statusLabel(st)}</Badge>
          ))}
        </Spec>
        <Spec label="Badges">
          <Badge>Not set</Badge>
          <Badge tone="accent">Prototype</Badge>
          <Badge tone="ok" dot>Set</Badge>
          <Badge tone="warn" dot>Waiting</Badge>
          <Badge tone="danger" dot>Failed</Badge>
          <Badge tone="info">Info</Badge>
        </Spec>
        <Spec label="Outline badges">
          <Badge variant="outline">Optional</Badge>
          <Badge variant="outline" tone="warn">Medium risk</Badge>
          <Badge variant="outline" tone="danger">High risk</Badge>
          <Badge size="sm" mono>v0.1.0</Badge>
        </Spec>
        <Spec label="Progress">
          <div className="gal-progress">
            <Progress label="Saved" value={64} />
            <Progress label="Loading" />
            <Progress label="Loading voice models" tone="starting" />
          </div>
        </Spec>
        <Spec label="Spinner and keys">
          <Spinner size={16} label="Loading" />
          <Kbd shortcut="mod+k" />
          <Kbd shortcut="mod+shift+z" />
          <Kbd shortcut="super+g" />
          <Kbd>Esc</Kbd>
        </Spec>
      </Block>

      <Block id={id("pending")} title="Pending change">
        <PendingSpecimens />
      </Block>

      <Block id={id("savebar")} title="Save bar">
        <SaveBarSpecimens />
      </Block>

      <Block id={id("surfaces")} title="Cards and sections">
        <div className="gal-grid2">
          <Card><p className="t-medium">Card</p><p className="t-2">Default surface, hairline border.</p></Card>
          <Card tone="sunken"><p className="t-medium">Sunken</p><p className="t-2">Wells: logs, code.</p></Card>
          <Card tone="accent"><p className="t-medium">Accent</p><p className="t-2">One per page at most.</p></Card>
          <Card tone="warn"><p className="t-medium">Warn</p><p className="t-2">A change waiting for you.</p></Card>
        </div>
        <PageHeader headingLevel="p" title="Page header" description="Title, one-line description, actions on the right." actions={<><Button variant="ghost">Edit TOML</Button><Button variant="primary">Save</Button></>} />
        <Spec label="Compact bar: the same header once it scrolls under the panel’s top edge" column>
          <div className="gal-page-bar">
            <div className="g-page-bar is-titled">
              <div className="g-page-bar__inner">
                <div className="g-page-bar__heading">
                  <span className="g-page-bar__title">Permissions</span>
                </div>
                <div className="g-page-bar__actions">
                  <ButtonSizeContext.Provider value="sm">
                    <Button variant="ghost">Edit TOML</Button>
                    <Button variant="primary">Save</Button>
                  </ButtonSizeContext.Provider>
                </div>
              </div>
            </div>
            <p className="t-2">Wrapping a long policy row that has scrolled under the bar, hidden behind it.</p>
          </div>
          <div className="gal-page-bar">
            <div className="g-page-bar is-titled">
              <div className="g-page-bar__inner">
                <div className="g-page-bar__heading">
                  <span className="g-page-bar__leading"><LineGlyph slug="accounting" name="Accounting" size="sm" /></span>
                  <span className="g-page-bar__title">Accounting</span>
                  <span className="g-page-status g-page-bar__status"><StatusDot phase="listening" size="sm" />{phaseText("listening")}</span>
                </div>
                <div className="g-page-bar__actions">
                  <ButtonSizeContext.Provider value="sm">
                    <Button leadingIcon={<Moon />}>Go to sleep</Button>
                  </ButtonSizeContext.Provider>
                </div>
              </div>
            </div>
            <p className="t-2">The session’s transcript, scrolled to its newest turn.</p>
          </div>
          <div className="gal-page-bar">
            <div className="g-page-bar is-titled">
              <div className="g-page-bar__inner">
                <div className="g-page-bar__heading">
                  <span className="g-page-bar__leading"><LineGlyph slug="accounting" name="Accounting" size="sm" /></span>
                  <span className="g-page-bar__title">Accounting</span>
                  <span className="g-page-status g-page-bar__status"><StatusDot phase={liveLabel("reconnecting", "offline").dot} size="sm" still />{liveLabel("reconnecting", "offline").title}</span>
                </div>
                <div className="g-page-bar__actions">
                  <ButtonSizeContext.Provider value="sm">
                    <Button leadingIcon={<Moon />} disabled>Go to sleep</Button>
                  </ButtonSizeContext.Provider>
                </div>
              </div>
            </div>
            <p className="t-2">Reconnecting: the same words as the sidebar card, the dot still under the moving banner, writes off.</p>
          </div>
          <p className="t-small t-3">
            Automatic for every Page with a header. The actions are the header’s own, moved into the bar, not a copy: one instance, one tab stop. Scroll <a href={scenarioHref("busy", "/audit")}>the audit log in Busy</a> to see it live.
          </p>
        </Spec>
      </Block>

      <Block id={id("lists")} title="Rows and tables">
        <Card list>
          <ListRow leading={<LineGlyph slug="ghost" name="Ghost" />} title="Ghost" description="Selected" onClick={noop} selected />
          <ListRow leading={<LineGlyph slug="security" name="Security" />} title="Security" description="Selected, hover" onClick={noop} selected className="is-hover" />
          <ListRow leading={<LineGlyph slug="release" name="Release" />} title="Release" description="Selected, focus" onClick={noop} selected className="is-focus" />
          <ListRow leading={<LineGlyph slug="accounting" name="Accounting" />} title="Accounting" description="Hover" onClick={noop} className="is-hover" trailing={<Button size="sm" variant="ghost">Switch</Button>} />
          <ListRow leading={<LineGlyph slug="standup" name="Standup" />} title="Standup" description="Pressed" onClick={noop} className="is-active" />
          <ListRow leading={<LineGlyph slug="legal" name="Legal" />} title="Legal" description="Focus" chevron onClick={noop} className="is-focus" />
          <ListRow title="Echo cancellation" description="Rest, with a trailing control" trailing={<Switch checked onChange={noop} aria-label="Echo cancellation" />} />
          <ListRow title="Disabled row" description="Not available yet" onClick={noop} disabled />
        </Card>
        <Card list>
          <Table density="compact" interactive>
            <thead>
              <tr>
                <th>Change</th>
                <th>Issue</th>
                <th className="is-shrink">Status</th>
                <th className="is-shrink is-num">When</th>
              </tr>
            </thead>
            <tbody>
              <tr><td className="is-mono">close</td><td className="is-mono is-nowrap">ghost-mock/app#151</td><td><Badge tone={statusTone("pending")} dot>{statusLabel("pending")}</Badge></td><td className="is-num is-muted is-nowrap">12:41</td></tr>
              <tr><td className="is-mono">approve</td><td className="is-mono is-nowrap">ghost-mock/app#208</td><td><Badge tone={statusTone("executed")} dot>{statusLabel("executed")}</Badge></td><td className="is-num is-muted is-nowrap">12:37</td></tr>
              <tr><td className="is-mono">comment</td><td className="is-mono is-nowrap">ghost-mock/infra#91</td><td><Badge tone={statusTone("failed")} dot>{statusLabel("failed")}</Badge></td><td className="is-num is-muted is-nowrap">Mon</td></tr>
            </tbody>
          </Table>
        </Card>
      </Block>

      <Block id={id("feedback")} title="Notices, empty, loading">
        <div className="stack gap-2">
          <Notice title="Saved and applied">Every level still logs the write.</Notice>
          <Notice tone="ok">Echo cancellation installed.</Notice>
          <Notice tone="info" title="Restart needed">
            <p>Restart Ghost to use the new microphone.</p>
            <code className="t-mono t-3">systemctl --user restart ghostd</code>
          </Notice>
          <Notice tone="warn" action={<Button size="sm">Install</Button>}>Ghost can hear itself. Install echo cancellation.</Notice>
          <Notice tone="danger" title="Couldn’t save" onDismiss={noop}>The policy file has a syntax error on line 14.</Notice>
        </div>
        <Card>
          <EmptyState icon={<Inbox />} title="Nothing waiting on you" description="When an issue needs your sign-off, it shows up here." actions={<Button>Check now</Button>} />
        </Card>
        <Card>
          <div className="stack gap-3">
            <Skeleton width={180} height={20} radius={6} />
            <SkeletonText lines={3} />
          </div>
        </Card>
        <Spec label="Toasts">
          <Button onClick={() => toast({ title: "Approved app 208", description: "Left your note that Dana’s fix looks right.", tone: "ok" })}>Success</Button>
          <Button onClick={() => toast({ title: "Pinned to Accounting" })}>Neutral</Button>
          <Button onClick={() => toast({ title: "Couldn’t reach GitHub", description: "Ghost will retry in 30 seconds.", tone: "danger" })}>Error</Button>
        </Spec>
        <div className="g-toast g-toast--ok gal-static">
          <CircleCheck size={16} className="g-toast__icon" />
          <div className="g-toast__body">
            <p className="g-toast__title">Saved</p>
            <p className="g-toast__description">Ghost uses the new levels from its next change.</p>
          </div>
          <button type="button" className="g-toast__action">Undo</button>
        </div>
      </Block>

      <Block id={id("overlays")} title="Overlays">
        <Spec label="Open">
          <Button onClick={() => setDialog(true)}>Dialog</Button>
          <Button variant="danger-ghost" onClick={() => setConfirmOpen(true)}>Confirm dialog</Button>
          <Tooltip content="Pin a note" shortcut="mod+p">
            <Button variant="ghost" leadingIcon={<Pin />}>Hover for tooltip</Button>
          </Tooltip>
        </Spec>
        <div className="gal-static-dialog">
          <div className="g-dialog__panel">
            <header className="g-dialog__header"><h2 className="g-dialog__title">Delete Legal?</h2></header>
            <p className="g-dialog__description">The line, its persona, memory and notes are removed.</p>
            <footer className="g-dialog__footer"><Button variant="ghost" className="is-focus">Cancel</Button><Button variant="danger">Delete Legal</Button></footer>
          </div>
        </div>
        <div className="g-tooltip is-placed gal-static">Pin a note <span className="g-tooltip__shortcut">Ctrl P</span></div>
        <Dialog open={dialog} onClose={() => setDialog(false)} title="Pin a note to Accounting" description="Pinned notes are never summarized away." footer={<><Button variant="ghost" onClick={() => setDialog(false)}>Cancel</Button><Button variant="primary" onClick={() => setDialog(false)}>Pin note</Button></>}>
          <Field label="Note"><Input autoFocus placeholder="Stripe renewal is due November 3" /></Field>
        </Dialog>
        <ConfirmDialog open={confirmOpen} title="Delete Accounting?" description="The line, its persona, memory and notes are removed." confirmLabel="Delete Accounting" tone="danger" onCancel={() => setConfirmOpen(false)} onConfirm={() => new Promise((r) => setTimeout(r, 800)).then(() => setConfirmOpen(false))} />
      </Block>

      <Block id={id("brand")} title="Brand">
        <Spec label="Mark" scroll>
          <GhostMark size={20} /><GhostMark size={24} /><GhostMark size={28} /><GhostMark size={40} /><GhostMark size={64} />
        </Spec>
        <Spec label="Glyph" scroll>
          <GhostGlyph size={12} /><GhostGlyph size={14} /><GhostGlyph size={16} /><GhostGlyph size={20} /><GhostGlyph size={24} /><GhostGlyph size={40} />
        </Spec>
        <Spec label="Lines" scroll>
          <LineGlyph slug="ghost" name="Ghost" size="sm" /><LineGlyph slug="ghost" name="Ghost" size="md" /><LineGlyph slug="accounting" name="Accounting" /><LineGlyph slug="legal" name="Legal" size="lg" /><LineGlyph slug="standup" name="Standup" size="xl" /><LineGlyph slug="ghost" name="Ghost" size="xl" />
        </Spec>
        <MascotSpec />
      </Block>
    </div>
  );
}
