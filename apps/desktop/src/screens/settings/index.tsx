/**
 * Owner: settings. How Ghost connects, listens and speaks: secrets (write-only), the model,
 * GitHub, voice and wake, microphone and speakers, voice models. Edits collect into one draft;
 * a save bar in the Page dock appears once something changed (Klaviyo), and says afterwards
 * what applied now and what waits for a restart. An "on this page" rail sits to the right of
 * the column (Neon). Non-secret edits survive leaving the page (sessionStorage); secrets never do.
 * Discard is one click and reversible from its toast (values only; a typed secret is gone).
 *
 * Focus never falls to <body>: closing a secret returns to its Replace… button, Save or Discard
 * return to the field you were in (or to the restart pill's Copy button), and echo Install or
 * Remove hand over to whatever replaced them once the device list has reloaded.
 *
 * The rail, the ?section= landing and keyboard focus all clear the whole sticky stack above
 * the content (any shell banner plus the compact page bar), measured live into
 * --set-sticky-offset, so nothing lands under a Starting or Reconnecting banner.
 *
 * States by URL (mock): any scenario, plus `?section=<id>` to land on a section, and in mock
 * mode only `?settings=dirty|invalid|error|restart|secret|loading|loaderror|nodevices|echoidle|pairing` to
 * hold a state for review (`pairing` opens the Pair a phone dialog; the phone's own states are
 * `?phone=off|unpaired|paired|pairs|expires`, see Phone.tsx).
 */
import type { DevicesView, ModelsView, SettingsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ArrowRight, MoreHorizontal, ShieldCheck, Trash2 } from "lucide-react";
import {
  type FocusEvent,
  type FormEvent,
  lazy,
  type ReactNode,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { hrefFor, navigate } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  Disclosure,
  Field,
  IconButton,
  Input,
  ListRow,
  Menu,
  LiveStatus,
  Notice,
  Page,
  Progress,
  Section,
  SegmentedControl,
  Select,
  Skeleton,
  Slider,
  Table,
  errorText,
  useConfirm,
  useFieldControl,
  useLive,
  useToast,
} from "../../ui";
import {
  DEFAULT_MODEL,
  type Draft,
  JEV_NEEDS_KEY,
  JEV_OPTIONS,
  KEY_SECTION,
  SECRET_ORDER,
  SECTION_TITLE,
  type SectionId,
  VALUE_KEYS,
  changedKeys,
  echoReady,
  fuzzyWords,
  isOptionalSecret,
  isSet,
  jevNow,
  joinList,
  listWords,
  lowerFirst,
  micDiffers,
  micInUse,
  micOptions,
  modelReady,
  parseFuzzy,
  micDismissedFor,
  readDraft,
  storeMicDismissed,
  tildePath,
  sectionDomId,
  splitList,
  storeDraft,
  truthy,
  validate,
  valueChanged,
} from "./model";
import { type BarState, ChipsInput, CommandCopy, LinesInput, type NavItem, PageNav, Row, SaveBar, SecretRow, SetupSection, type SetupStep, secretActionId } from "./parts";
import { PhoneSection } from "./Phone";
import "./settings.css";

// Charter sections with no API yet. Compiled only in development; rendered only under the mock layer.
const Prototype = import.meta.env.DEV ? lazy(() => import("./prototype")) : null;

const FORCED = new Set(["dirty", "invalid", "error", "restart", "secret", "loading", "loaderror", "nodevices", "echoidle", "pairing"]);

const FUZZY = "GHOST_WAKE_FUZZY";
const fieldId = (k: string) => `set-f-${k}`;

/** Focus an element without scrolling; for a control that can't take focus itself, its checked radio or first control. */
function focusInto(el: HTMLElement | null) {
  if (!el) return false;
  const focusable = el.matches("input, select, textarea, button, [tabindex]:not([tabindex='-1'])")
    ? el
    : (el.querySelector<HTMLElement>("[role=radio][aria-checked=true]") ?? el.querySelector<HTMLElement>("input, select, textarea, button"));
  focusable?.focus({ preventScroll: true });
  return !!focusable && document.activeElement === focusable;
}

const ECHO_IDS = { offer: "set-echo-offer", install: "set-echo-install", reinstall: "set-echo-reinstall", more: "set-echo-more", msg: "set-echo-msg" };

/**
 * How far from the top of the page panel (#main) content is hidden: the shell's banners plus
 * the compact page bar under them, from the shell's own `--page-sticky-top`.
 */
function stickyBottom(): number {
  const main = document.getElementById("main");
  const style = getComputedStyle(main ?? document.documentElement);
  const v = parseFloat(style.getPropertyValue("--page-sticky-top"));
  return Number.isFinite(v) ? v : parseFloat(style.getPropertyValue("--page-bar-height")) || 48;
}
/** Breathing room between the sticky stack and a section title that was scrolled to (24px). */
const LAND_GAP = 24;

export default function SettingsScreen(_: RouteProps) {
  const { client, status } = useGhost();
  const { online, connection } = useLive();
  const design = useDesign();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();

  const params = useMemo(() => new URLSearchParams(window.location.search), []);
  const forcedParam = (params.get("settings") ?? "").toLowerCase();
  const forced = design && FORCED.has(forcedParam) ? forcedParam : null;

  const [s, setS] = useState<SettingsView | null>(null);
  const [devices, setDevices] = useState<DevicesView | null>(null);
  const [devicesError, setDevicesError] = useState<string | null>(null);
  const [models, setModels] = useState<ModelsView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft>({ values: {}, secrets: {} });
  const [restored, setRestored] = useState(false);
  const [openSecrets, setOpenSecrets] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [restartKeys, setRestartKeys] = useState<string[]>([]);
  const [lastApplied, setLastApplied] = useState<string[]>([]);
  // Keys that just went live, marked on their rows for a few seconds after a save.
  const [liveMarks, setLiveMarks] = useState<string[]>([]);
  const [restartDismissed, setRestartDismissed] = useState(false);
  const [echoBusy, setEchoBusy] = useState<"install" | "remove" | null>(null);
  const [echoMsg, setEchoMsg] = useState<{ tone: "ok" | "danger"; title: string; text: string; offer?: "ghost.mic" | "default" } | null>(null);
  const [active, setActive] = useState<SectionId | null>(null);
  // The optional keys' disclosure remounts open when something sends the operator to it.
  const [optionalOpen, setOptionalOpen] = useState(0);
  const sRef = useRef<SettingsView | null>(null);
  sRef.current = s;
  const layoutRef = useRef<HTMLDivElement>(null);
  const copyRef = useRef<HTMLButtonElement>(null);
  const lastFocus = useRef<HTMLElement | null>(null);
  const pendingFocus = useRef<(() => void) | null>(null);
  // Forced error states keep failing until the operator presses Try again (StrictMode runs loads twice).
  const forcedRetried = useRef({ settings: false, devices: false });

  const [entryErrors, setEntryErrors] = useState<Record<string, string | null>>({});
  const dirtyKeys = useMemo(() => changedKeys(draft, s), [draft, s]);
  const dirty = dirtyKeys.length > 0;
  const errors = useMemo(() => validate(draft, s), [draft, s]);
  // A chips entry that couldn't be added (typed, not yet a chip) blocks Save like any other error.
  const invalidKeys = VALUE_KEYS.filter((k) => (errors[k] && dirtyKeys.includes(k)) || !!entryErrors[k]);

  /* ---------------------------------------------------------------- loading */
  /** Devices and models. `after` runs in the same tick as the state updates, so both land in one render. */
  const loadAux = useCallback(async (after?: () => void) => {
    const failDevices = forced === "nodevices" && !forcedRetried.current.devices;
    const [dv, mv] = await Promise.all([
      (failDevices ? Promise.reject(new Error("PortAudio couldn’t open the sound server (pipewire).")) : client.devices()).then(
        (d) => {
          setDevicesError(null);
          return d;
        },
        (e) => {
          setDevicesError(errorText(e));
          return null;
        },
      ),
      client.models().catch(() => null),
    ]);
    // echoidle: installed but not running yet, the state Reinstall repairs.
    if (dv) setDevices(forced === "echoidle" ? { ...dv, echo_cancel_active: false, echo_cancel_source_present: true } : dv);
    if (mv) setModels(mv);
    after?.();
  }, [client, forced]);

  const loadAll = useCallback(async () => {
    try {
      if (forced === "loaderror" && !forcedRetried.current.settings) {
        throw new Error("Ghost didn’t answer GET /settings in time.");
      }
      const sv = await client.settings();
      setLoadError(null);
      // A reload while editing keeps the operator's edits on top of what the daemon says.
      // The first load brings back edits left behind when the operator went to another page.
      const first = !sRef.current;
      const kept = first && !forced ? readDraft() : null;
      setDraft((d) => {
        const prev = sRef.current;
        const values: Record<string, string> = { ...sv.values };
        if (prev) for (const k of VALUE_KEYS) if (valueChanged(k, prev.values[k], d.values[k])) values[k] = d.values[k] ?? "";
        if (kept) for (const [k, val] of Object.entries(kept)) if (valueChanged(k, sv.values[k], val)) values[k] = val;
        return { values, secrets: d.secrets };
      });
      if (kept && Object.entries(kept).some(([k, val]) => valueChanged(k, sv.values[k], val))) setRestored(true);
      setS(sv);
    } catch (e) {
      setLoadError(errorText(e));
    }
    await loadAux();
  }, [client, loadAux, forced]);

  useEffect(() => {
    if (forced === "loading") return;
    void loadAll();
  }, [loadAll, forced]);
  // Model readiness changes the models card and often the device list.
  useEffect(() => {
    if (s) void loadAux();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status?.models_ready]);

  // Ghost came back after being away (a restart): whatever waited for it has applied.
  const prevConnection = useRef(connection);
  useEffect(() => {
    const was = prevConnection.current;
    prevConnection.current = connection;
    if (connection === "online" && was !== "online" && was !== "connecting") {
      setRestartKeys([]);
      void loadAll();
    }
  }, [connection, loadAll]);

  // Keep non-secret edits for this window, so leaving the page doesn't lose them. Never secrets.
  useEffect(() => {
    if (!s || forced) return;
    const changed: Record<string, string> = {};
    for (const k of VALUE_KEYS) if (valueChanged(k, s.values[k], draft.values[k])) changed[k] = draft.values[k] ?? "";
    storeDraft(Object.keys(changed).length ? changed : null);
  }, [draft.values, s, forced]);
  useEffect(() => {
    if (!dirty) setRestored(false);
  }, [dirty]);

  // Focus moves that must wait for the render that mounts their target. A layout effect, so a
  // move that replaces a removed element happens before the browser paints that render.
  useLayoutEffect(() => {
    const f = pendingFocus.current;
    if (!f) return;
    pendingFocus.current = null;
    f();
  });

  useEffect(() => {
    if (!liveMarks.length) return;
    const t = window.setTimeout(() => setLiveMarks([]), 8000);
    return () => window.clearTimeout(t);
  }, [liveMarks]);

  /* ------------------------------------------------------- design-only states */
  const forcedApplied = useRef(false);
  useEffect(() => {
    if (!forced || !s || forcedApplied.current) return;
    forcedApplied.current = true;
    const v = { ...s.values };
    if (forced === "dirty" || forced === "error") {
      setDraft({
        values: { ...v, GHOST_WAKE_PHRASES: joinList([...splitList(v.GHOST_WAKE_PHRASES), "ghost wake up"]), GHOST_LLM_MODEL: "claude-opus-5-5" },
        secrets: { GITHUB_TOKEN: "github_pat_example_value" },
      });
      setOpenSecrets(new Set(["GITHUB_TOKEN"]));
      if (forced === "error") setSaveError("Ghost couldn’t write ~/.config/ghost/settings.env: permission denied. Check who owns the file, then save again.");
    } else if (forced === "invalid") {
      setDraft({ values: { ...v, ANTHROPIC_BASE_URL: "api.example.com" }, secrets: {} });
    } else if (forced === "restart") {
      setRestartKeys(["ANTHROPIC_BASE_URL", "GITHUB_TOKEN"]);
      setLastApplied(["GHOST_WAKE_PHRASES"]);
      setLiveMarks(["GHOST_WAKE_PHRASES"]);
    } else if (forced === "secret") {
      setOpenSecrets(new Set(["GITHUB_TOKEN"]));
    }
  }, [forced, s]);

  /* ------------------------------------------------------------- navigation */
  const go = useCallback((id: SectionId, opts: { smooth?: boolean; focus?: boolean } = {}) => {
    const el = document.getElementById(sectionDomId(id));
    const main = document.getElementById("main");
    if (!el || !main) return;
    const top = el.getBoundingClientRect().top - main.getBoundingClientRect().top + main.scrollTop - stickyBottom() - LAND_GAP;
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    main.scrollTo({ top: Math.max(0, top), behavior: opts.smooth !== false && !reduce ? "smooth" : "auto" });
    setActive(id);
    if (opts.focus !== false) {
      const h = document.getElementById(`${sectionDomId(id)}-title`);
      if (h) {
        h.tabIndex = -1;
        h.focus({ preventScroll: true });
      }
    }
  }, []);

  const focusKey = useCallback(
    (key: string) => {
      const section = KEY_SECTION[key];
      if (section) go(section, { focus: false });
      requestAnimationFrame(() => focusInto(document.getElementById(fieldId(key))));
    },
    [go],
  );

  /** Secrets, then the TypeSafe key: where "Add one under Secrets" leads. */
  const goToJevKey = useCallback(() => {
    setOptionalOpen((n) => n + 1);
    go("secrets", { focus: false });
    pendingFocus.current = () => requestAnimationFrame(() => focusInto(document.getElementById(secretActionId(isSet(sRef.current, "TYPESAFE_API_KEY") ? "OPENROUTER_API_KEY" : "TYPESAFE_API_KEY"))));
  }, [go]);

  // ?section=voice lands on that section once the page has its data, then keeps landing while
  // the column is still changing size (the device list, a late Notice, the lazy prototypes), until
  // the operator scrolls or presses a key themselves, or two seconds pass. The watch survives
  // re-runs of the effect (StrictMode, a reload) until one of those ends it.
  const landed = useRef(false);
  const landingUntil = useRef(0);
  useEffect(() => {
    const target = params.get("section") as SectionId | null;
    const main = document.getElementById("main");
    const layout = layoutRef.current;
    // Wait for the devices request to settle either way: the setup list and the audio card depend on it.
    if (!s || (!devices && !devicesError) || !target || !main || !layout) return;
    const land = () => go(target, { smooth: false, focus: false });
    if (!landed.current) {
      landed.current = true;
      landingUntil.current = Date.now() + 2000;
      requestAnimationFrame(land);
    }
    if (Date.now() >= landingUntil.current) return;
    const ro = new ResizeObserver(() => {
      if (Date.now() < landingUntil.current) land();
    });
    ro.observe(layout);
    const stop = () => {
      landingUntil.current = 0;
      ro.disconnect();
    };
    const events = ["wheel", "touchstart", "keydown", "pointerdown"];
    for (const ev of events) main.addEventListener(ev, stop, { passive: true });
    const timer = window.setTimeout(stop, Math.max(0, landingUntil.current - Date.now()));
    return () => {
      clearTimeout(timer);
      ro.disconnect();
      for (const ev of events) main.removeEventListener(ev, stop);
    };
  }, [s, devices, devicesError, params, go]);

  // Scroll spy: the last section whose top has passed under the compact bar. Re-runs whenever
  // the column changes size too, so a section that mounts late (setup, prototypes) is counted.
  useEffect(() => {
    const main = document.getElementById("main");
    const layout = layoutRef.current;
    if (!main || !s || !layout) return;
    let raf = 0;
    const update = () => {
      raf = 0;
      const sections = Array.from(layout.querySelectorAll<HTMLElement>("section.set-section"));
      if (!sections.length) return;
      const idOf = (el: HTMLElement) => el.id.replace(/^set-/, "") as SectionId;
      const top = main.getBoundingClientRect().top + stickyBottom() + LAND_GAP * 2;
      let current = idOf(sections[0]);
      for (const el of sections) if (el.getBoundingClientRect().top <= top) current = idOf(el);
      if (main.scrollTop > 0 && main.scrollTop + main.clientHeight >= main.scrollHeight - 2) current = idOf(sections[sections.length - 1]);
      setActive(current);
    };
    const schedule = () => {
      if (!raf) raf = requestAnimationFrame(update);
    };
    update();
    main.addEventListener("scroll", schedule, { passive: true });
    const ro = new ResizeObserver(schedule);
    ro.observe(layout);
    return () => {
      main.removeEventListener("scroll", schedule);
      ro.disconnect();
      if (raf) cancelAnimationFrame(raf);
    };
  }, [s, design]);

  /* ------------------------------------------------------------------ focus */
  // The last field the operator was in, so Save and Discard can put them back there.
  const onFocusIn = (e: FocusEvent<HTMLDivElement>) => {
    const t = e.target as HTMLElement;
    if (t.id && !t.closest(".set-nav")) lastFocus.current = t;
  };
  /** Where focus should go back to after the bar's action: the field in use, else the last one. */
  const returnTarget = (): HTMLElement | null => {
    const a = document.activeElement as HTMLElement | null;
    if (a && layoutRef.current?.contains(a) && !a.closest(".set-nav")) return a;
    return lastFocus.current ?? (dirtyKeys[0] ? document.getElementById(fieldId(dirtyKeys[0])) ?? document.getElementById(secretActionId(dirtyKeys[0])) : null);
  };
  /** Back to `el`; a secret's field that has closed hands over to its Replace… button. */
  const focusBack = (el: HTMLElement | null) => {
    if (!el) return;
    requestAnimationFrame(() => {
      if (el.isConnected && focusInto(el)) return;
      const m = /^set-secret-(.+)$/.exec(el.id);
      if (m) document.getElementById(secretActionId(m[1]))?.focus({ preventScroll: true });
      else if (el.id) focusInto(document.getElementById(el.id));
    });
  };

  /* ------------------------------------------------------------------ edits */
  const setValue = (k: string, v: string) => {
    setSaveError(null);
    setDraft((d) => ({ ...d, values: { ...d.values, [k]: v } }));
  };
  const setSecret = (k: string, v: string) => {
    setSaveError(null);
    setDraft((d) => ({ ...d, secrets: { ...d.secrets, [k]: v } }));
  };
  const closeSecret = (k: string) => {
    setOpenSecrets((o) => {
      const n = new Set(o);
      n.delete(k);
      return n;
    });
    setDraft((d) => {
      const { [k]: _, ...rest } = d.secrets;
      return { ...d, secrets: rest };
    });
  };
  const discard = () => {
    if (!s) return;
    const back = returnTarget();
    const before = draft.values;
    const valueKeys = dirtyKeys.filter((k) => !(k in draft.secrets));
    const typed = dirtyKeys.length - valueKeys.length;
    setDraft({ values: { ...s.values }, secrets: {} });
    setOpenSecrets(new Set());
    setEntryErrors({});
    setSaveError(null);
    storeDraft(null);
    pendingFocus.current = () => focusBack(back);
    const n = dirtyKeys.length;
    toast({
      title: `Discarded ${n} change${n === 1 ? "" : "s"}`,
      description: typed ? `A typed secret isn’t kept; the other${valueKeys.length === 1 ? "" : "s"} can come back.` : undefined,
      action: valueKeys.length
        ? {
            label: "Undo",
            onClick: () => {
              setDraft((d) => ({ ...d, values: { ...d.values, ...Object.fromEntries(valueKeys.map((k) => [k, before[k] ?? ""])) } }));
              pendingFocus.current = () => requestAnimationFrame(() => focusInto(document.getElementById(fieldId(valueKeys[0]))));
            },
          }
        : undefined,
    });
  };

  /* ------------------------------------------------------------------- save */
  const save = useCallback(async () => {
    if (!s || busy || !online) return;
    if (invalidKeys.length) return focusKey(invalidKeys[0]);
    const changedValues: Record<string, string | null> = {};
    for (const k of VALUE_KEYS) {
      const v = (draft.values[k] ?? "").trim();
      if (valueChanged(k, s.values[k], draft.values[k])) changedValues[k] = v === "" ? null : v;
    }
    const changedSecrets: Record<string, string | null> = {};
    for (const [k, v] of Object.entries(draft.secrets)) if (v) changedSecrets[k] = v;
    if (!Object.keys(changedValues).length && !Object.keys(changedSecrets).length) return;
    const keys = [...Object.keys(changedValues), ...Object.keys(changedSecrets)];
    const fromBar = !!(document.activeElement as HTMLElement | null)?.closest(".g-savebar");
    const back = returnTarget();
    setBusy(true);
    setSaveError(null);
    try {
      const r = await client.updateSettings({
        values: Object.keys(changedValues).length ? changedValues : null,
        secrets: Object.keys(changedSecrets).length ? changedSecrets : null,
      });
      setS(r.settings);
      setDraft({ values: { ...r.settings.values }, secrets: {} });
      setOpenSecrets(new Set());
      storeDraft(null);
      const applied = r.applied ?? [];
      const waiting = keys.filter((k) => !applied.includes(k));
      setLastApplied(applied);
      setLiveMarks(applied);
      if (r.restart_required) {
        setRestartKeys((prev) => Array.from(new Set([...prev, ...waiting])));
        setRestartDismissed(false);
      } else {
        toast({
          title: "Saved",
          description: applied.length ? `Live now: ${lowerFirst(listWords(applied))}.` : "Ghost uses the new settings now.",
          tone: "ok",
        });
      }
      // From the bar, the restart bar's Copy is the next thing to do; from a field, stay there.
      pendingFocus.current = r.restart_required && fromBar ? () => requestAnimationFrame(() => copyRef.current?.focus({ preventScroll: true })) : () => focusBack(back);
    } catch (e) {
      setSaveError(errorText(e));
    } finally {
      setBusy(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s, busy, online, invalidKeys.join(), focusKey, draft, client, toast]);

  // Enter in any field saves, the same as Save.
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void save();
  };

  // Ctrl+S saves while there is something to save.
  useEffect(() => {
    if (!dirty) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dirty, save]);

  /* --------------------------------------------------------- leaving the page */
  // Non-secret edits survive leaving (sessionStorage); a typed secret can't, so leaving by a
  // link (the sidebar, a status chip) asks first, and closing the window warns. Until the router
  // offers a before-leave hook this catches link clicks; Back and typed hashes aren't covered.
  const typedSecrets = Object.entries(draft.secrets).filter(([, val]) => !!val).map(([k]) => k);
  const typedKey = typedSecrets.join();
  useEffect(() => {
    if (!typedKey) return;
    const names = typedKey.split(",");
    const onClick = (e: MouseEvent) => {
      if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
      const a = (e.target as Element | null)?.closest?.("a[href^='#/']");
      const to = a?.getAttribute("href")?.slice(1);
      if (!a || !to || to === "/settings" || layoutRef.current?.contains(a)) return;
      e.preventDefault();
      e.stopPropagation();
      const what = names.length === 1 ? `the ${lowerFirst(listWords(names))}` : `${names.length} secrets`;
      void confirm({
        title: `Leave without saving ${what}?`,
        description: "Ghost never keeps a secret you haven’t saved, so you’d paste it again. Your other changes stay here until you come back.",
        confirmLabel: "Leave without saving",
        cancelLabel: "Stay",
        tone: "danger",
      }).then((ok) => {
        if (ok) navigate(to);
      });
    };
    const onUnload = (e: BeforeUnloadEvent) => e.preventDefault();
    document.addEventListener("click", onClick, true);
    window.addEventListener("beforeunload", onUnload);
    return () => {
      document.removeEventListener("click", onClick, true);
      window.removeEventListener("beforeunload", onUnload);
    };
  }, [typedKey, confirm]);

  /* ------------------------------------------------------- echo cancellation */
  const echo = async (uninstall: boolean) => {
    if (
      uninstall &&
      !(await confirm({
        title: "Remove echo cancellation?",
        description: "Without it, Ghost can hear its own voice through your speakers and answer itself. You can install it again any time.",
        confirmLabel: "Remove echo cancellation",
        tone: "danger",
      }))
    )
      return;
    setEchoBusy(uninstall ? "remove" : "install");
    setEchoMsg(null);
    let msg: NonNullable<typeof echoMsg>;
    try {
      const r = await client.setupEchoCancel(uninstall);
      msg = r.ok
        ? uninstall
          ? { tone: "ok", title: "Echo cancellation removed", text: "Restart Ghost to apply.", offer: "default" }
          : { tone: "ok", title: "Echo cancellation installed", text: "Use ghost.mic as the microphone, save, and restart Ghost.", offer: "ghost.mic" }
        : { tone: "danger", title: uninstall ? "Couldn’t remove echo cancellation" : "Couldn’t install echo cancellation", text: r.detail ?? "Ghost didn’t say why. Run journalctl --user -u ghostd to see the log." };
    } catch (e) {
      msg = { tone: "danger", title: "Couldn’t reach Ghost", text: errorText(e) };
    }
    // The row stays "Removing…" / "Installing…" until the device list has reloaded; then the
    // row, the result and the focus move all land in one render, so the page never contradicts itself.
    const settle = () => {
      pendingFocus.current = focusAfterEcho;
      setEchoMsg(msg);
      setEchoBusy(null);
    };
    try {
      await loadAux(settle);
    } catch {
      settle();
    }
  };

  /**
   * After Install or Remove the button that started it is usually gone. If focus went with it,
   * hand it to the result's next step (Use ghost.mic), else the row's new button, else the
   * result's Dismiss. Focus the operator moved elsewhere in the meantime is left alone.
   */
  const focusAfterEcho = () => {
    // Still somewhere real: the same button after a failed attempt, or wherever the operator went.
    const a = document.activeElement as HTMLElement | null;
    if (a && a !== document.body && a.isConnected) return;
    for (const id of [ECHO_IDS.offer, ECHO_IDS.install, ECHO_IDS.reinstall, ECHO_IDS.more]) {
      const el = document.getElementById(id);
      if (el && !el.hasAttribute("disabled") && focusInto(el)) return;
    }
    document.querySelector<HTMLElement>(`#${ECHO_IDS.msg} .g-notice__dismiss`)?.focus({ preventScroll: true });
  };
  /** The result's next step changes the microphone; focus follows to the microphone it set. */
  const pickMic = (value: string) => {
    setValue("GHOST_INPUT_DEVICE", value);
    pendingFocus.current = () => requestAnimationFrame(() => focusInto(document.getElementById(fieldId("GHOST_INPUT_DEVICE"))));
  };

  /* ---------------------------------------------------------------- derived */
  const v = draft.values;
  const mock = !!design;
  const ready = !!s && modelReady(s, v);
  // Until both basics are done, the page leads with them.
  const showSetup = !!s && !!devices && !(modelReady(s, s.values) && echoReady(devices));
  const restartSet = new Set(restartKeys);

  const sectionMark = (id: SectionId): NavItem["mark"] => {
    const keys = Object.entries(KEY_SECTION).filter(([, sec]) => sec === id).map(([k]) => k);
    const secretKeys = id === "secrets" ? SECRET_ORDER : [];
    const all = [...keys, ...secretKeys];
    if (all.some((k) => errors[k] && dirtyKeys.includes(k))) return "error";
    if (all.some((k) => dirtyKeys.includes(k))) return "dirty";
    if (all.some((k) => restartSet.has(k))) return "restart";
    if (id === "audio" && micDiffers(devices, s?.values.GHOST_INPUT_DEVICE)) return "restart";
    return undefined;
  };
  const navItems: NavItem[] = [
    ...(showSetup ? [{ id: "setup" as const, label: SECTION_TITLE.setup }] : []),
    ...(["secrets", "model", "github", "lines", "skills", "voice", "audio", "models", "phone"] as SectionId[]).map((id) => ({ id, label: SECTION_TITLE[id], mark: sectionMark(id) })),
    ...(mock && Prototype ? (["privacy", "sounds"] as SectionId[]).map((id) => ({ id, label: SECTION_TITLE[id], prototype: true })) : []),
  ];

  const steps: SetupStep[] = s
    ? [
        {
          id: "model",
          title: "Give Ghost a model",
          done: modelReady(s, s.values),
          doneText: isSet(s, "ANTHROPIC_AUTH_TOKEN") ? "Anthropic token set." : "Using your own model endpoint.",
          todoText: "Add an Anthropic token, or point Ghost at your own endpoint.",
          target: isSet(s, "ANTHROPIC_AUTH_TOKEN") || !s.values.ANTHROPIC_BASE_URL ? "secrets" : "model",
        },
        {
          id: "echo",
          title: "Install echo cancellation",
          done: echoReady(devices),
          doneText: "Ghost hears you, not itself.",
          todoText: "So Ghost doesn’t hear its own voice through your speakers.",
          target: "audio",
        },
      ]
    : [];

  // The microphone saved before this visit but not yet opened by Ghost owes a restart too; the
  // pill carries the command even when nothing was saved on this page.
  // Dismissing that pill is remembered per saved device for this window's life (sessionStorage),
  // so it doesn't come back on every visit; a different saved device brings it back.
  const micWaits = !!s && micDiffers(devices, s.values.GHOST_INPUT_DEVICE);
  const micDismissed = !!s && micDismissedFor(s.values.GHOST_INPUT_DEVICE);
  const barState: BarState | null = dirty
    ? { kind: "dirty", keys: dirtyKeys, online, busy, error: saveError, invalid: invalidKeys, restored }
    : restartKeys.length && !restartDismissed
      ? { kind: "restart", waiting: restartKeys, live: lastApplied }
      : micWaits && !restartDismissed && !micDismissed
        ? { kind: "restart", waiting: ["GHOST_INPUT_DEVICE"], live: [] }
        : null;
  const dismissRestart = () => {
    setRestartDismissed(true);
    if (micWaits && s) storeMicDismissed(s.values.GHOST_INPUT_DEVICE ?? "");
  };

  // A key saved but not yet live says so first, in its own help line, until Ghost restarts.
  const waits = (k: string, text: ReactNode) =>
    restartSet.has(k) && !dirtyKeys.includes(k) ? (
      <>
        <span className="set-waits">Waits for a restart.</span> {text}
      </>
    ) : liveMarks.includes(k) && !dirtyKeys.includes(k) ? (
      <>
        <span className="set-live">Live now.</span> {text}
      </>
    ) : (
      text
    );
  const help = (k: string, text: ReactNode) => (errors[k] && dirtyKeys.includes(k) ? undefined : entryErrors[k] ? undefined : waits(k, text));
  const err = (k: string) => entryErrors[k] ?? (dirtyKeys.includes(k) ? errors[k] : undefined);
  // Jev's "needs a key" sentence carries its own way there.
  const jevWords = (text: string | undefined): ReactNode =>
    text === JEV_NEEDS_KEY ? (
      <>
        Jev needs a TypeSafe or OpenRouter key.{" "}
        <button type="button" className="set-link" onClick={goToJevKey}>
          Add one under Secrets
        </button>
        , or choose Automatic.
      </>
    ) : (
      text
    );

  /* ----------------------------------------------------------------- render */
  const phrases = splitList(v.GHOST_WAKE_PHRASES);
  const storedFuzzy = s?.values[FUZZY] ?? "";
  const fuzzy = parseFuzzy(v[FUZZY]);
  const exact = (v[FUZZY] ?? "").trim() !== "" && fuzzy === 0;
  const step = Math.round((1 - Math.max(0.5, exact ? 0.8 : fuzzy)) / 0.05);
  // Moving back to the saved level writes the saved text back, so no phantom change is left.
  const setFuzzy = (n: number) => setValue(FUZZY, parseFuzzy(storedFuzzy) === n && parseFuzzy(storedFuzzy) > 0 ? storedFuzzy : n.toFixed(2));
  const jev = v.GHOST_JEV || "auto";
  const jevStatus = jevNow(jev, s, draft.secrets);
  const crossAnswers = truthy(v.GHOST_CROSS_DEPARTMENT_ANSWERS);
  const routing = truthy(v.GHOST_DEPARTMENT_ROUTING);
  const mics = micOptions(devices, v.GHOST_INPUT_DEVICE ?? "");
  const secretNames = s
    ? SECRET_ORDER.filter((n) => s.secrets.some((x) => x.name === n)).concat(s.secrets.map((x) => x.name).filter((n) => !SECRET_ORDER.includes(n)))
    : [];
  const requiredSecrets = secretNames.filter((n) => !isOptionalSecret(n));
  const optionalSecrets = secretNames.filter(isOptionalSecret);
  const optionalSet = optionalSecrets.filter((n) => isSet(s, n)).length;
  const optionalBusy = optionalSecrets.some((n) => openSecrets.has(n) || restartSet.has(n));
  const secretRow = (name: string) => (
    <SecretRow
      key={name}
      name={name}
      set={isSet(s, name)}
      value={draft.secrets[name]}
      open={openSecrets.has(name)}
      onOpen={() => setOpenSecrets((o) => new Set(o).add(name))}
      onChange={(val) => setSecret(name, val)}
      onCancel={() => closeSecret(name)}
      onSubmit={submit}
      disabled={!online}
      waiting={restartSet.has(name)}
    />
  );

  return (
    <Page
      width="narrow"
      title="Settings"
      status={<LiveStatus />}
      description={
        <>
          How Ghost connects, listens and speaks. Saved to <span className="set-path">{tildePath(s?.settings_path) || "~/.config/ghost/settings.env"}</span>.
        </>
      }
      dock={
        barState ? (
          <SaveBar state={barState} copyRef={copyRef} onSave={() => void save()} onDiscard={discard} onDismiss={dismissRestart} onFix={focusKey} />
        ) : undefined
      }
      dockFirst
      className="set-page"
    >
      {dialog}
      <div className="set-layout" ref={layoutRef} onFocus={onFocusIn}>
        {!s && loadError && (
          <Notice
            tone="danger"
            title="Couldn’t load settings"
            action={
              <Button
                size="sm"
                onClick={() => {
                  forcedRetried.current.settings = true;
                  void loadAll();
                }}
              >
                Try again
              </Button>
            }
          >
            {loadError} Check that Ghost is running, then try again.
          </Notice>
        )}
        {!s && !loadError && <SettingsSkeleton />}

        {s && (
          <>
            {showSetup && <SetupSection steps={steps} onGo={(id) => go(id)} />}

            {/* --------------------------------------------------------- Secrets */}
            <Section
              id={sectionDomId("secrets")}
              title={SECTION_TITLE.secrets}
              description={
                <>
                  Kept in <span className="set-path">{tildePath(s.env_path)}</span>. Never shown again once saved.
                </>
              }
              className="set-section"
            >
              {/* Plain help, not a Notice: the section already shows a tinted block when setup is unfinished or a restart is owed. */}
              {s.vault_managed && <p className="set-help is-readable set-vault-note">Filled from your vault each time Ghost starts. A value you save here lasts until the next start.</p>}
              <Card list>
                {requiredSecrets.map(secretRow)}
                {optionalSecrets.length > 0 && (
                  <div className="set-optional-keys">
                    <Disclosure
                      key={optionalOpen}
                      defaultOpen={optionalOpen > 0 || optionalBusy}
                      summary={
                        <span className="set-optional-keys__summary">
                          Optional keys
                          <span className="set-count t-num">
                            {optionalSet} of {optionalSecrets.length} set
                          </span>
                        </span>
                      }
                    >
                      <p className="set-help set-optional-keys__lead">Jev hears your yes with TypeSafe or OpenRouter. Hugging Face speeds up the first model download.</p>
                      <div className="set-optional-keys__list">{optionalSecrets.map(secretRow)}</div>
                    </Disclosure>
                  </div>
                )}
              </Card>
            </Section>

            <form onSubmit={submit} autoComplete="off" className="set-form">
              {/* ----------------------------------------------------------- Model */}
              <Section
                id={sectionDomId("model")}
                title={SECTION_TITLE.model}
                description="Where Ghost sends a session to think. Leave both empty to use Anthropic’s defaults."
                className="set-section"
              >
                <Card list>
                  <Row>
                    <Field layout="inline" id={fieldId("ANTHROPIC_BASE_URL")} label="Model endpoint" detail="ANTHROPIC_BASE_URL" help={help("ANTHROPIC_BASE_URL", "Any Anthropic-compatible URL.")} error={err("ANTHROPIC_BASE_URL")}>
                      <Input value={v.ANTHROPIC_BASE_URL ?? ""} placeholder="https://api.anthropic.com" inputMode="url" spellCheck={false} onChange={(e) => setValue("ANTHROPIC_BASE_URL", e.target.value)} />
                    </Field>
                  </Row>
                  <Row>
                    <Field layout="inline" id={fieldId("GHOST_LLM_MODEL")} label="Model" detail="GHOST_LLM_MODEL" help={help("GHOST_LLM_MODEL", `Empty means ${DEFAULT_MODEL}.`)}>
                      <Input mono value={v.GHOST_LLM_MODEL ?? ""} placeholder={DEFAULT_MODEL} spellCheck={false} onChange={(e) => setValue("GHOST_LLM_MODEL", e.target.value)} />
                    </Field>
                  </Row>
                </Card>
                {!ready && (
                  <p className="set-note">
                    Ghost can’t answer until it has an Anthropic token or an endpoint.{" "}
                    <button type="button" className="set-link" onClick={() => go("secrets")}>
                      Add a token
                    </button>
                  </p>
                )}
              </Section>

              {/* ---------------------------------------------------------- GitHub */}
              <Section
                id={sectionDomId("github")}
                title={SECTION_TITLE.github}
                description="GitHub is an MCP server declared in your policy file, not a setting here (ADR 0022)."
                className="set-section"
              >
                <Card padding="md">
                  <p className="set-github t-body">
                    Its declaration under <span className="set-detail">[mcp.github]</span> in <span className="set-detail">~/.config/ghost/policy.toml</span> names the
                    command Ghost runs, the repositories it may touch and the writes Ghost may make. The GitHub token above is what it signs in with; one token per
                    server, so a second organization is a second server.
                  </p>
                  <div className="set-github__links">
                    <Button size="sm" variant="secondary" href={hrefFor("/servers")} trailingIcon={<ArrowRight />}>
                      Open Servers
                    </Button>
                    <Button size="sm" variant="ghost" href={`${window.location.pathname}?policy=toml#/policy`}>
                      Edit the policy file
                    </Button>
                  </div>
                </Card>
                {!isSet(s, "GITHUB_TOKEN") && (
                  <p className="set-note">
                    Ghost has no GitHub token, so a GitHub server can’t sign in.{" "}
                    <button type="button" className="set-link" onClick={() => go("secrets")}>
                      Add a token
                    </button>
                  </p>
                )}
              </Section>

              {/* ----------------------------------------------------------- Lines */}
              <Section
                id={sectionDomId("lines")}
                title={SECTION_TITLE.lines}
                description="What a scoped line does when you ask about something outside its scope. It never changes anything outside it, whatever you choose here."
                className="set-section"
              >
                <Card list>
                  <Row className="set-practice">
                    <Checkbox
                      id={fieldId("GHOST_CROSS_DEPARTMENT_ANSWERS")}
                      checked={crossAnswers}
                      onChange={(on) => setValue("GHOST_CROSS_DEPARTMENT_ANSWERS", on ? "1" : "0")}
                      label={
                        <span className="set-practice__label">
                          Cross-department answers <span className="set-detail">GHOST_CROSS_DEPARTMENT_ANSWERS</span>
                        </span>
                      }
                      description={waits("GHOST_CROSS_DEPARTMENT_ANSWERS", "A line may look things up outside its scope to answer you. It never changes them.")}
                    />
                  </Row>
                  <Row className="set-practice">
                    <Checkbox
                      id={fieldId("GHOST_DEPARTMENT_ROUTING")}
                      checked={routing}
                      onChange={(on) => setValue("GHOST_DEPARTMENT_ROUTING", on ? "1" : "0")}
                      label={
                        <span className="set-practice__label">
                          Department routing <span className="set-detail">GHOST_DEPARTMENT_ROUTING</span>
                        </span>
                      }
                      description={waits("GHOST_DEPARTMENT_ROUTING", "When a request belongs to another line, Ghost patches you through to it instead of telling you where it belongs.")}
                    />
                  </Row>
                </Card>
                <p className="set-note">
                  Both off: Ghost says which line it belongs to and offers to switch. Ghost itself is never scoped.
                </p>
              </Section>

              {/* ----------------------------------------------------------- Skills */}
              <Section
                id={sectionDomId("skills")}
                title={SECTION_TITLE.skills}
                description="Where Ghost finds skills: the SKILL.md folders that tell it how to do a kind of task."
                className="set-section"
              >
                <Card list>
                  <Row>
                    <Field
                      id={fieldId("GHOST_SKILL_ROOTS")}
                      label="Project skill roots"
                      detail="GHOST_SKILL_ROOTS"
                      help={help(
                        "GHOST_SKILL_ROOTS",
                        <>
                          One folder per line. Ghost reads each one’s <span className="t-mono">.agents/skills</span> in place, without copying. Its own skills live in{" "}
                          <span className="t-mono">~/.config/ghost/skills</span>.
                        </>,
                      )}
                      error={err("GHOST_SKILL_ROOTS")}
                    >
                      <LinesInput value={v.GHOST_SKILL_ROOTS ?? ""} placeholder="~/projects/heka" onChange={(next) => setValue("GHOST_SKILL_ROOTS", next)} />
                    </Field>
                  </Row>
                </Card>
                <p className="set-note">
                  What Ghost found, and what each skill needs, is under <a href={hrefFor("/skills")}>Skills</a>.
                </p>
              </Section>

              {/* -------------------------------------------------- Voice and wake */}
              <Section
                id={sectionDomId("voice")}
                title={SECTION_TITLE.voice}
                description="What wakes Ghost and how it hears your yes."
                className="set-section"
              >
                <Card list>
                  <Row>
                    <Field
                      id={fieldId("GHOST_WAKE_PHRASES")}
                      label="Wake phrases"
                      detail="GHOST_WAKE_PHRASES"
                      help={help("GHOST_WAKE_PHRASES", phrases.length ? "Any of these starts a session. Two or three words work best." : "Empty means “hey ghost”.")}
                      error={err("GHOST_WAKE_PHRASES")}
                    >
                      <ChipsInput
                        kind="phrase"
                        noun="phrase"
                        items={phrases}
                        placeholder="hey ghost"
                        check={(e) => (e.split(/\s+/).length > 5 ? "Keep a wake phrase to five words or fewer." : null)}
                        onEntryError={(m) => setEntryErrors((x) => ({ ...x, GHOST_WAKE_PHRASES: m }))}
                        onChange={(items) => setValue("GHOST_WAKE_PHRASES", joinList(items))}
                      />
                    </Field>
                  </Row>
                  <Row>
                    <Field
                      layout="inline"
                      id={fieldId(FUZZY)}
                      label="Wake sensitivity"
                      detail={FUZZY}
                      help={help(FUZZY, exact ? "Ghost wakes only on the exact words." : "Forgiving catches a mumble; strict wakes less by accident.")}
                    >
                      <WakeSensitivity
                        step={step}
                        exact={exact}
                        onStep={(n) => setFuzzy(stepToFuzzy(n))}
                        onExact={(on) => {
                          if (on) setValue(FUZZY, parseFuzzy(storedFuzzy) === 0 && storedFuzzy.trim() !== "" ? storedFuzzy : "0");
                          else setValue(FUZZY, storedFuzzy.trim() === "" || parseFuzzy(storedFuzzy) > 0 ? storedFuzzy : "0.80");
                        }}
                      />
                    </Field>
                  </Row>
                  <Row>
                    <Field
                      layout="inline"
                      id={fieldId("GHOST_JEV")}
                      label="How Ghost hears your yes"
                      detail="GHOST_JEV"
                      help={help("GHOST_JEV", jevWords(jevStatus.label))}
                      error={jevWords(err("GHOST_JEV") ?? undefined) || undefined}
                    >
                      <JevControl value={jev} onChange={(val) => setValue("GHOST_JEV", val === "auto" && !s.values.GHOST_JEV ? "" : val)} />
                    </Field>
                  </Row>
                </Card>
                <p className="set-promise-line">
                  <ShieldCheck size={14} aria-hidden />
                  Until you say a wake phrase, nothing you say leaves this computer.
                </p>
              </Section>

              {/* ------------------------------------------- Microphone and speakers */}
              <Section
                id={sectionDomId("audio")}
                title={SECTION_TITLE.audio}
                description="What Ghost hears with and speaks through."
                className="set-section"
              >
                {echoMsg && (
                  <div id={ECHO_IDS.msg}>
                  <Notice
                    tone={echoMsg.tone}
                    title={echoMsg.title}
                    onDismiss={() => setEchoMsg(null)}
                    action={
                      echoMsg.offer === "ghost.mic" && v.GHOST_INPUT_DEVICE !== "ghost.mic" ? (
                        <Button size="sm" id={ECHO_IDS.offer} onClick={() => pickMic("ghost.mic")}>
                          Use ghost.mic
                        </Button>
                      ) : echoMsg.offer === "default" && v.GHOST_INPUT_DEVICE === "ghost.mic" ? (
                        <Button size="sm" id={ECHO_IDS.offer} onClick={() => pickMic("")}>
                          Use system default
                        </Button>
                      ) : undefined
                    }
                  >
                    {echoMsg.offer === "default" && v.GHOST_INPUT_DEVICE === "ghost.mic" ? "Ghost is still set to ghost.mic, which is gone now. Pick another microphone, save, and restart Ghost." : echoMsg.text}
                  </Notice>
                  </div>
                )}
                {devicesError && !devices && (
                  <Notice
                    tone="danger"
                    title="Couldn’t list audio devices"
                    action={
                      <Button
                        size="sm"
                        onClick={() => {
                          forcedRetried.current.devices = true;
                          void loadAux();
                        }}
                      >
                        Try again
                      </Button>
                    }
                  >
                    {devicesError}
                  </Notice>
                )}
                <Card list>
                  <Row>
                    <Field
                      layout="inline"
                      id={fieldId("GHOST_INPUT_DEVICE")}
                      label="Microphone"
                      detail="GHOST_INPUT_DEVICE"
                      help={
                        devices ? (
                          <>
                            {(micWaits || (restartSet.has("GHOST_INPUT_DEVICE") && !dirtyKeys.includes("GHOST_INPUT_DEVICE"))) && (
                              <span className="set-waits">Waits for a restart. </span>
                            )}
                            In use now: {micInUse(devices)}.
                            {devices.echo_cancel_source_present && v.GHOST_INPUT_DEVICE !== "ghost.mic" ? " Pick ghost.mic so Ghost doesn’t hear itself." : ""}
                          </>
                        ) : (
                          "Type a device name, like ghost.mic."
                        )
                      }
                    >
                      {devices ? (
                        <Select value={v.GHOST_INPUT_DEVICE ?? ""} onChange={(e) => setValue("GHOST_INPUT_DEVICE", e.target.value)} options={mics} />
                      ) : (
                        <Input value={v.GHOST_INPUT_DEVICE ?? ""} placeholder="ghost.mic" spellCheck={false} onChange={(e) => setValue("GHOST_INPUT_DEVICE", e.target.value)} />
                      )}
                    </Field>
                  </Row>
                  <ListRow
                    title="Speakers"
                    description="Ghost uses the system output. Change it in your sound settings."
                    meta={<span className="set-value">{devices?.output_device ?? "System default"}</span>}
                  />
                  <ListRow
                    title="Echo cancellation"
                    description={
                      echoBusy
                        ? echoBusy === "remove"
                          ? "Removing echo cancellation…"
                          : "Installing echo cancellation…"
                        : !devices
                        ? "Checked once the device list loads."
                        : devices.echo_cancel_active
                          ? "Running. Ghost hears you, not its own voice."
                          : devices.echo_cancel_source_present
                            ? (
                              <>
                                Installed, not running yet. Restart Ghost to start it, or reinstall it.
                                <CommandCopy className="set-row-command" />
                              </>
                            )
                            : "Not installed. Without it, Ghost can hear itself through your speakers."
                    }
                    meta={
                      echoBusy ? undefined : devices ? (
                        devices.echo_cancel_active ? (
                          <Badge tone="ok" size="sm" dot>Running</Badge>
                        ) : (
                          <Badge size="sm">{devices.echo_cancel_source_present ? "Installed" : "Off"}</Badge>
                        )
                      ) : undefined
                    }
                    trailing={
                      devices?.echo_cancel_active || devices?.echo_cancel_source_present ? (
                        <span className="set-echo-actions">
                          {!devices.echo_cancel_active && (
                            <Button
                              id={ECHO_IDS.reinstall}
                              size="sm"
                              variant="secondary"
                              loading={echoBusy === "install"}
                              disabled={!online || (!!echoBusy && echoBusy !== "install")}
                              onClick={() => echo(false)}
                            >
                              Reinstall
                            </Button>
                          )}
                          {/* Remove is rare and destructive: behind More, red only inside the menu, confirmed in a dialog. */}
                          <Menu
                            align="end"
                            trigger={
                              <IconButton
                                id={ECHO_IDS.more}
                                size="sm"
                                variant="ghost"
                                label="More for echo cancellation"
                                icon={<MoreHorizontal />}
                                disabled={!online || !!echoBusy}
                              />
                            }
                            items={[{ label: "Remove echo cancellation…", icon: <Trash2 />, danger: true, onSelect: () => void echo(true) }]}
                          />
                        </span>
                      ) : (
                        <Button
                          id={ECHO_IDS.install}
                          size="sm"
                          variant="secondary"
                          loading={echoBusy === "install"}
                          disabled={!online || !devices || (!!echoBusy && echoBusy !== "install")}
                          onClick={() => echo(false)}
                        >
                          Install
                        </Button>
                      )
                    }
                  />
                  {devices && (
                  <div className="set-devices">
                      <Disclosure
                        summary={
                          <span className="set-devices__summary">
                            Detected devices <span className="set-count t-num">{devices?.devices.length ?? 0}</span>
                          </span>
                        }
                      >
                        <Table density="compact" className="set-device-table">
                          <thead>
                            <tr>
                              <th>Device</th>
                              <th className="is-num">In</th>
                              <th className="is-num">Out</th>
                              <th className="is-num">Rate</th>
                            </tr>
                          </thead>
                          <tbody>
                            {devices?.devices.map((d) => (
                              <tr key={d.index}>
                                <td className="is-mono">
                                  {d.name}
                                  {(d.is_default_input || d.is_default_output) && (
                                    <span className="set-device-default">
                                      {d.is_default_input && d.is_default_output ? "default in and out" : d.is_default_input ? "default in" : "default out"}
                                    </span>
                                  )}
                                </td>
                                <td className="is-num is-muted">{d.max_input_channels || "–"}</td>
                                <td className="is-num is-muted">{d.max_output_channels || "–"}</td>
                                <td className="is-num is-muted is-nowrap">{(d.default_samplerate / 1000).toFixed(1)} kHz</td>
                              </tr>
                            ))}
                          </tbody>
                        </Table>
                      </Disclosure>
                  </div>
                  )}
                </Card>
              </Section>
              {/* Lets Enter in any field submit; the visible Save lives in the dock. */}
              <button type="submit" className="sr-only" tabIndex={-1} aria-hidden />
            </form>

            {/* ------------------------------------------------------ Voice models */}
            <Section
              id={sectionDomId("models")}
              title={SECTION_TITLE.models}
              description="Speech turns into text on this computer. Audio never leaves it."
              className="set-section"
            >
              <Card list>
                {models && !models.ready && (
                  <Row className="set-loading">
                    <p className="set-label">Loading voice models…</p>
                    <Progress label="Loading voice models" />
                    <p className="set-help">Ghost can’t hear its wake phrase until this finishes. The first start can take a few minutes.</p>
                  </Row>
                )}
                <ListRow
                  title="Speaking voice"
                  description="Kokoro, on this computer."
                  meta={
                    !models ? undefined : models.kokoro_present ? (
                      <Badge tone="ok" size="sm" dot>Downloaded</Badge>
                    ) : !models.ready ? (
                      <Badge tone="warn" size="sm" dot>Downloading…</Badge>
                    ) : (
                      <Badge size="sm">Downloads on first start</Badge>
                    )
                  }
                />
                <ListRow title="Speech to text" description="Parakeet, on this computer." meta={<span className="set-value is-mono">{models?.stt_model ?? "–"}</span>} />
                <ListRow title="Model cache" description="Delete this folder to download the models again." meta={<span className="set-value is-mono">{models ? tildePath(models.cache_dir) : "–"}</span>} />
              </Card>
            </Section>

            {/* ------------------------------------------------------------ Phone */}
            <PhoneSection settingsPath={s.settings_path} openOnLoad={forced === "pairing"} />

            {mock && Prototype && (
              <Suspense fallback={null}>
                <Prototype />
              </Suspense>
            )}
          </>
        )}
        {/* After the content in the DOM, so Tab reaches the fields first; it sits to their right. */}
        {s && <PageNav items={navItems} active={active} onGo={(id) => go(id)} />}
      </div>
    </Page>
  );
}

/** A slider step (0 = strict, 10 = forgiving) back to the threshold it stands for. */
const stepToFuzzy = (n: number) => Math.round((1 - n * 0.05) * 100) / 100;

/**
 * The wake slider and its "Exact words only" escape hatch. Only the slider belongs to the
 * Field (its label and help); the checkbox has its own id and label. The readout is the word
 * for the level (Strict, Balanced, Forgiving), inline like every other slider on the page, and
 * the same word is what the slider announces.
 */
function WakeSensitivity({ step, exact, onStep, onExact }: { step: number; exact: boolean; onStep: (n: number) => void; onExact: (on: boolean) => void }) {
  return (
    <div className="set-sense">
      <Slider value={step} min={0} max={10} step={1} disabled={exact} onChange={onStep} format={(n) => (exact ? "Exact" : fuzzyWords(stepToFuzzy(n)))} />
      <Checkbox id={`${fieldId(FUZZY)}-exact`} checked={exact} onChange={onExact} label="Exact words only" />
    </div>
  );
}

/**
 * The Jev choice. SegmentedControl doesn't take a Field's id, aria-describedby or
 * aria-invalid yet, so they are set on its radiogroup here: the label's `for`, the help and
 * the error then reach the control that owns them.
 */
function JevControl({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const a = useFieldControl({});
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const g = ref.current?.querySelector('[role="radiogroup"]');
    if (!g) return;
    const set = (k: string, val: string | undefined) => (val ? g.setAttribute(k, val) : g.removeAttribute(k));
    set("id", a.id);
    set("aria-describedby", a["aria-describedby"]);
    set("aria-invalid", a["aria-invalid"] ? "true" : undefined);
  });
  const known = JEV_OPTIONS.some((o) => o.value === value);
  const options = known ? [...JEV_OPTIONS] : [...JEV_OPTIONS, { value, label: value }];
  return (
    <div className="set-jev" ref={ref}>
      <SegmentedControl aria-label="How Ghost hears your yes" fullWidth value={value} onChange={onChange} options={options.map((o) => ({ value: o.value, label: o.label }))} />
    </div>
  );
}

function SettingsSkeleton() {
  return (
    <div className="set-skeleton" aria-busy="true" aria-label="Loading settings">
      {[3, 2, 2].map((rows, i) => (
        <div key={i} className="set-skeleton__section">
          <div className="set-skeleton__titles">
            <Skeleton width={96} height={14} />
            <Skeleton width={320} height={12} />
          </div>
          <Card list>
            {Array.from({ length: rows }, (_, j) => (
              <div key={j} className="set-row set-skeleton__row">
                <div className="set-skeleton__text">
                  <Skeleton width={140} height={12} />
                  <Skeleton width={220} height={10} />
                </div>
                <Skeleton width={88} height={28} radius="var(--radius-md)" />
              </div>
            ))}
          </Card>
        </div>
      ))}
    </div>
  );
}
