import { useCopyFeedback } from "../../lib/useCopyFeedback";
import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import {
  devicesList,
  pairingComplete,
  pairingConfirm,
  pairingStart,
  peerRevoke,
  peerSetCaps,
} from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import type {
  Cap,
  DevicesReply,
  PairingCode,
  PeerRole,
  PeerRow,
  PendingPairing,
  RemoteState,
} from "../../types/ipc";
import { nextCaps } from "./peerCaps";
import { DeviceGlyph } from "./devices/DeviceGlyph";
import { DeviceKebab } from "./devices/DeviceKebab";
import "./devices.css";

/**
 * Devices panel: this device's identity, the two pairing directions, the
 * confirmations this device still owes, and the peers it has.
 *
 * There is no push channel for device state in this slice, so the panel polls
 * `devices_list` — every 2 s while nothing is happening, every 1 s while a code
 * is on screen or a confirmation is pending, because those two states are the
 * ones that change under the user's eyes.
 *
 * The daemon owns every fact here. The panel never derives a device id from a
 * name, never invents a reason string, and never answers a permission or a
 * pairing on the far side's behalf.
 */

const POLL_IDLE_MS = 2_000;
const POLL_ACTIVE_MS = 1_000;

/** Codes are typed off a screen: no 0/O, no 1/I, no lowercase. */
const CODE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_PATTERN = new RegExp(`[^${CODE_ALPHABET}]`, "g");
const CODE_LENGTH = 8;

/** Default port the panel hints at; the daemon can be told another one. */
const DEFAULT_PEER_PORT = 47831;

/** Shown when the typed address is not a shape the daemon can be asked about. */
const ADDRESS_ERROR =
  "Enter the address as host:port, for example 100.64.0.1:47831 or [fd7a:115c:a1e0::1]:47831.";

const ROLE_OPTIONS: readonly { value: PeerRole; label: string; hint: string }[] = [
  {
    value: "client",
    label: "Client",
    hint: "a phone or laptop of yours that views and steers this device",
  },
  {
    value: "daemon",
    label: "Daemon",
    hint: "another devboule that this one may talk to as a machine",
  },
];

/** The panel's capability table, in switch order. The one runtime
 * enumeration a new grant must join — the PEER_CAPS walker test reads it. */
export const CAP_ORDER: readonly Cap[] = [
  "view",
  "send",
  "answer_permissions",
  "create_sessions",
  "roster",
  "search",
  "admin",
];

const CAP_LABELS: Record<Cap, string> = {
  view: "view",
  send: "send",
  answer_permissions: "answer permissions",
  create_sessions: "create sessions",
  // The labels below name what the grant discloses, not just the act:
  // a person deciding on `roster` is deciding who may see the live agents
  // of the user who approved the pairing, and a person deciding on `search`
  // is deciding whether this machine's code may be semantically searched
  // for that device's agents — snippets, paths, line ranges.
  roster: "read this device's live agent roster",
  search: "search this machine's code (source snippets, paths, lines)",
  // The whole remaining surface in one switch, so the label names the surface
  // and gives three examples of it: a person unchecking this is deciding that
  // the device may still drive sessions but may not change this machine.
  admin: "administer this device (settings, projects, shutdown)",
};

/** Groups a hex string in fours, which is how a person reads one aloud. */
export function groupFingerprint(value: string): string {
  return value.match(/.{1,4}/g)?.join(" ") ?? "";
}

/**
 * Splits a pairing address into the host and port the daemon will be asked to
 * reach, without pretending to be an IP validator.
 *
 * Tailscale nodes answer on both a dotted-quad IPv4 and one or more IPv6
 * addresses, so `host:port` and `[ipv6]:port` are accepted. An unbracketed IPv6
 * is refused rather than guessed at: `fd7a::1:47831` has two readings (`fd7a::1`
 * with port 47831, or the address `fd7a::1:47831` with no port) and nothing in
 * the text says which one the person meant. A missing port is refused for the
 * same reason — the daemon's default is not this panel's to assume.
 */
export function parsePeerAddress(input: string): { host: string; port: number } | null {
  const value = input.trim();
  if (value === "") return null;
  let host: string;
  let portText: string;
  if (value.startsWith("[")) {
    const close = value.indexOf("]");
    if (close === -1) return null;
    host = value.slice(1, close);
    if (value[close + 1] !== ":") return null;
    portText = value.slice(close + 2);
    // Shape check only: a colon and nothing but hex, colons and dots. Whether
    // the literal is routable is the daemon's and the OS's answer, not ours.
    if (!host.includes(":") || !/^[0-9a-fA-F:.]+$/.test(host)) return null;
  } else {
    const colon = value.lastIndexOf(":");
    if (colon === -1) return null;
    host = value.slice(0, colon);
    portText = value.slice(colon + 1);
    if (host.includes(":")) return null;
    if (!/^[0-9A-Za-z.-]+$/.test(host)) return null;
  }
  if (host === "") return null;
  if (!/^\d{1,5}$/.test(portText)) return null;
  const port = Number(portText);
  if (port < 1 || port > 65535) return null;
  return { host, port };
}
/** Splits an 8-character code in half so it can be read off a screen. */
export function groupCode(value: string): string {
  return groupFingerprint(value);
}

/** Uppercases and drops everything outside the code alphabet, spaces included. */
export function sanitizePairingCode(input: string): string {
  return input.toUpperCase().replace(CODE_PATTERN, "").slice(0, CODE_LENGTH);
}

/** `m:ss` left until a deadline; never negative. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/** How long ago something happened, in the coarsest unit that fits. */
export function relativeTime(atMs: number, nowMs: number): string {
  const seconds = Math.max(0, Math.round((nowMs - atMs) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 36) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}

/**
 * The reachability line for this device. The three states are the daemon's, and
 * the reason text for `disabled` is the daemon's own sentence — shown verbatim,
 * never rewritten and never replaced by one of ours.
 */
export function remoteLabel(remote: RemoteState): string {
  switch (remote.state) {
    case "enabled":
      return "Reachable on the tailnet";
    case "disabled": {
      // An empty reason is treated as no reason: `Remote off · ` with a
      // dangling separator reads as a rendering bug, not as information.
      const reason = remote.reason?.trim() ?? "";
      return reason === "" ? "Remote off" : `Remote off · ${reason}`;
    }
    case "key_missing":
      return "Key missing · re-pair required";
  }
}

interface RoleChoiceProps {
  name: string;
  value: PeerRole;
  disabled: boolean;
  onChange: (role: PeerRole) => void;
}

/** The two peer roles, each with the one line that says what it means. */
function RoleChoice({ name, value, disabled, onChange }: RoleChoiceProps) {
  return (
    <fieldset className="dev-role-choice">
      <legend>Pair the other device as</legend>
      {ROLE_OPTIONS.map((option) => (
        <label className="dev-role-option" key={option.value}>
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={value === option.value}
            disabled={disabled}
            onChange={() => onChange(option.value)}
          />
          <span className="dev-role-name">{option.label}</span>
          <span className="dev-role-hint">{option.hint}</span>
        </label>
      ))}
    </fieldset>
  );
}

interface PeerCardProps {
  row: PeerRow;
  caps: readonly Cap[];
  now: number;
  /** True while this row's own request is in flight. */
  busy: boolean;
  error: ErrorSentence | undefined;
  onToggleCap: (cap: Cap, next: boolean) => void;
  onRevoke: () => void;
  /** The row left with its confirm armed (a poll removed its peer). */
  onArmedUnmount: () => void;
}

/**
 * The one switch a row may not turn off, and the daemon's own reason it is
 * held: `validate_caps` refuses a `Client` without `view`, and refuses an
 * empty set for every role. A `Daemon`-role row's `view` is not special — the
 * daemon forces that name only for a client, so the panel does not force it
 * either; any single capability is enough to keep the row usable.
 */
function heldCap(row: PeerRow, caps: readonly Cap[]): { cap: Cap; note: string } | null {
  if (row.role === "client") {
    return { cap: "view", note: "Client peers can always view their own sessions" };
  }
  const last = caps[0];
  return caps.length === 1 && last !== undefined
    ? { cap: last, note: "A device must keep at least one capability" }
    : null;
}

/** One paired device: the row (glyph, name, status, kebab) with its
 * capability toggles and its inline revoke underneath. */
function PeerCard({
  row,
  caps,
  now,
  busy,
  error,
  onToggleCap,
  onRevoke,
  onArmedUnmount,
}: PeerCardProps) {
  // Which revoke copy is armed on this row, if any. Local to the row so a
  // half-answered revoke on one device is not shown as armed on another.
  const [armed, setArmed] = useState<"revoke" | "lost" | null>(null);
  const held = heldCap(row, caps);
  // The armed confirm renders at the bottom of a tall row, past the
  // capability switches, while the kebab that armed it sits at the top:
  // focus moves to the confirm and its `role="alert"` announces what
  // the menu just armed, so forward Tab is not the only route there.
  const confirmRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (armed !== null) confirmRef.current?.focus();
  }, [armed]);
  // Disarming unmounts the confirm that holds focus. Landing back on the
  // row's kebab — the control the person came from — keeps the tab order
  // on the row instead of dropping it on `<body>`.
  const rowRef = useRef<HTMLDivElement>(null);
  const wasArmedRef = useRef(false);
  useEffect(() => {
    if (wasArmedRef.current && armed === null) {
      rowRef.current?.querySelector<HTMLButtonElement>(".dev-kebab")?.focus();
    }
    wasArmedRef.current = armed !== null;
  }, [armed]);
  // A poll can remove the peer while its confirm holds focus: the row
  // unmounts under focus, so the panel moves it to the paired list head
  // through its existing post-commit effect instead of losing it to body.
  // The mirror lives in an effect (never a render-time ref write) and the
  // report below subscribes once, so only a real unmount can trigger it —
  // never a re-render, and never Cancel (which disarms without unmounting).
  const armedRef = useRef(armed);
  useEffect(() => {
    armedRef.current = armed;
  });
  useEffect(() => {
    const report = onArmedUnmount;
    return () => {
      if (armedRef.current) report();
    };
  }, [onArmedUnmount]);
  return (
    <div className="dev-row-wrap" ref={rowRef}>
      <div className="dev-row">
        <span className="dev-glyph">
          <DeviceGlyph />
        </span>
        <span className="dev-name">{row.displayName}</span>
        <span className="dev-role-chip">{row.role}</span>
        <span className="dev-status">
          <span className={`dev-dot dev-dot-${row.online ? "live" : "idle"}`} aria-hidden="true" />
          {row.online ? "online" : "offline"}
        </span>
        <span className="dev-spacer" aria-hidden="true" />
        <DeviceKebab
          displayName={row.displayName}
          onRevoke={() => setArmed("revoke")}
          onLost={() => setArmed("lost")}
        />
      </div>
      <div className="dev-details">
        <span className="dev-meta">
          {row.address}
          {row.bindingNodeName === null ? "" : ` · ${row.bindingNodeName}`}
        </span>
        <span className="dev-meta">paired {relativeTime(row.pairedAt, now)}</span>
        <fieldset className="dev-caps">
          <legend className="dev-caps-legend">This device may</legend>
          {CAP_ORDER.map((cap) => (
            <label className="dev-cap" key={cap}>
              <input
                type="checkbox"
                checked={caps.includes(cap)}
                disabled={busy || held?.cap === cap}
                onChange={(event) => onToggleCap(cap, event.target.checked)}
              />
              <span>{CAP_LABELS[cap]}</span>
              {held?.cap === cap ? <span className="dev-cap-note">{held.note}</span> : null}
            </label>
          ))}
        </fieldset>
        {row.role === "client" ? null : (
          <p className="device-copy">
            A daemon peer reaches the sessions it created on this device; this machine's own
            sessions are not in its list.
          </p>
        )}
        {error === undefined ? null : (
          <p role="alert" className="device-error">
            <ErrorText
              sentence={error.sentence}
              detail={error.detail}
              id={`devices-row-error-${row.deviceId}`}
            />
          </p>
        )}
        {armed === null ? null : (
          <div className="device-inline-confirm" role="alert" ref={confirmRef} tabIndex={-1}>
            <p className="device-copy">
              {armed === "lost"
                ? "Revokes this device now, closes its connections, and records it in the audit log."
                : "Revoking stops this device reaching this one. It can come back only with a new pairing code."}
            </p>
            <div className="device-actions">
              <button
                type="button"
                className="settings-device-action"
                disabled={busy}
                onClick={onRevoke}
              >
                Revoke now
              </button>
              <button
                type="button"
                className="settings-device-action"
                onClick={() => setArmed(null)}
              >
                Cancel
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export function DevicesPanel() {
  const [reply, setReply] = useState<DevicesReply | null>(null);
  const [listError, setListError] = useState<ErrorSentence | null>(null);
  // One clock for every countdown and relative time on screen. It only runs
  // while something is counting down; each poll moves it forward as well, so
  // the relative times in the list stay honest without a permanent timer.
  const [now, setNow] = useState(() => Date.now());
  // Bumping this restarts the poll loop, which is how an action that changed
  // the peers table gets its answer onto the screen without waiting a tick.
  const [refreshSeq, setRefreshSeq] = useState(0);

  const [showRole, setShowRole] = useState<PeerRole>("client");
  const [code, setCode] = useState<PairingCode | null>(null);
  const [codeError, setCodeError] = useState<ErrorSentence | null>(null);
  const [starting, setStarting] = useState(false);

  const [enterOpen, setEnterOpen] = useState(false);
  const [enterAddress, setEnterAddress] = useState("");
  const [enterCode, setEnterCode] = useState("");
  const [enterRole, setEnterRole] = useState<PeerRole>("client");
  const [enterBusy, setEnterBusy] = useState(false);
  const [enterError, setEnterError] = useState<ErrorSentence | null>(null);
  const [waiting, setWaiting] = useState<PendingPairing | null>(null);
  const [pairedNotice, setPairedNotice] = useState<PeerRow | null>(null);

  const [confirmBusy, setConfirmBusy] = useState<string | null>(null);
  const [confirmError, setConfirmError] = useState<{
    deviceId: string;
    message: ErrorSentence;
  } | null>(null);

  const [capOverrides, setCapOverrides] = useState<Record<string, Cap[]>>({});
  const [rowBusy, setRowBusy] = useState<string | null>(null);
  const [rowError, setRowError] = useState<{ deviceId: string; message: ErrorSentence } | null>(
    null,
  );

  const feedback = useCopyFeedback({ resetAfterMs: 1500, clearTimerAfterWrite: true });
  const copyState = feedback.stateFor("fingerprint");

  // Where focus goes when the card the user just acted on is removed: a heading
  // with `tabIndex={-1}` is reachable by script and is a deliberate landing
  // spot, unlike the `<body>` fallback a removed card leaves behind.
  const pairHeadingRef = useRef<HTMLHeadingElement | null>(null);
  const pendingHeadingRef = useRef<HTMLHeadingElement | null>(null);
  const pairedHeadingRef = useRef<HTMLHeadingElement | null>(null);

  // False after the panel unmounts, so an action that answers late cannot write
  // state. The poll keeps its own `cancelled` flag on top of this because it
  // also has a timer to stop.
  const mountedRef = useRef(true);

  // Mirror of `waiting` for the poll callback, which is created once per effect
  // run and would otherwise close over a stale value.
  const waitingRef = useRef<PendingPairing | null>(null);

  // Where the post-commit effect should move focus, if anywhere.
  const focusTargetRef = useRef<"pending" | "paired" | null>(null);

  // Data epoch. Every poll request and every local write takes the next number,
  // and a poll reply is applied only while its number is still the newest one.
  // A reply that left the daemon before a change this panel already made is
  // stale by definition: it would put the pre-change row back on screen for a
  // poll cycle, which is exactly the flash a user reads as "my toggle did not
  // take".
  const epochRef = useRef(0);

  function nextEpoch(): number {
    epochRef.current += 1;
    return epochRef.current;
  }

  const pendingCount = reply?.pending.length ?? 0;
  // A code that reached its deadline is dead, and the protocol has no cancel
  // message: deriving the live one from the clock is what drops it, with no
  // state write and no effect to keep in sync.
  const liveCode = code !== null && code.expiresAt > now ? code : null;
  // The waiting card does NOT vanish on expiry. It keeps saying what happened —
  // a parked pairing that silently disappears reads as a bug on both devices —
  // and stops blocking the pairing form underneath it.
  const waitingExpired = waiting !== null && waiting.expiresAt <= now;
  const waitingActive = waiting !== null && !waitingExpired;
  const pollingActive = liveCode !== null || waiting !== null || pendingCount > 0;

  // Poll. A recursive timeout, not an interval: the next request is scheduled
  // only after the previous one settled, so a slow daemon cannot pile requests
  // up. Two guards protect what is on screen: `cancelled` stops a replaced loop
  // (and its timer), and the epoch drops a reply that a newer request or a local
  // write has already passed. Together they are also what keeps an unmounted
  // panel from writing anything at all.
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const intervalMs = pollingActive ? POLL_ACTIVE_MS : POLL_IDLE_MS;
    const tick = () => {
      const epoch = nextEpoch();
      void devicesList()
        .then((fresh) => {
          // Two ways a reply can be past its time: this loop was replaced by a
          // newer one (`cancelled`), or a newer request or local write exists
          // (`epoch`). Either way what is on screen is the newer truth, and
          // this reply must not overwrite it.
          if (cancelled || epoch !== epochRef.current) return;
          const awaited = waitingRef.current;
          const confirmed =
            awaited === null
              ? undefined
              : fresh.peers.find(
                  (peer) => peer.deviceId === awaited.deviceId && peer.revokedAt === null,
                );
          if (confirmed !== undefined) {
            // The far side accepted. The waiting card has done its job, and the
            // row the daemon wrote is what the user should be looking at.
            setWaiting(null);
            setPairedNotice(confirmed);
          }
          setReply(fresh);
          setListError(null);
          setNow(Date.now());
        })
        .catch((cause: unknown) => {
          if (cancelled || epoch !== epochRef.current) return;
          // The last good reply stays on screen: a single missed poll is not
          // evidence that every device disappeared.
          setListError(errorSentence(cause));
        })
        .finally(() => {
          if (cancelled) return;
          timer = setTimeout(tick, intervalMs);
        });
    };
    tick();
    return () => {
      cancelled = true;
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [pollingActive, refreshSeq]);

  const needsClock = liveCode !== null || waiting !== null || pendingCount > 0;
  useEffect(() => {
    if (!needsClock) return;
    const id = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(id);
  }, [needsClock]);

  useEffect(() => {
    // Set on every mount, not only the first: StrictMode's throwaway mount runs
    // this cleanup and would otherwise leave the flag false for the real one.
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    waitingRef.current = waiting;
  }, [waiting]);

  // No dependency array: this has to run after every commit, because the whole
  // point is to observe the DOM the commit produced.
  useEffect(() => {
    const target = focusTargetRef.current;
    if (target === null) return;
    focusTargetRef.current = null;
    const candidates =
      target === "pending"
        ? [pendingHeadingRef.current, pairHeadingRef.current]
        : [pairedHeadingRef.current];
    for (const candidate of candidates) {
      if (candidate !== null) {
        candidate.focus();
        return;
      }
    }
  });

  const refresh = useCallback(() => setRefreshSeq((seq) => seq + 1), []);

  // Stable across renders: it only records a flag the post-commit effect
  // consumes, so the row's unmount guard can depend on it without
  // re-subscribing (and misfiring) on every render.
  const requestPairedFocus = useCallback(() => requestFocus("paired"), []);

  /**
   * Asks for focus to be moved once the card the user acted on is gone. The
   * handler only records where it should land; the effect above runs after the
   * commit that removed the card, so a heading that went with its section is
   * already gone and the fallback is the one that gets focused. Focusing from
   * the handler or a microtask instead aims at an element React is about to
   * delete, and the browser then drops focus on `<body>`.
   */
  function requestFocus(target: "pending" | "paired") {
    focusTargetRef.current = target;
  }

  function copyFingerprint(fingerprint: string) {
    return feedback.copy("fingerprint", groupFingerprint(fingerprint));
  }

  async function showCode() {
    if (starting) return;
    setStarting(true);
    setCodeError(null);
    try {
      const fresh = await pairingStart(showRole);
      if (!mountedRef.current) return;
      setCode(fresh);
      setNow(Date.now());
    } catch (cause) {
      if (!mountedRef.current) return;
      setCodeError(errorSentence(cause));
    } finally {
      if (mountedRef.current) setStarting(false);
    }
  }

  function cancelCode() {
    setCode(null);
    setCodeError(null);
  }

  async function submitEnter(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (enterBusy) return;
    // A shape the daemon cannot act on is refused here, so the user gets the
    // sentence about the field they are looking at instead of a round trip that
    // comes back with the same complaint.
    if (parsePeerAddress(enterAddress) === null) {
      setEnterError({ sentence: ADDRESS_ERROR, detail: null });
      return;
    }
    setEnterBusy(true);
    setEnterError(null);
    setPairedNotice(null);
    try {
      const outcome = await pairingComplete(enterAddress.trim(), enterCode, enterRole);
      if (!mountedRef.current) return;
      if (outcome.type === "pairing_pending") {
        setWaiting(outcome.peer);
        setNow(Date.now());
      } else {
        setPairedNotice(outcome.peer);
        setEnterCode("");
        setEnterOpen(false);
      }
      refresh();
    } catch (cause) {
      // The daemon's pairing errors are already sentences meant for a person
      // (a wrong code says so), so they are shown as they arrive.
      if (!mountedRef.current) return;
      setEnterError(errorSentence(cause));
    } finally {
      if (mountedRef.current) setEnterBusy(false);
    }
  }

  async function answerPending(deviceId: string, accept: boolean) {
    if (confirmBusy !== null) return;
    setConfirmBusy(deviceId);
    setConfirmError(null);
    try {
      const confirmed = await pairingConfirm(deviceId, accept);
      if (!mountedRef.current) return;
      // `null` is a decline, and a decline succeeded: the parked pairing is
      // gone. Dropping the card is the whole outcome — an error card here would
      // tell the user the decline failed when it did not.
      if (confirmed === null) dropPending(deviceId);
      // The card goes either way (accepted, the pending list loses it on the
      // refresh), so focus moves off it before the commit removes it.
      requestFocus("pending");
      refresh();
    } catch (cause) {
      if (!mountedRef.current) return;
      setConfirmError({ deviceId, message: errorSentence(cause) });
    } finally {
      if (mountedRef.current) setConfirmBusy(null);
    }
  }

  // Drops one card from the pending list without waiting for the next poll.
  function dropPending(deviceId: string) {
    nextEpoch();
    setReply((prev) =>
      prev === null
        ? prev
        : { ...prev, pending: prev.pending.filter((pending) => pending.deviceId !== deviceId) },
    );
  }

  function replacePeer(updated: PeerRow) {
    nextEpoch();
    setReply((prev) =>
      prev === null
        ? prev
        : {
            ...prev,
            peers: prev.peers.map((peer) => (peer.deviceId === updated.deviceId ? updated : peer)),
          },
    );
  }

  function clearCapOverride(deviceId: string) {
    nextEpoch();
    setCapOverrides((prev) => {
      const next = { ...prev };
      delete next[deviceId];
      return next;
    });
  }

  async function toggleCap(row: PeerRow, cap: Cap, next: boolean) {
    if (rowBusy !== null) return;
    const current = capOverrides[row.deviceId] ?? row.caps;
    const wanted = nextCaps(current, cap, next, CAP_ORDER);
    // Optimistic: the checkbox flips now, and the daemon's answer is what stays.
    nextEpoch();
    setCapOverrides((prev) => ({ ...prev, [row.deviceId]: [...wanted] }));
    setRowBusy(row.deviceId);
    setRowError(null);
    try {
      const updated = await peerSetCaps(row.deviceId, wanted);
      if (!mountedRef.current) return;
      replacePeer(updated);
      clearCapOverride(row.deviceId);
    } catch (cause) {
      if (!mountedRef.current) return;
      clearCapOverride(row.deviceId);
      setRowError({ deviceId: row.deviceId, message: errorSentence(cause) });
    } finally {
      if (mountedRef.current) setRowBusy(null);
    }
  }

  async function revoke(row: PeerRow) {
    if (rowBusy !== null) return;
    setRowBusy(row.deviceId);
    setRowError(null);
    try {
      const updated = await peerRevoke(row.deviceId);
      if (!mountedRef.current) return;
      replacePeer(updated);
      // A revoked row leaves the paired list for the collapsed section, taking
      // the focused button with it.
      if (updated.revokedAt !== null) requestFocus("paired");
      refresh();
    } catch (cause) {
      if (!mountedRef.current) return;
      setRowError({ deviceId: row.deviceId, message: errorSentence(cause) });
    } finally {
      if (mountedRef.current) setRowBusy(null);
    }
  }

  if (reply === null) {
    return (
      <div id="settings-panel-devices">
        {listError === null ? (
          <div role="status">Loading devices…</div>
        ) : (
          <div className="device-actions" role="alert">
            <ErrorText
              sentence={listError.sentence}
              detail={listError.detail}
              id="devices-list-error-initial"
            />
            <button type="button" className="settings-device-action" onClick={refresh}>
              Retry
            </button>
          </div>
        )}
      </div>
    );
  }

  const self = reply.selfInfo;
  const activePeers = reply.peers.filter((peer) => peer.revokedAt === null);
  const revokedPeers = reply.peers.filter(
    (peer): peer is PeerRow & { revokedAt: number } => peer.revokedAt !== null,
  );
  const showBusy = starting || liveCode !== null;
  const enterBusyFlow = enterBusy || waitingActive;

  return (
    <div id="settings-panel-devices">
      {listError === null ? null : (
        <div className="device-actions" role="alert">
          <ErrorText
            sentence={listError.sentence}
            detail={listError.detail}
            id="devices-list-error-panel"
          />
          <button type="button" className="settings-device-action" onClick={refresh}>
            Retry
          </button>
        </div>
      )}

      <section className="dev-card">
        <div className="dev-head-line">
          <span
            className={`dev-dot dev-dot-${self.remote.state === "enabled" ? "live" : "idle"}`}
            aria-hidden="true"
          />
          <h3 className="dev-card-title">This device</h3>
          <span className="dev-status">{remoteLabel(self.remote)}</span>
        </div>
        <span className="dev-name">{self.displayName}</span>
        <div className="dev-fingerprint-row">
          <span className="dev-fingerprint">{groupFingerprint(self.keyFingerprint)}</span>
          <button
            type="button"
            className="settings-device-action"
            onClick={() => void copyFingerprint(self.keyFingerprint)}
          >
            {copyState === "copied" ? "Copied." : copyState === "failed" ? "Copy failed" : "Copy"}
          </button>
        </div>
        <span className="dev-meta">
          {self.addresses.length === 0
            ? "no tailnet address"
            : `${self.addresses.join(", ")} · port ${self.port}`}
        </span>
      </section>

      <section className="dev-card">
        <h3 className="dev-card-title" ref={pairHeadingRef} tabIndex={-1}>
          Pair a device
        </h3>
        <p className="device-copy">
          One device shows a code, the other types it. The far side then has to confirm the pairing,
          and the code stops working five minutes after it appears.
        </p>
        <div className="device-actions">
          <button
            type="button"
            className="settings-device-action"
            disabled={enterOpen || enterBusyFlow}
            onClick={() => void showCode()}
          >
            {starting ? "Asking…" : "Show a code"}
          </button>
          <button
            type="button"
            className="settings-device-action"
            disabled={showBusy}
            onClick={() => setEnterOpen((open) => !open)}
          >
            Enter a code
          </button>
        </div>

        {liveCode === null ? null : (
          <div className="dev-pair-block">
            <span className="dev-pair-code" aria-label="pairing code">
              {groupCode(liveCode.code)}
            </span>
            <span className="dev-meta">Type this on the other device at {liveCode.address}</span>
            <span className="dev-meta">Expires in {formatDuration(liveCode.expiresAt - now)}</span>
            <button type="button" className="settings-device-action" onClick={cancelCode}>
              Cancel
            </button>
          </div>
        )}
        {codeError === null ? null : (
          <p role="alert" className="device-error">
            <ErrorText
              sentence={codeError.sentence}
              detail={codeError.detail}
              id="devices-code-error"
            />
          </p>
        )}

        <RoleChoice
          name="pairing-show-role"
          value={showRole}
          disabled={showBusy || enterOpen}
          onChange={setShowRole}
        />

        {enterOpen ? (
          <form className="dev-pair-form" onSubmit={(event) => void submitEnter(event)}>
            <label className="device-field">
              <span>Address</span>
              <input
                type="text"
                className="dev-typed-input"
                value={enterAddress}
                placeholder={`100.64.0.1:${DEFAULT_PEER_PORT}`}
                autoComplete="off"
                spellCheck={false}
                onChange={(event) => setEnterAddress(event.target.value)}
              />
              <span className="device-field-hint">
                host:port — an IPv6 address goes in brackets, like [fd7a:115c:a1e0::1]:
                {DEFAULT_PEER_PORT}
              </span>
            </label>
            <label className="device-field">
              <span>Code</span>
              <input
                type="text"
                className="dev-typed-input"
                value={enterCode}
                aria-label="pairing code"
                placeholder="XXXX XXXX"
                autoComplete="off"
                autoCapitalize="characters"
                spellCheck={false}
                inputMode="text"
                maxLength={CODE_LENGTH}
                onChange={(event) => setEnterCode(sanitizePairingCode(event.target.value))}
              />
            </label>
            <RoleChoice
              name="pairing-enter-role"
              value={enterRole}
              disabled={enterBusyFlow}
              onChange={setEnterRole}
            />
            <div className="device-actions">
              <button type="submit" className="settings-device-action" disabled={enterBusyFlow}>
                {enterBusy ? "Pairing…" : "Pair"}
              </button>
              <button
                type="button"
                className="settings-device-action"
                disabled={enterBusyFlow}
                onClick={() => {
                  setEnterOpen(false);
                  setEnterError(null);
                }}
              >
                Cancel
              </button>
            </div>
          </form>
        ) : null}

        {waiting === null ? null : (
          <div className="dev-pair-block">
            {waitingExpired ? (
              <span className="dev-meta">Pairing expired</span>
            ) : (
              <>
                <span className="dev-meta">Waiting for {waiting.displayName} to confirm</span>
                <span className="dev-meta">
                  Expires in {formatDuration(waiting.expiresAt - now)}
                </span>
              </>
            )}
            <button
              type="button"
              className="settings-device-action"
              onClick={() => setWaiting(null)}
            >
              {waitingExpired ? "Dismiss" : "Cancel"}
            </button>
          </div>
        )}

        {pairedNotice === null ? null : (
          <span className="dev-meta">
            Paired with {pairedNotice.displayName} ({pairedNotice.role}).
          </span>
        )}

        {enterError === null ? null : (
          <p role="alert" className="device-error">
            <ErrorText
              sentence={enterError.sentence}
              detail={enterError.detail}
              id="devices-enter-error"
            />
          </p>
        )}
      </section>

      {reply.pending.length === 0 ? null : (
        <section className="dev-list-card">
          <div className="dev-list-head">
            <h3 className="dev-card-title" ref={pendingHeadingRef} tabIndex={-1}>
              Waiting for your confirmation ({reply.pending.length})
            </h3>
            <p className="device-copy">
              A device asked to pair. Compare the fingerprint below with the one shown on that
              device — the person there can read theirs aloud — before you let it in.
            </p>
          </div>
          {reply.pending.map((pending) => (
            <div className="dev-row-wrap" key={pending.deviceId}>
              <div className="dev-row">
                <span className="dev-glyph">
                  <DeviceGlyph />
                </span>
                <span className="dev-name">{pending.displayName}</span>
                <span className="dev-role-chip">{pending.role}</span>
                <span className="dev-status">
                  expires in {formatDuration(pending.expiresAt - now)}
                </span>
              </div>
              <div className="dev-details">
                <span className="dev-fingerprint">{groupFingerprint(pending.keyFingerprint)}</span>
                <span className="dev-meta">{pending.address}</span>
                <div className="device-actions">
                  <button
                    type="button"
                    className="settings-device-action"
                    disabled={confirmBusy !== null}
                    onClick={() => void answerPending(pending.deviceId, true)}
                  >
                    Confirm pairing
                  </button>
                  <button
                    type="button"
                    className="settings-device-action"
                    disabled={confirmBusy !== null}
                    onClick={() => void answerPending(pending.deviceId, false)}
                  >
                    Decline
                  </button>
                </div>
                {confirmError !== null && confirmError.deviceId === pending.deviceId ? (
                  <p role="alert" className="device-error">
                    <ErrorText
                      sentence={confirmError.message.sentence}
                      detail={confirmError.message.detail}
                      id={`devices-confirm-error-${pending.deviceId}`}
                    />
                  </p>
                ) : null}
              </div>
            </div>
          ))}
        </section>
      )}

      <section className="dev-list-card">
        <div className="dev-list-head">
          <h3 className="dev-card-title" ref={pairedHeadingRef} tabIndex={-1}>
            Paired devices
            {activePeers.length === 0 ? "" : ` (${activePeers.length})`}
          </h3>
          {activePeers.length === 0 ? (
            <p className="device-copy">Nothing is paired with this device yet.</p>
          ) : (
            <p className="device-copy">
              A new pairing starts with every switch on. A device paired before 21 September 2026
              keeps whatever set it had then: nothing grants it the new default on its own, and its
              admin switch stays off until a person turns it on. The same is true of a device paired
              before the search capability existed — its search switch starts off, because nothing
              grants a switch the pairing never wrote. For every device, that search switch is the
              one that turns the Oracle search on.
            </p>
          )}
        </div>
        {activePeers.map((row) => (
          <PeerCard
            key={row.deviceId}
            row={row}
            caps={capOverrides[row.deviceId] ?? row.caps}
            now={now}
            busy={rowBusy === row.deviceId}
            error={rowError?.deviceId === row.deviceId ? rowError.message : undefined}
            onToggleCap={(cap, next) => void toggleCap(row, cap, next)}
            onRevoke={() => void revoke(row)}
            onArmedUnmount={requestPairedFocus}
          />
        ))}
        {revokedPeers.length === 0 ? null : (
          <details className="dev-revoked">
            <summary>Revoked ({revokedPeers.length})</summary>
            {revokedPeers.map((row) => (
              <div className="dev-revoked-row" key={row.deviceId}>
                <span className="dev-name">{row.displayName}</span>
                <span className="dev-role-chip">{row.role}</span>
                <span className="dev-meta" title={new Date(row.revokedAt).toISOString()}>
                  revoked {relativeTime(row.revokedAt, now)}
                </span>
              </div>
            ))}
          </details>
        )}
      </section>
    </div>
  );
}
