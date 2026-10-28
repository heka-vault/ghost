/**
 * Owner: settings. The Phone page (ADR 0016, ADR 0020): whether the phone listener is on, the
 * paired phone and how it last reached Ghost (home network or tunnel), each address and its
 * network, and pairing. Ghost pairs with exactly one phone, the operator's; pairing another
 * replaces it, and the page (once, beside the paired phone) and the dialog both say so. With
 * no address on a tunnel, a short
 * guide says how to reach Ghost away from home. The page header carries the title and what
 * it is for.
 *
 * Pairing is a short focused task, so it is a Dialog: a QR code the phone app scans (its
 * payload is `PhonePairing.link`, docs/phone-protocol.md), and beside it the address and the
 * six-digit code to type instead, large and in mono, with the code's countdown and a New code
 * action. The QR code is dark on a light tile in both themes. The dialog polls GET /phone
 * while it is open (the daemon sends no event when a phone pairs), and flips to “Paired with
 * Pixel 9” the moment the phone spends the code.
 *
 * Mock states: `?phone=off|unpaired|paired|tunnel|tunnel-down|pairs|expires` on any scenario
 * (see design/mock/index.ts), and `?settings=pairing` opens the dialog on load.
 */
import type { PhoneAddress, PhoneDevice, PhonePairing, PhoneView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Check, Smartphone } from "lucide-react";
import { create as createQr } from "qrcode";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Button, Card, Dialog, Disclosure, ListRow, Notice, Section, Skeleton, Spinner, errorText, relativeTime, useConfirm, useLive, useToast } from "../../ui";
import { sectionDomId, tildePath } from "./model";
import { CommandCopy, Row } from "./parts";

const POLL_MS = 1500;
const PORT = "8765";
/** Decided on #36: said once on the page, beside the paired phone. */
const ONE_PHONE = "Ghost pairs with one phone. Pairing another replaces this one.";

/** “482913” as “482 913”: two groups of three are easier to read off and type. */
const grouped = (code: string) => `${code.slice(0, 3)} ${code.slice(3)}`;
/** Seconds as “1:42”. */
const clock = (secs: number) => `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}`;

export function PhoneSection({ settingsPath, openOnLoad }: { settingsPath: string; openOnLoad?: boolean }) {
  const { client, status } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog: confirmDialog } = useConfirm();
  const [phone, setPhone] = useState<PhoneView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pairOpen, setPairOpen] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  const pairButton = useRef<HTMLButtonElement>(null);

  const load = useCallback(async () => {
    try {
      const p = await client.phone();
      setPhone(p);
      setError(null);
      return p;
    } catch (e) {
      setError(errorText(e));
      return null;
    }
  }, [client]);

  useEffect(() => {
    void load().then((p) => {
      if (openOnLoad && p?.address) setPairOpen(true);
    });
  }, [load, openOnLoad]);
  // A phone session starting or ending moves “last seen”.
  useEffect(() => {
    if (phone) void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status?.session_origin]);

  const forget = async () => {
    const name = phone?.device?.name ?? "this phone";
    const ok = await confirm({
      title: `Forget ${name}?`,
      description: `Ghost stops answering ${name}. Its sessions stop working until you pair it again, and a session on it now ends.`,
      confirmLabel: `Forget ${name}`,
      tone: "danger",
    });
    if (!ok) return;
    setForgetting(true);
    try {
      await client.forgetPhone();
      await load();
      toast({ title: "Phone forgotten", description: `Pair ${name} again to talk to Ghost from it.` });
      requestAnimationFrame(() => pairButton.current?.focus({ preventScroll: true }));
    } catch (e) {
      toast({ title: "Couldn’t forget the phone", description: errorText(e), tone: "danger" });
    } finally {
      setForgetting(false);
    }
  };

  const device = phone?.device ?? null;
  // An older daemon sends no `listen`: its addresses, network unknown.
  const listen: PhoneAddress[] = phone?.listen?.length
    ? phone.listen
    : (phone?.addresses?.length ? phone.addresses : phone?.address ? [phone.address] : []).map((address) => ({ address, network: null, listening: true }));

  return (
    <Section id={sectionDomId("phone")} className="set-section">
      {confirmDialog}
      {!phone && error && (
        <Notice tone="danger" title="Couldn’t check the phone" action={<Button size="sm" onClick={() => void load()}>Try again</Button>}>
          {error}
        </Notice>
      )}
      {!phone && !error && (
        <Card list>
          <div className="set-row set-skeleton__row" aria-busy="true" aria-label="Loading the phone">
            <div className="set-skeleton__text">
              <Skeleton width={140} height={12} />
              <Skeleton width={260} height={10} />
            </div>
            <Skeleton width={104} height={28} radius="var(--radius-md)" />
          </div>
        </Card>
      )}

      {phone && !phone.address && <ListenerOff settingsPath={settingsPath} />}

      {phone?.address && (
        <Card list>
          {device ? (
            <ListRow
              leading={<Smartphone size={16} aria-hidden className="set-phone__icon" />}
              title={device.name}
              description={
                <>
                  Paired <time dateTime={device.paired_at}>{relativeTime(device.paired_at)}</time> · <Reached device={device} />
                  {status?.session_origin === "phone" && <span className="set-phone__live"> · in a session now</span>}
                  <span className="set-phone__one">{ONE_PHONE}</span>
                </>
              }
              trailing={
                <span className="set-phone__actions">
                  <Button size="sm" variant="ghost" disabled={!online} onClick={() => setPairOpen(true)}>
                    Pair another phone
                  </Button>
                  <Button size="sm" variant="danger-ghost" loading={forgetting} disabled={!online} onClick={() => void forget()}>
                    Forget this phone
                  </Button>
                </span>
              }
            />
          ) : (
            <ListRow
              leading={<Smartphone size={16} aria-hidden className="set-phone__icon" />}
              title="No phone paired"
              description="Pair once. After that your phone can start a session from home, or from anywhere through your tunnel. Ghost pairs with one phone, yours."
              trailing={
                <Button ref={pairButton} size="sm" variant="secondary" disabled={!online} onClick={() => setPairOpen(true)}>
                  Pair a phone
                </Button>
              }
            />
          )}
          <ListRow
            title={listen.length > 1 ? "Phone addresses" : "Phone address"}
            description={
              listen.length > 1
                ? "Where your phone reaches Ghost. It uses the first that answers, so at home it stays on your home network."
                : "Where your phone reaches Ghost."
            }
            meta={
              <ul className="set-phone__addresses">
                {listen.map((a, i) => (
                  <li key={a.address + i}>
                    <span className="set-value is-mono">{a.address}</span>
                    {networkLabel(a) && <span className="set-phone__network"> · {networkLabel(a)}</span>}
                  </li>
                ))}
              </ul>
            }
          />
          {listen.every((a) => a.network === "home") && <TunnelGuide settingsPath={settingsPath} home={listen[0]?.address ?? phone.address} />}
        </Card>
      )}

      <PairDialog
        open={pairOpen}
        before={device}
        onClose={() => {
          setPairOpen(false);
          void load();
        }}
        onPaired={(p) => setPhone(p)}
      />
    </Section>
  );
}

/** “reached Ghost 3 min ago over your home network”, “… through your tunnel”, or not yet. */
function Reached({ device }: { device: PhoneDevice }) {
  if (!device.last_seen) return <>hasn’t reached Ghost yet</>;
  const via =
    device.last_network === "home" ? " over your home network"
    : device.last_network === "tunnel" ? " through your tunnel"
    : device.last_address ? ` at ${device.last_address}`
    : "";
  return (
    <>
      reached Ghost <time dateTime={device.last_seen}>{relativeTime(device.last_seen)}</time>
      {via}
    </>
  );
}

const WILDCARD = /^(0\.0\.0\.0|\[::\]|::):\d+$/;

/** What an address is to the operator: home network, tunnel, every network, or not up yet. */
function networkLabel(a: PhoneAddress): string | null {
  if (a.network === "home") return "home network";
  if (a.network === "tunnel") return "tunnel";
  if (!a.listening) return "not up yet, Ghost keeps trying";
  if (WILDCARD.test(a.address)) return "every network";
  return null;
}

/**
 * Every address is on the home network: how to reach Ghost away from home (ADR 0020). Closed
 * by default; the page's focal point stays the phone.
 */
function TunnelGuide({ settingsPath, home }: { settingsPath: string; home: string | null }) {
  const port = home?.match(/:(\d+)$/)?.[1] ?? PORT;
  const line = `GHOST_PHONE_LISTEN=${home ?? `192.168.1.20:${port}`},10.8.0.4:${port}`;
  return (
    <div className="set-phone__guide">
      <Disclosure summary="Reach Ghost away from home">
        <ol className="set-phone__steps set-help is-readable">
          <li>Make this computer and your phone peers of your own WireGuard network. On the phone, let the WireGuard app carry only Ghost, and leave it on.</li>
          <li>
            Add this computer’s tunnel address after the home one in <span className="set-path">{tildePath(settingsPath)}</span>, then restart Ghost. Use your tunnel address in place of 10.8.0.4.
            <span className="set-phone-off__commands">
              <CommandCopy command={line} />
              <CommandCopy />
            </span>
          </li>
          <li>Your phone learns the new address the next time it reaches Ghost at home. It doesn’t need pairing again.</li>
        </ol>
        <p className="set-help">
          The whole setup, and why it’s WireGuard: <code>docs/adr/0020-phone-reaches-the-desk-over-the-operators-wireguard.md</code>
        </p>
      </Disclosure>
    </div>
  );
}

/** A QR code as crisp SVG: dark modules on a light tile with a quiet zone, in both themes. */
function QrCode({ value, label, dim }: { value: string; label: string; dim?: boolean }) {
  const { path, size } = useMemo(() => {
    const { modules } = createQr(value, { errorCorrectionLevel: "M" });
    let d = "";
    for (let y = 0; y < modules.size; y++) {
      for (let x = 0; x < modules.size; x++) if (modules.get(y, x)) d += `M${x} ${y}h1v1h-1z`;
    }
    return { path: d, size: modules.size };
  }, [value]);
  const quiet = 4;
  const box = size + quiet * 2;
  return (
    <svg className={dim ? "set-pair__qr is-dim" : "set-pair__qr"} viewBox={`${-quiet} ${-quiet} ${box} ${box}`} shapeRendering="crispEdges" role="img" aria-label={label}>
      <rect x={-quiet} y={-quiet} width={box} height={box} fill="var(--qr-light)" />
      <path d={path} fill="var(--qr-dark)" />
    </svg>
  );
}

/** GHOST_PHONE_LISTEN is unset: say how to turn it on, and the firewall step if ufw is on. */
function ListenerOff({ settingsPath }: { settingsPath: string }) {
  return (
    <Card list>
      <Row className="set-phone-off">
        <p className="set-label">
          <Smartphone size={16} aria-hidden className="set-phone__icon" />
          The phone listener is off
        </p>
        <p className="set-help is-readable">
          Add this line to <span className="set-path">{tildePath(settingsPath)}</span> with this computer’s address on your home network, then restart Ghost.
        </p>
        <div className="set-phone-off__commands">
          <CommandCopy command={`GHOST_PHONE_LISTEN=192.168.1.20:${PORT}`} />
          <CommandCopy />
        </div>
        <p className="set-help">
          Run <code>hostname -I</code> to see the address. Use the one your router gave this computer, like 192.168.1.20.
        </p>
      </Row>
      <div className="set-phone-off__ufw">
        <Disclosure summary="If ufw is on">
          <p className="set-help is-readable">Let your home network reach the listener’s port, and the UDP ports the voice uses.</p>
          <div className="set-phone-off__commands">
            <CommandCopy command={`sudo ufw allow from 192.168.1.0/24 to any port ${PORT} proto tcp`} />
            <CommandCopy command="sudo ufw allow from 192.168.1.0/24 proto udp" />
          </div>
        </Disclosure>
      </div>
    </Card>
  );
}

/**
 * Pair a phone: ask for a code on open, show it until it expires or the phone spends it. `before`
 * is the phone paired when the dialog opened; pairing again replaces it, so a different id or a
 * newer paired_at is what counts as “just paired”.
 */
function PairDialog({ open, before, onClose, onPaired }: { open: boolean; before: PhoneView["device"]; onClose: () => void; onPaired: (p: PhoneView) => void }) {
  const { client } = useGhost();
  const { online } = useLive();
  const [pairing, setPairing] = useState<PhonePairing | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [paired, setPaired] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const beforeRef = useRef(before);
  const doneRef = useRef<HTMLButtonElement>(null);
  // The code's view is gone once the phone pairs; focus goes to Done, not the close button.
  useEffect(() => {
    if (paired) doneRef.current?.focus({ preventScroll: true });
  }, [paired]);

  const fresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setPairing(await client.pairPhone());
      setNow(Date.now());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLoading(false);
    }
  }, [client]);

  // Each open starts clean with a new code.
  useEffect(() => {
    if (!open) return;
    beforeRef.current = before;
    setPairing(null);
    setPaired(null);
    void fresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const left = pairing ? Math.max(0, Math.ceil((new Date(pairing.expires_at).getTime() - now) / 1000)) : 0;
  const expired = !!pairing && left === 0;

  // The countdown.
  useEffect(() => {
    if (!open || !pairing || paired) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [open, pairing, paired]);

  // Watch for the phone while a code is live.
  useEffect(() => {
    if (!open || !pairing || paired || expired) return;
    let stop = false;
    const t = window.setInterval(async () => {
      try {
        const p = await client.phone();
        const d = p.device;
        const was = beforeRef.current;
        if (!stop && d && (!was || d.id !== was.id || d.paired_at !== was.paired_at)) {
          setPaired(d.name);
          onPaired(p);
        }
      } catch {
        /* A missed poll is fine; the next one tries again. */
      }
    }, POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(t);
    };
  }, [open, pairing, paired, expired, client, onPaired]);

  const footer = paired ? (
    <Button ref={doneRef} variant="primary" onClick={onClose}>
      Done
    </Button>
  ) : (
    <>
      <Button variant="ghost" onClick={onClose}>
        Close
      </Button>
      <Button variant={expired ? "primary" : "secondary"} loading={loading} disabled={!online} onClick={() => void fresh()}>
        New code
      </Button>
    </>
  );

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={paired ? `Paired with ${paired}` : "Pair a phone"}
      description={
        paired
          ? undefined
          : `On your phone, open Ghost and point it at this code, or tap Enter the address and type both. ${before ? `Ghost pairs with one phone, yours. Pairing another replaces ${before.name}.` : "Ghost pairs with one phone, yours."}`
      }
      footer={footer}
      className="set-pair"
    >
      {paired ? (
        <div className="set-pair__done" role="status">
          <span className="set-pair__done-icon">
            <Check size={20} aria-hidden />
          </span>
          <p className="set-help is-readable">Your phone can start a session with Ghost now. Its mic is open only while that session runs.</p>
        </div>
      ) : error ? (
        <Notice tone="danger" title="Couldn’t get a code">
          {error}
        </Notice>
      ) : (
        <div className="set-pair__body">
          <div className="set-pair__well">
          <div className="set-pair__scan">
            {pairing ? (
              <QrCode value={pairing.link} dim={expired} label={expired ? "Expired pairing code" : "Pairing code for the Ghost app on your phone"} />
            ) : (
              <Skeleton width="100%" height="100%" radius="var(--radius-md)" />
            )}
          </div>
          <dl className="set-pair__pairs">
            <div className="set-pair__item">
              <dt className="set-pair__label">Address</dt>
              <dd className="set-pair__address" data-user-content>
                {pairing ? pairing.address : <Skeleton width={220} height={24} />}
              </dd>
            </div>
            <div className="set-pair__item">
              <dt className="set-pair__label">Code</dt>
              <dd className={expired ? "set-pair__code is-expired" : "set-pair__code"}>
                {pairing ? (
                  <span aria-label={pairing.code.split("").join(" ")}>{grouped(pairing.code)}</span>
                ) : (
                  <Skeleton width={200} height={40} />
                )}
              </dd>
            </div>
          </dl>
          </div>
          <p className="set-pair__status" aria-live="polite">
            {!pairing ? (
              " "
            ) : expired ? (
              <span className="t-2">This code expired. Get a new one to keep going.</span>
            ) : (
              <>
                <Spinner size={14} />
                <span>
                  Waiting for your phone · <span className="t-num" aria-hidden>{clock(left)}</span>
                </span>
              </>
            )}
          </p>
          {pairing && !expired && <p className="set-help">Good for two minutes and five tries. A new code replaces this one.</p>}
        </div>
      )}
    </Dialog>
  );
}
