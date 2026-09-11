import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import {
  devicesList,
  pairingComplete,
  pairingConfirm,
  pairingStart,
  peerRevoke,
  peerSetCaps,
  reasonFromCause,
} from "../../lib/tauri";
import type {
  Cap,
  DevicesReply,
  PairingCode,
  PeerRole,
  PeerRow,
  PendingPairing,
  RemoteState,
} from "../../types/ipc";
import { SettingsHeading } from "./SettingsSurface";
import "./settings.css";

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

const CAP_ORDER: readonly Cap[] = ["view", "send", "answer_permissions", "create_sessions"];

const CAP_LABELS: Record<Cap, string> = {
  view: "view",
  send: "send",
  answer_permissions: "answer permissions",
  create_sessions: "create sessions",
};

const DEVICES_DESCRIPTION =
  "Paired clients that may drive this daemon. Pairing is per-device and revocable.";

/** Groups a hex string in fours, which is how a person reads one aloud. */
export function groupFingerprint(value: string): string {
  return value.match(/.{1,4}/g)?.join(" ") ?? "";
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
    case "disabled":
      return remote.reason === null ? "Remote off" : `Remote off · ${remote.reason}`;
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
    <fieldset className="device-role-choice">
      <legend>Pair the other device as</legend>
      {ROLE_OPTIONS.map((option) => (
        <label className="device-role-option" key={option.value}>
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={value === option.value}
            disabled={disabled}
            onChange={() => onChange(option.value)}
          />
          <span className="device-role-name">{option.label}</span>
          <span className="device-role-hint">{option.hint}</span>
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
  error: string | undefined;
  onToggleCap: (cap: Cap, next: boolean) => void;
  onRevoke: () => void;
}

/** One paired device, with its capability toggles and its inline revoke. */
function PeerCard({ row, caps, now, busy, error, onToggleCap, onRevoke }: PeerCardProps) {
  // Which revoke copy is armed on this row, if any. Local to the row so a
  // half-answered revoke on one device is not shown as armed on another.
  const [armed, setArmed] = useState<"revoke" | "lost" | null>(null);
  return (
    <div className="settings-device-card device-peer">
      <div className="device-peer-head">
        <span
          className={`device-dot device-dot-${row.online ? "ready" : "idle"}`}
          aria-hidden="true"
        />
        <span className="settings-card-title">{row.displayName}</span>
        <span className="device-role-chip">{row.role}</span>
        <span className="settings-card-value">{row.online ? "online" : "offline"}</span>
      </div>
      <span className="settings-card-meta">
        {row.address}
        {row.bindingNodeName === null ? "" : ` · ${row.bindingNodeName}`}
      </span>
      <span className="settings-card-meta">paired {relativeTime(row.pairedAt, now)}</span>
      {row.role === "client" ? (
        <fieldset className="device-caps">
          <legend className="device-caps-legend">This client may</legend>
          {CAP_ORDER.map((cap) => (
            <label className="device-cap" key={cap}>
              <input
                type="checkbox"
                checked={caps.includes(cap)}
                disabled={busy || cap === "view"}
                onChange={(event) => onToggleCap(cap, event.target.checked)}
              />
              <span>{CAP_LABELS[cap]}</span>
              {cap === "view" ? (
                <span className="device-cap-note">
                  Client peers can always view their own sessions
                </span>
              ) : null}
            </label>
          ))}
        </fieldset>
      ) : (
        <p className="device-copy">
          A daemon peer is scoped by the daemon: it reaches the sessions it created on this device
          and nothing else.
        </p>
      )}
      {error === undefined ? null : (
        <p role="alert" className="device-error">
          {error}
        </p>
      )}
      <div className="device-actions">
        <button type="button" className="settings-device-action" onClick={() => setArmed("revoke")}>
          Revoke
        </button>
        <button type="button" className="settings-device-action" onClick={() => setArmed("lost")}>
          Lost or stolen device
        </button>
      </div>
      {armed === null ? null : (
        <div className="device-inline-confirm">
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
            <button type="button" className="settings-device-action" onClick={() => setArmed(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export function DevicesPanel() {
  const [reply, setReply] = useState<DevicesReply | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  // One clock for every countdown and relative time on screen. It only runs
  // while something is counting down; each poll moves it forward as well, so
  // the relative times in the list stay honest without a permanent timer.
  const [now, setNow] = useState(() => Date.now());
  // Bumping this restarts the poll loop, which is how an action that changed
  // the peers table gets its answer onto the screen without waiting a tick.
  const [refreshSeq, setRefreshSeq] = useState(0);

  const [showRole, setShowRole] = useState<PeerRole>("client");
  const [code, setCode] = useState<PairingCode | null>(null);
  const [codeError, setCodeError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);

  const [enterOpen, setEnterOpen] = useState(false);
  const [enterAddress, setEnterAddress] = useState("");
  const [enterCode, setEnterCode] = useState("");
  const [enterRole, setEnterRole] = useState<PeerRole>("client");
  const [enterBusy, setEnterBusy] = useState(false);
  const [enterError, setEnterError] = useState<string | null>(null);
  const [waiting, setWaiting] = useState<PendingPairing | null>(null);
  const [pairedNotice, setPairedNotice] = useState<PeerRow | null>(null);

  const [confirmBusy, setConfirmBusy] = useState<string | null>(null);
  const [confirmError, setConfirmError] = useState<{ deviceId: string; message: string } | null>(
    null,
  );

  const [capOverrides, setCapOverrides] = useState<Record<string, Cap[]>>({});
  const [rowBusy, setRowBusy] = useState<string | null>(null);
  const [rowError, setRowError] = useState<{ deviceId: string; message: string } | null>(null);

  const [copied, setCopied] = useState(false);
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const pendingCount = reply?.pending.length ?? 0;
  // A code (or a parked pairing) that reached its deadline is dead, and the
  // protocol has no cancel message: deriving the live one from the clock is what
  // drops it, with no state write and no effect to keep in sync.
  const liveCode = code !== null && code.expiresAt > now ? code : null;
  const liveWaiting = waiting !== null && waiting.expiresAt > now ? waiting : null;
  const pollingActive = liveCode !== null || liveWaiting !== null || pendingCount > 0;

  // Poll. A recursive timeout, not an interval: the next request is scheduled
  // only after the previous one settled, so a slow daemon cannot pile requests
  // up. The cleanup clears the pending timer and gates every state write, which
  // is what keeps an unmounted panel from writing anything at all.
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const intervalMs = pollingActive ? POLL_ACTIVE_MS : POLL_IDLE_MS;
    const tick = () => {
      void devicesList()
        .then((fresh) => {
          if (cancelled) return;
          setReply(fresh);
          setListError(null);
          setNow(Date.now());
        })
        .catch((cause: unknown) => {
          if (cancelled) return;
          // The last good reply stays on screen: a single missed poll is not
          // evidence that every device disappeared.
          setListError(reasonFromCause(cause));
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

  const needsClock = liveCode !== null || liveWaiting !== null || pendingCount > 0;
  useEffect(() => {
    if (!needsClock) return;
    const id = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(id);
  }, [needsClock]);

  useEffect(
    () => () => {
      if (copyTimerRef.current !== null) clearTimeout(copyTimerRef.current);
    },
    [],
  );

  const refresh = useCallback(() => setRefreshSeq((seq) => seq + 1), []);

  async function copyFingerprint(fingerprint: string) {
    const text = groupFingerprint(fingerprint);
    try {
      await navigator.clipboard?.writeText(text);
    } catch {
      // A denied clipboard is not worth an error card: the text is on screen.
      return;
    }
    setCopied(true);
    if (copyTimerRef.current !== null) clearTimeout(copyTimerRef.current);
    copyTimerRef.current = setTimeout(() => setCopied(false), 1_500);
  }

  async function showCode() {
    if (starting) return;
    setStarting(true);
    setCodeError(null);
    try {
      const fresh = await pairingStart(showRole);
      setCode(fresh);
      setNow(Date.now());
    } catch (cause) {
      setCodeError(reasonFromCause(cause));
    } finally {
      setStarting(false);
    }
  }

  function cancelCode() {
    setCode(null);
    setCodeError(null);
  }

  async function submitEnter(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (enterBusy) return;
    setEnterBusy(true);
    setEnterError(null);
    try {
      const outcome = await pairingComplete(enterAddress.trim(), enterCode, enterRole);
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
      setEnterError(reasonFromCause(cause));
    } finally {
      setEnterBusy(false);
    }
  }

  async function answerPending(deviceId: string, accept: boolean) {
    if (confirmBusy !== null) return;
    setConfirmBusy(deviceId);
    setConfirmError(null);
    try {
      const confirmed = await pairingConfirm(deviceId, accept);
      // `null` is a decline, and a decline succeeded: the parked pairing is
      // gone. Dropping the card is the whole outcome — an error card here would
      // tell the user the decline failed when it did not.
      if (confirmed === null) dropPending(deviceId);
      refresh();
    } catch (cause) {
      setConfirmError({ deviceId, message: reasonFromCause(cause) });
    } finally {
      setConfirmBusy(null);
    }
  }

  // Drops one card from the pending list without waiting for the next poll.
  function dropPending(deviceId: string) {
    setReply((prev) =>
      prev === null
        ? prev
        : { ...prev, pending: prev.pending.filter((pending) => pending.deviceId !== deviceId) },
    );
  }

  function replacePeer(updated: PeerRow) {
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
    setCapOverrides((prev) => {
      const next = { ...prev };
      delete next[deviceId];
      return next;
    });
  }

  async function toggleCap(row: PeerRow, cap: Cap, next: boolean) {
    if (rowBusy !== null) return;
    const current = capOverrides[row.deviceId] ?? row.caps;
    const wanted = next
      ? CAP_ORDER.filter((candidate) => candidate === cap || current.includes(candidate))
      : CAP_ORDER.filter((candidate) => candidate !== cap && current.includes(candidate));
    // Optimistic: the checkbox flips now, and the daemon's answer is what stays.
    setCapOverrides((prev) => ({ ...prev, [row.deviceId]: [...wanted] }));
    setRowBusy(row.deviceId);
    setRowError(null);
    try {
      const updated = await peerSetCaps(row.deviceId, wanted);
      replacePeer(updated);
      clearCapOverride(row.deviceId);
    } catch (cause) {
      clearCapOverride(row.deviceId);
      setRowError({ deviceId: row.deviceId, message: reasonFromCause(cause) });
    } finally {
      setRowBusy(null);
    }
  }

  async function revoke(row: PeerRow) {
    if (rowBusy !== null) return;
    setRowBusy(row.deviceId);
    setRowError(null);
    try {
      const updated = await peerRevoke(row.deviceId);
      replacePeer(updated);
      refresh();
    } catch (cause) {
      setRowError({ deviceId: row.deviceId, message: reasonFromCause(cause) });
    } finally {
      setRowBusy(null);
    }
  }

  if (reply === null) {
    return (
      <div id="settings-panel-devices" role="tabpanel" aria-label="Devices">
        <SettingsHeading title="Devices" description={DEVICES_DESCRIPTION} />
        {listError === null ? (
          <div role="status">Loading devices…</div>
        ) : (
          <div className="device-actions" role="alert">
            <span>{listError}</span>
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
  const enterBusyFlow = enterBusy || liveWaiting !== null;

  return (
    <div id="settings-panel-devices" role="tabpanel" aria-label="Devices">
      <SettingsHeading title="Devices" description={DEVICES_DESCRIPTION} />

      <div className="settings-stack settings-stack-tight settings-devices-list">
        {listError === null ? null : (
          <div className="device-actions" role="alert">
            <span>{listError}</span>
            <button type="button" className="settings-device-action" onClick={refresh}>
              Retry
            </button>
          </div>
        )}

        <section className="settings-card device-section">
          <div className="device-self-head">
            <span
              className={`device-dot device-dot-${self.remote.state === "enabled" ? "ready" : "idle"}`}
              aria-hidden="true"
            />
            <h3 className="settings-card-title">This device</h3>
            <span className="settings-card-value">{remoteLabel(self.remote)}</span>
          </div>
          <span className="settings-card-meta">{self.displayName}</span>
          <div className="device-fingerprint-row">
            <span className="device-fingerprint">{groupFingerprint(self.keyFingerprint)}</span>
            <button
              type="button"
              className="settings-device-action"
              onClick={() => void copyFingerprint(self.keyFingerprint)}
            >
              {copied ? "Copied." : "Copy"}
            </button>
          </div>
          <span className="settings-card-meta">
            {self.addresses.length === 0
              ? "no tailnet address"
              : `${self.addresses.join(", ")} · port ${self.port}`}
          </span>
        </section>

        <section className="settings-card device-section">
          <h3 className="settings-card-title">Pair a device</h3>
          <p className="device-copy">
            One device shows a code, the other types it. The far side then has to confirm the
            pairing, and the code stops working five minutes after it appears.
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
            <div className="device-pair-block">
              <span className="device-pair-code" aria-label="pairing code">
                {groupCode(liveCode.code)}
              </span>
              <span className="settings-card-meta">
                Type this on the other device at {liveCode.address}
              </span>
              <span className="settings-card-meta">
                Expires in {formatDuration(liveCode.expiresAt - now)}
              </span>
              <button type="button" className="settings-device-action" onClick={cancelCode}>
                Cancel
              </button>
            </div>
          )}
          {codeError === null ? null : (
            <p role="alert" className="device-error">
              {codeError}
            </p>
          )}

          <RoleChoice
            name="pairing-show-role"
            value={showRole}
            disabled={showBusy || enterOpen}
            onChange={setShowRole}
          />

          {enterOpen ? (
            <form className="device-pair-form" onSubmit={(event) => void submitEnter(event)}>
              <label className="device-field">
                <span>Address</span>
                <input
                  type="text"
                  value={enterAddress}
                  placeholder={`100.64.0.1:${DEFAULT_PEER_PORT}`}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(event) => setEnterAddress(event.target.value)}
                />
              </label>
              <label className="device-field">
                <span>Code</span>
                <input
                  type="text"
                  value={enterCode}
                  aria-label="pairing code"
                  placeholder="XXXX XXXX"
                  autoComplete="off"
                  spellCheck={false}
                  inputMode="text"
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

          {liveWaiting === null ? null : (
            <div className="device-pair-block">
              <span className="settings-card-meta">
                Waiting for {liveWaiting.displayName} to confirm
              </span>
              <span className="settings-card-meta">
                Expires in {formatDuration(liveWaiting.expiresAt - now)}
              </span>
              <button
                type="button"
                className="settings-device-action"
                onClick={() => setWaiting(null)}
              >
                Cancel
              </button>
            </div>
          )}

          {pairedNotice === null ? null : (
            <span className="settings-card-meta">
              Paired with {pairedNotice.displayName} ({pairedNotice.role}).
            </span>
          )}

          {enterError === null ? null : (
            <p role="alert" className="device-error">
              {enterError}
            </p>
          )}
        </section>

        {reply.pending.length === 0 ? null : (
          <section className="settings-card device-section">
            <h3 className="settings-card-title">
              Waiting for your confirmation ({reply.pending.length})
            </h3>
            <p className="device-copy">
              A device asked to pair. Compare the fingerprint below with the one shown on that
              device — the person there can read theirs aloud — before you let it in.
            </p>
            {reply.pending.map((pending) => (
              <div className="settings-device-card device-pending" key={pending.deviceId}>
                <div className="device-peer-head">
                  <span className="settings-card-title">{pending.displayName}</span>
                  <span className="device-role-chip">{pending.role}</span>
                  <span className="settings-card-value">
                    expires in {formatDuration(pending.expiresAt - now)}
                  </span>
                </div>
                <span className="device-fingerprint">
                  {groupFingerprint(pending.keyFingerprint)}
                </span>
                <span className="settings-card-meta">{pending.address}</span>
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
                    {confirmError.message}
                  </p>
                ) : null}
              </div>
            ))}
          </section>
        )}

        <section className="settings-card device-section">
          <h3 className="settings-card-title">
            Paired devices
            {activePeers.length === 0 ? "" : ` (${activePeers.length})`}
          </h3>
          {activePeers.length === 0 ? (
            <p className="device-copy">Nothing is paired with this device yet.</p>
          ) : null}
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
            />
          ))}
          {revokedPeers.length === 0 ? null : (
            <details className="device-revoked">
              <summary>Revoked ({revokedPeers.length})</summary>
              {revokedPeers.map((row) => (
                <div className="settings-device-card device-revoked-row" key={row.deviceId}>
                  <span className="settings-card-title">{row.displayName}</span>
                  <span className="device-role-chip">{row.role}</span>
                  <span
                    className="settings-card-meta"
                    title={new Date(row.revokedAt).toISOString()}
                  >
                    revoked {relativeTime(row.revokedAt, now)}
                  </span>
                </div>
              ))}
            </details>
          )}
        </section>
      </div>
    </div>
  );
}
