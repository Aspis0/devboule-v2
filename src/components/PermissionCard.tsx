import { useEffect, useRef, useState } from "react";
import { reasonFromCause, sessionPermissionRespond } from "../lib/tauri";
import type { DaemonConnectionState, PermissionRequest, SessionOrigin } from "../types/ipc";
import "./PermissionCard.css";

export type PermissionState = "waiting" | "submitting" | "allowed" | "denied";

export const PERMISSION_LABELS: Record<PermissionState, string> = {
  waiting: "Waiting on you",
  submitting: "Sending decision…",
  allowed: "Allowed once · running",
  denied: "Denied — the turn continues without it",
};

/**
 * Human words for the tool an agent asks with.
 *
 * The wire names the tool ("Read", "Glob"). That word is implementation
 * vocabulary: it says nothing to the person deciding whether an agent may
 * touch their machine. The map is short on purpose — a tool we do not
 * recognise keeps the agent's own wording, because a wrong translation is
 * worse than an untranslated word.
 */
const PERMISSION_ACTION_LABELS: Record<string, string> = {
  read: "Read a file",
  notebookread: "Read a notebook",
  write: "Create or overwrite a file",
  edit: "Change a file",
  multiedit: "Change several files",
  notebookedit: "Change a notebook",
  bash: "Run a command",
  powershell: "Run a command",
  glob: "Find files by name",
  grep: "Search inside files",
  webfetch: "Open a web page",
  websearch: "Search the web",
  task: "Hand this to a sub-agent",
  agent: "Hand this to a sub-agent",
  todowrite: "Update the task list",
  skill: "Run a saved instruction",
};

export interface PermissionSubject {
  /** What the agent would do, in words a person can act on. */
  action: string;
  /** What it would do it to — the path the agent named — or null when it named none. */
  target: string | null;
}

/**
 * Resolve the two facts a person needs before answering: what the agent would
 * do, and to what.
 *
 * `toolTitle` is the live session's own wording for this tool call. On the
 * Claude wire it is the only place a target appears at all: the daemon builds
 * the permission request from the tool's name, so the path the agent is asking
 * about reaches the card here or not at all.
 */
export function permissionSubject(
  request: Pick<PermissionRequest, "title">,
  toolTitle?: string | null,
): PermissionSubject {
  const candidates = [toolTitle, request.title]
    .filter((value): value is string => typeof value === "string")
    .map((value) => value.trim())
    .filter((value) => value.length > 0);
  // A bare tool name is one word; anything with a space names a target, and
  // that is the more informative sentence wherever it came from.
  const sentence = candidates.find((value) => /\s/.test(value)) ?? candidates[0] ?? "";
  const separator = sentence.indexOf(" ");
  const verb = separator === -1 ? sentence : sentence.slice(0, separator);
  const label = PERMISSION_ACTION_LABELS[verb.toLowerCase()];
  if (label === undefined) {
    // No translation for this word, so the agent's own sentence stays the whole
    // heading: splitting it would print the target twice and explain nothing.
    return { action: sentence === "" ? "Permission requested" : sentence, target: null };
  }
  const remainder = separator === -1 ? "" : sentence.slice(separator + 1).trim();
  return { action: label, target: remainder.length > 0 ? remainder : null };
}

/**
 * A path identifies itself by its tail, so a long one loses its middle rather
 * than its filename. This is a character budget, not a pixel one, so the cut
 * lands in the same place whatever the host's font happens to be; the whole
 * value stays on the element's `title`.
 */
export const PERMISSION_TARGET_LIMIT = 44;

export function shortenPermissionTarget(target: string): string {
  if (target.length <= PERMISSION_TARGET_LIMIT) return target;
  const keep = PERMISSION_TARGET_LIMIT - 1;
  const head = Math.ceil(keep / 2);
  return `${target.slice(0, head)}…${target.slice(target.length - (keep - head))}`;
}

export function quotePermissionArg(value: string): string {
  if (value.length === 0 || /[\s"]/.test(value)) {
    return `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;
  }
  return value;
}

export function formatPermissionCommand(request: PermissionRequest): string | null {
  if (!request.command) return null;
  if (request.args === undefined || request.args.length === 0) return request.command;
  return [request.command, ...request.args].map(quotePermissionArg).join(" ");
}

/** How much of a device id a provenance line shows when no name resolved. */
export const PERMISSION_DEVICE_ID_LIMIT = 8;

/**
 * The head of a device id, for a line that has no display name to show.
 *
 * A device id is a UUID, and a UUID in a provenance line is noise. The head is
 * the same idea as a key fingerprint: enough to match against the Devices panel
 * without reading the whole string aloud. The card never prints the raw id.
 */
export function shortenDeviceId(deviceId: string): string {
  return deviceId.length <= PERMISSION_DEVICE_ID_LIMIT
    ? deviceId
    : `${deviceId.slice(0, PERMISSION_DEVICE_ID_LIMIT)}…`;
}

/**
 * The provenance line for a permission card, or null when the session's origin
 * is known and local.
 *
 * Three states, three renderings. A `peer` origin names the device and the
 * role. A `local` origin says nothing: the card's own wording is about the
 * agent, not the machine it runs on, and a local request is the case the card
 * had before peer sessions existed. An ABSENT origin is neither of those — the
 * daemon now stamps an origin on every request, so `undefined` can only come
 * from a daemon older than the field — and it gets the word `unknown` instead,
 * because staying silent would render it exactly like a local one.
 *
 * The line is built from the origin alone, never from the request's own text:
 * on a remote-origin turn the tool input is chosen upstream, so a header it
 * prints itself would be indistinguishable from the provenance if the two
 * shared a text run. The device is named through the workspace's `DevicesList`
 * map when it has an entry; otherwise the id's head stands in. `unknown` does
 * for a field the daemon did not send, matching `sessionStateLabel`'s
 * vocabulary — a peer origin always carries both, so that is a guard, not a
 * case in normal use.
 */
export function permissionOriginLabel(
  origin: SessionOrigin | undefined,
  deviceNames?: ReadonlyMap<string, string> | null,
): string | null {
  if (origin === undefined) return "Origin: unknown";
  if (origin.kind === "local") return null;
  const { deviceId } = origin;
  const name = deviceId === undefined ? undefined : deviceNames?.get(deviceId);
  const device = name ?? (deviceId === undefined ? "unknown" : shortenDeviceId(deviceId));
  return `Device: ${device} · Role: ${origin.role ?? "unknown"}`;
}

export interface PermissionCardProps {
  sessionId: string;
  subscriptionId: number;
  request: PermissionRequest;
  capabilities: readonly string[];
  /** Design passes the live state so a disconnected daemon never hides a waiting request. */
  daemonState?: DaemonConnectionState;
  /**
   * The live session's own wording for this tool call, when the host has it.
   * The Claude wire sends only the tool's name in the request itself, so the
   * target travels on the transcript item with the same `toolCallId`.
   */
  toolTitle?: string | null;
  /**
   * The origin of the session this request belongs to. A host that already
   * knows the session's origin passes it here; otherwise the card falls back to
   * the `origin` the daemon put on the request itself. Passing nothing while
   * the request carries nothing either is the absent-origin state — an older
   * daemon — which renders as `Origin: unknown`, not as a local request.
   */
  origin?: SessionOrigin;
  /**
   * Device id to display name, the same `DevicesList` map the session badge
   * resolves against. A peer origin names a device; handed no map, the card has
   * only the id and prints its head instead of the whole UUID.
   */
  deviceNames?: ReadonlyMap<string, string>;
  onRespond?: (outcome: "allow_once" | "deny") => Promise<void>;
  onResolved?: (sessionId: string, toolCallId: string) => void;
}

/** A real ACP permission prompt; it is inert unless the handshake negotiated typed_permissions. */
export function PermissionCard({
  sessionId,
  subscriptionId,
  request,
  capabilities,
  daemonState = "connected",
  toolTitle = null,
  origin,
  deviceNames,
  onRespond,
  onResolved,
}: PermissionCardProps) {
  const [permission, setPermission] = useState<PermissionState>("waiting");
  const [error, setError] = useState<string | null>(null);
  const submittingRef = useRef(false);
  // Set false on unmount so a late answer never stamps state (or fires
  // onResolved) after the host closed the run and removed the card.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  // A fresh subscription means the previous answer was sent over an attachment the
  // daemon no longer owns. Reset the card so the re-delivered request is answerable,
  // and bump the generation so the abandoned answer cannot overwrite this state.
  const generationRef = useRef(0);

  useEffect(() => {
    generationRef.current += 1;
    submittingRef.current = false;
    setPermission("waiting");
    setError(null);
  }, [sessionId, request.toolCallId, subscriptionId]);

  if (!capabilities.includes("typed_permissions") && daemonState === "connected") return null;

  const subject = permissionSubject(request, toolTitle);
  const provenance = permissionOriginLabel(origin ?? request.origin, deviceNames);
  const commandLine = formatPermissionCommand(request);
  const daemonReachable = daemonState === "connected";
  const allowSupported = request.options.some((option) => option.kind === "allow_once");
  const denySupported = request.options.some((option) => option.kind === "reject_once");

  const respond = async (outcome: "allow_once" | "deny") => {
    if (submittingRef.current || permission !== "waiting") return;
    const generation = generationRef.current;
    submittingRef.current = true;
    setPermission("submitting");
    setError(null);
    try {
      await (onRespond?.(outcome) ??
        sessionPermissionRespond(sessionId, subscriptionId, request.toolCallId, outcome));
      if (!mountedRef.current || generationRef.current !== generation) return;
      setPermission(outcome === "allow_once" ? "allowed" : "denied");
      onResolved?.(sessionId, request.toolCallId);
    } catch (cause) {
      if (!mountedRef.current || generationRef.current !== generation) return;
      submittingRef.current = false;
      setPermission("waiting");
      setError(reasonFromCause(cause));
    }
  };

  return (
    <div className="permission-card" aria-live="polite">
      {/* The provenance line is the card's first child and its own element
          (A14): the request's text renders below it, so a remote turn cannot
          print something that reads as it. */}
      {provenance !== null ? <div className="permission-card-origin">{provenance}</div> : null}
      <div className="permission-card-heading">
        <span className={`permission-card-dot permission-card-${permission}`} />
        <span className="permission-card-action">{subject.action}</span>
        {request.cwd ? <span className="permission-card-context">{request.cwd}</span> : null}
      </div>
      {subject.target ? (
        <div className="permission-card-subject" title={subject.target}>
          {shortenPermissionTarget(subject.target)}
        </div>
      ) : null}
      {request.description ? (
        <div className="permission-card-description">{request.description}</div>
      ) : null}
      {commandLine ? <div className="permission-card-command">{commandLine}</div> : null}
      {request.env && request.env.length > 0 ? (
        <div className="permission-card-env">
          {request.env.map((variable) => `${variable.name}=${variable.value}`).join("\n")}
        </div>
      ) : null}
      {!daemonReachable ? (
        <div className="permission-card-unavailable" role="status">
          The daemon is not reachable. Reconnect to answer this request.
        </div>
      ) : null}
      {!allowSupported || !denySupported ? (
        <div className="permission-card-unavailable" role="status">
          {!allowSupported ? "Allow once is not offered for this request." : null}
          {!allowSupported && !denySupported ? " " : null}
          {!denySupported ? "Deny is not offered for this request." : null}
        </div>
      ) : null}
      <div className="permission-card-actions">
        <span className="permission-card-label">{PERMISSION_LABELS[permission]}</span>
        <button
          type="button"
          className="permission-card-secondary-action permission-card-deny-action"
          onClick={() => void respond("deny")}
          disabled={permission !== "waiting" || !daemonReachable || !denySupported}
        >
          Deny
        </button>
        <button
          type="button"
          className="permission-card-primary-action"
          onClick={() => void respond("allow_once")}
          disabled={permission !== "waiting" || !daemonReachable || !allowSupported}
        >
          Allow once
        </button>
      </div>
      {error ? <div role="alert">{error}</div> : null}
    </div>
  );
}
