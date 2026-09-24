import { useEffect, useRef, useState } from "react";
import { boundByGraphemes } from "../lib/graphemeBound";
import { optionOutcome } from "../lib/optionOutcome";
import { sessionPermissionRespond } from "../lib/tauri";
import { errorSentence, type ErrorSentence } from "../lib/errorSentence";
import { ErrorText } from "./ErrorText";
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
 * The resolved labels when the answer provably came from the child's creator —
 * delegated answering where the daemon's `answeredBy` matches the child's own
 * `createdBy`. Both outcomes carry the attribution, and the wording says "its
 * creator" because that identity was CHECKED, not assumed: the card sits on
 * the child's surface, and the app compares the answerer with the creator the
 * roster names before it prints the word. A denial by an agent is exactly the
 * event a human reviewing the roster needs to see happened, so it must not
 * render as an unattributed one.
 */
export const PERMISSION_CREATOR_LABELS: Record<"allowed" | "denied", string> = {
  allowed: "Allowed by its creator · running",
  denied: "Denied by its creator — the turn continues without it",
};

/**
 * The labels when a session answered but the creator check does not pass —
 * either the answerer is a different session, or the app does not know this
 * child's creator and cannot verify the claim. Naming the answerer is the
 * honest rendering; "its creator" is not, because nobody checked. The head of
 * the id stands in for the whole UUID (the roster's own creator-badge rule);
 * the full id stays on the label's `title`.
 */
export const PERMISSION_ANSWERER_LABELS: Record<"allowed" | "denied", (who: string) => string> = {
  allowed: (who) => `Allowed by session ${who} · running`,
  denied: (who) => `Denied by session ${who} — the turn continues without it`,
};

/**
 * The labels when the daemon did not say who answered. Silence is not a
 * person: rendering "a person answered" off an absent field is the exact
 * collapse this card exists to prevent, so the unnamed answer is its own
 * visible state — and the human can clear the card once they have read it.
 */
export const PERMISSION_UNNAMED_LABELS: Record<"allowed" | "denied", string> = {
  allowed: "Allowed — the daemon did not say who answered · running",
  denied: "Denied — the daemon did not say who answered — the turn continues without it",
};

/**
 * The labels when the daemon did not say WHAT was chosen: `selectedOptionKind`
 * was absent, or a value this build does not know. The card states that an
 * answer happened and refuses to invent the decision — "denied" was exactly
 * what an `allow_always` used to render as, and a consent surface may not
 * guess in either direction.
 */
export const PERMISSION_UNCLAIMED_LABELS = {
  named: (who: string) => `Answered by session ${who} — allowed or denied, the daemon did not say`,
  unnamed: "Answered — by whom and with what outcome, the daemon did not say",
};

/**
 * The three states an outside answer's attribution can be in: `creator`
 * (answerer === the child's `createdBy`), `other` (a named session that is
 * not the creator, or a creator the roster does not know), `unnamed` (the
 * daemon said nothing). The outcome is a separate axis — a resolution can
 * name the answerer and still not name the decision.
 */
type ResolutionAttribution = "creator" | "other" | "unnamed";

/**
 * The decision the daemon's selected option kind maps to, walked as a table
 * over the closed permission vocabulary — the same mapping the daemon's
 * broker owns (`allow_once | allow_always → Allow`, `reject_once |
 * reject_always → Deny`). Keyed by the raw wire string, so an unknown or
 * absent kind misses the table and yields `undefined`: the caller renders an
 * unclaimed answer, never a guessed decision. THIS TABLE IS THE FORK POINT —
 * when the daemon's vocabulary grows, this is the line that must grow with
 * it, and the outcome test walks every row plus the unknown one.
 */
export const OUTCOME_BY_OPTION_KIND: Record<string, "allowed" | "denied"> = {
  allow_once: "allowed",
  allow_always: "allowed",
  reject_once: "denied",
  reject_always: "denied",
};

/** The outcome a resolution claims, or null when the kind named none. */
export function resolutionOutcome(
  selectedOptionKind: string | undefined | null,
): "allowed" | "denied" | null {
  if (selectedOptionKind === undefined || selectedOptionKind === null) return null;
  return OUTCOME_BY_OPTION_KIND[selectedOptionKind] ?? null;
}

/** How much of an answerer's session id the attribution line shows. */
export const PERMISSION_ANSWERER_ID_LIMIT = 8;

/** The head of an answerer's session id, the roster's creator-badge rule.
 * Bounded by grapheme clusters — a unit-based cut halves an astral scalar
 * and renders U+FFFD (re-audit F12). */
export function shortenAnswererId(answeredBy: string): string {
  return boundByGraphemes(answeredBy, PERMISSION_ANSWERER_ID_LIMIT);
}

/**
 * The resolved label, from a table the render walks — never a chain of
 * `===`. The attribution axis decides WHO may be claimed; the outcome axis
 * decides WHAT is claimed; when the outcome is null (an unknown or absent
 * option kind) the card claims no decision at all and says so.
 */
const RESOLVED_LABELS: Record<
  ResolutionAttribution,
  Record<"allowed" | "denied", (who: string) => string>
> = {
  creator: {
    allowed: () => PERMISSION_CREATOR_LABELS.allowed,
    denied: () => PERMISSION_CREATOR_LABELS.denied,
  },
  other: {
    allowed: (who) => PERMISSION_ANSWERER_LABELS.allowed(who),
    denied: (who) => PERMISSION_ANSWERER_LABELS.denied(who),
  },
  unnamed: {
    allowed: () => PERMISSION_UNNAMED_LABELS.allowed,
    denied: () => PERMISSION_UNNAMED_LABELS.denied,
  },
};

export function resolvedCardLabel(
  attribution: ResolutionAttribution,
  outcome: "allowed" | "denied" | null,
  who: string | null,
): string {
  if (outcome === null) {
    return who === null
      ? PERMISSION_UNCLAIMED_LABELS.unnamed
      : PERMISSION_UNCLAIMED_LABELS.named(who);
  }
  return RESOLVED_LABELS[attribution][outcome](who ?? "");
}

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
 * because staying silent would render it exactly like a local one. A `kind` of
 * `unknown`, and any other kind string this build does not know, take that same
 * line: `local` is the one case that stays silent, and no unknown may fall into
 * it.
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
  if (origin.kind !== "peer") return "Origin: unknown";
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
  /**
   * An outside answer that arrived while this card was waiting: its creator
   * resolved it through delegated answering. The card stays on screen and
   * renders the attributed label — it does not vanish, and the person does
   * not answer it again. Null keeps the card answerable, as before.
   *
   * `outcome` is null when the daemon did not say what was chosen (absent or
   * unknown `selectedOptionKind`) — the card claims no decision. `answeredBy`
   * is null when the daemon did not say who — the card claims no answerer,
   * and "a person answered" is never read into the silence.
   * `selectedOptionName` is the daemon's own word for the option that was
   * chosen, kept so the card can say which choice the answer was.
   */
  resolution?: {
    outcome: "allowed" | "denied" | null;
    answeredBy: string | null;
    selectedOptionName?: string | null;
  } | null;
  /**
   * The session id that created THIS session, as the roster carries it — the
   * only fact "answered by its creator" can be checked against. Absent or
   * null means the roster does not know the creator, so a named answerer can
   * never earn the creator label, only the named-session one.
   */
  creatorId?: string | null;
  onRespond?: (outcome: "allow_once" | "deny", optionId?: string) => Promise<void>;
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
  resolution = null,
  creatorId = null,
  onRespond,
  onResolved,
}: PermissionCardProps) {
  const [permission, setPermission] = useState<PermissionState>("waiting");
  const [error, setError] = useState<ErrorSentence | null>(null);
  // The option a chooser answer picked, under the agent's own option name:
  // set when THIS card answers with an option id, so its resolved state says
  // which choice it was. An outside answer's name arrives on `resolution`.
  const [localChoice, setLocalChoice] = useState<string | null>(null);
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
    setLocalChoice(null);
  }, [sessionId, request.toolCallId, subscriptionId]);

  if (!capabilities.includes("typed_permissions") && daemonState === "connected") return null;

  const subject = permissionSubject(request, toolTitle);
  const provenance = permissionOriginLabel(origin ?? request.origin, deviceNames);
  const commandLine = formatPermissionCommand(request);
  const daemonReachable = daemonState === "connected";
  const allowSupported = request.options.some((option) => option.kind === "allow_once");
  const denySupported = request.options.some((option) => option.kind === "reject_once");
  // The daemon's chooser verdict, read and never re-derived here: a marked
  // request renders one control per option, an unmarked one the ordinary pair.
  const isChooser = request.isChooser === true;
  // The agent refused by offering a reject option of its own; only when it
  // did not does the card keep its plain Deny.
  const hasRejectOption = request.options.some((option) => option.kind.startsWith("reject"));
  // Which option the answer was, from whichever side said it: the daemon's
  // name on an outside resolution, or the option this card clicked.
  const chosenName = resolution?.selectedOptionName || localChoice;
  // The creator answered elsewhere: the card resolves with the attribution
  // on it, an answerer the label can name, and a way to clear it once read.
  // The dot keeps the outcome's colour so the state is readable at the same
  // glance as the words — except when no outcome was claimed, where the dot
  // goes neutral: an answered card with no claimed decision is not a denial
  // and must not wear one's colour.
  const resolvedByCreator = resolution !== null;
  const attribution: ResolutionAttribution =
    resolution === null || resolution.answeredBy === null
      ? "unnamed"
      : creatorId !== null && resolution.answeredBy === creatorId
        ? "creator"
        : "other";
  const answererHead =
    resolution?.answeredBy === null || resolution?.answeredBy === undefined
      ? null
      : shortenAnswererId(resolution.answeredBy);
  const cardLabel = resolvedByCreator
    ? resolvedCardLabel(attribution, resolution.outcome, answererHead)
    : PERMISSION_LABELS[permission];
  const cardTone = resolvedByCreator ? (resolution.outcome ?? "unclaimed") : permission;

  const respond = async (outcome: "allow_once" | "deny", optionId?: string) => {
    if (resolvedByCreator || submittingRef.current || permission !== "waiting") return;
    const generation = generationRef.current;
    submittingRef.current = true;
    setPermission("submitting");
    setError(null);
    try {
      // The ordinary pair posts no option id: its options are unambiguous,
      // so the daemon's own pick is the right one. A chooser answer names the
      // option it is answering with.
      await (onRespond?.(outcome, optionId) ??
        (optionId === undefined
          ? sessionPermissionRespond(sessionId, subscriptionId, request.toolCallId, outcome)
          : sessionPermissionRespond(
              sessionId,
              subscriptionId,
              request.toolCallId,
              outcome,
              optionId,
            )));
      if (!mountedRef.current || generationRef.current !== generation) return;
      setPermission(outcome === "allow_once" ? "allowed" : "denied");
      if (optionId !== undefined) {
        setLocalChoice(
          request.options.find((option) => option.optionId === optionId)?.name ?? null,
        );
      }
      onResolved?.(sessionId, request.toolCallId);
    } catch (cause) {
      if (!mountedRef.current || generationRef.current !== generation) return;
      submittingRef.current = false;
      setPermission("waiting");
      setError(errorSentence(cause));
    }
  };

  return (
    <div className="permission-card" aria-live="polite">
      {/* The provenance line is the card's first child and its own element
          (A14): the request's text renders below it, so a remote turn cannot
          print something that reads as it. */}
      {provenance !== null ? <div className="permission-card-origin">{provenance}</div> : null}
      <div className="permission-card-heading">
        <span className={`permission-card-dot permission-card-${cardTone}`} />
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
      {!isChooser && (!allowSupported || !denySupported) ? (
        <div className="permission-card-unavailable" role="status">
          {!allowSupported ? "Allow once is not offered for this request." : null}
          {!allowSupported && !denySupported ? " " : null}
          {!denySupported ? "Deny is not offered for this request." : null}
        </div>
      ) : null}
      {chosenName ? <div className="permission-card-choice">Chosen: {chosenName}</div> : null}
      <div className="permission-card-actions">
        <span
          className="permission-card-label"
          title={resolution?.answeredBy != null ? resolution.answeredBy : undefined}
        >
          {cardLabel}
        </span>
        {resolvedByCreator ? (
          <button
            type="button"
            className="permission-card-secondary-action permission-card-dismiss-action"
            // The one control a resolved card keeps: without it, an outside
            // answer sits in the queue for the life of the app — nothing else
            // can remove it, and two hundred answered cards are two hundred
            // permanent fixtures.
            aria-label="Clear this answered card"
            onClick={() => onResolved?.(sessionId, request.toolCallId)}
          >
            Clear
          </button>
        ) : isChooser ? (
          <>
            {!hasRejectOption ? (
              <button
                type="button"
                className="permission-card-secondary-action permission-card-deny-action"
                // The agent offered no way to refuse, so the card keeps its
                // own Deny: a question the person will not answer can still
                // be refused.
                onClick={() => void respond("deny")}
                disabled={permission !== "waiting" || !daemonReachable}
              >
                Deny
              </button>
            ) : null}
            {request.options.map((option) => {
              const outcome = optionOutcome(option.kind);
              return (
                <button
                  key={option.optionId}
                  type="button"
                  className={
                    outcome === "deny"
                      ? "permission-card-secondary-action permission-card-deny-action"
                      : "permission-card-primary-action"
                  }
                  onClick={() => void respond(outcome, option.optionId)}
                  disabled={permission !== "waiting" || !daemonReachable}
                >
                  {option.name}
                </button>
              );
            })}
          </>
        ) : (
          <>
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
          </>
        )}
      </div>
      {error ? (
        <div role="alert">
          <ErrorText sentence={error.sentence} detail={error.detail} id="permission-error" />
        </div>
      ) : null}
    </div>
  );
}
