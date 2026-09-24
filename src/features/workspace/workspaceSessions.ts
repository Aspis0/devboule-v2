import { useCallback, useEffect, useState } from "react";
import {
  createSessionStateChannel,
  sessionCreate,
  sessionsList,
  sessionsUnwatch,
  sessionsWatch,
} from "../../lib/tauri";
import type {
  AttentionReason,
  DelegationState,
  PeerRow,
  ProviderInfo,
  Session,
  SessionKind,
  SessionStateSnapshot,
  UnattendedState,
} from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";
import { boundByGraphemes } from "../../lib/graphemeBound";

export interface WorkspaceSessionSource {
  list: () => Promise<Session[]>;
  create: (
    workspaceId: string | null,
    kind?: SessionKind,
    provider?: string | null,
  ) => Promise<Session>;
  watch?: (listener: (snapshots: SessionStateSnapshot[]) => void) => Promise<() => void>;
}

export interface WorkspaceSessionState {
  sessions: Session[];
  selectedSessionId: string | null;
  loading: boolean;
  creating: boolean;
  error: string | null;
}

export interface WorkspaceSessionController {
  getState: () => WorkspaceSessionState;
  subscribe: (listener: () => void) => () => void;
  refresh: () => Promise<void>;
  create: (
    kind?: SessionKind,
    provider?: string | null,
    workspaceId?: string | null,
  ) => Promise<Session | null>;
  select: (sessionId: string | null) => void;
  open: (session: Session) => void;
  watch: () => () => void;
  reconnect: () => Promise<void>;
  dismissError: () => void;
}

const DEFAULT_SOURCE: WorkspaceSessionSource = {
  list: sessionsList,
  create: (workspaceId, kind = "acp", provider = null) =>
    provider == null
      ? sessionCreate(workspaceId, kind)
      : sessionCreate(workspaceId, kind, provider),
  watch: async (listener) => {
    const channel = createSessionStateChannel(listener);
    await sessionsWatch(channel);
    return () => {
      void sessionsUnwatch();
    };
  },
};

const LIST_ERROR = "Could not load sessions. The daemon is unreachable.";
const CREATE_FALLBACK_ERROR = "Could not create the agent session.";

/**
 * What belongs in the tab strip: a running process (live/silent) plus a
 * recovered transcript. Attaching is reading — replay from the journal, no
 * process, no cost — so recovered rows appear by themselves. Ended rows stay
 * out: they remain reachable from History and join only once opened there.
 * Resuming (a child process, a new bearer, a live turn) never happens here.
 */
function sessionHasProcess(session: Session): boolean {
  return session.state.type === "live" || session.state.type === "silent";
}

export function isRecoveredSession(session: Pick<Session, "state">): boolean {
  return session.state.type === "recovered";
}

/**
 * Message from a rejected invoke. Tauri rejections are not always `Error`s:
 * the daemon's serialized WireError arrives as a plain
 * `{ code, message }` object, which `String(cause)` would render as
 * "[object Object]".
 */
function rejectionMessage(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  if (typeof cause === "object" && cause !== null && "message" in cause) {
    const message = (cause as { message: unknown }).message;
    if (typeof message === "string") return message;
  }
  return String(cause);
}

export function workspaceSessions(sessions: readonly Session[]): Session[] {
  return [...sessions];
}

function formatElapsed(elapsedMs: number): string {
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes > 0) return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  const seconds = Math.floor(elapsedMs / 1_000);
  return `${seconds} second${seconds === 1 ? "" : "s"}`;
}

export function sessionStateLabel(state: unknown, elapsedMs?: number | null): string {
  if (typeof state !== "object" || state === null || !("type" in state)) return "unknown";
  const type = state.type;
  if (type === "silent") {
    return typeof elapsedMs === "number"
      ? `silent · ${formatElapsed(elapsedMs)}`
      : "silent · duration unknown";
  }
  if (type === "live") return "live";
  if (type === "ended" || type === "recovered") {
    if ("integrity" in state && typeof state.integrity === "object" && state.integrity !== null) {
      const integrity = state.integrity;
      if (
        "kind" in integrity &&
        (integrity.kind === "truncated" || integrity.kind === "unverifiable")
      ) {
        return `${type} · ${integrity.kind}`;
      }
    }
    return type;
  }
  return "unknown";
}

export function sessionDotTone(state: unknown): "green" | "terracotta" | "border" {
  const label = sessionStateLabel(state);
  if (label === "live") return "green";
  if (label.startsWith("silent")) return "border";
  if (label.startsWith("recovered") || label.startsWith("ended ·")) return "border";
  return "terracotta";
}

/**
 * Human words for why a session wants attention. Rendered inside the tab
 * button so the reason is part of the tab's accessible name, not only its
 * colour.
 */
export function sessionAttentionLabel(reason: AttentionReason): string {
  if (reason === "permission") return "needs approval";
  return reason;
}

/**
 * The name a session is shown under, at the four places that name one: the tab
 * strip's label and the selected session's panel title (`Workspace.tsx`), a
 * History row (`HistoryPanel.tsx`), and the History search, which matches what
 * a row shows (`historyGrouping.ts`). No other surface calls it. A name derived
 * anywhere else is a second name for one session, and a session with two names
 * is exactly the bug this function exists to close: History said `worker` while
 * the tab strip said `worker one`.
 *
 * A session's own name — `Session.displayName`, set once by whoever created it
 * (a human naming a session, an agent naming the child it commissioned) —
 * outranks anything derived from the row. The fallback is unchanged and still
 * the common path: a session a person started carries no display name, and a
 * session recovered from an older journal comes back without one, which is
 * exactly the gap `title` then the kind-derived name cover.
 */
export function sessionTitle(
  session: Pick<Session, "id" | "title" | "kind" | "displayName">,
): string {
  const displayName = session.displayName?.trim();
  if (displayName) return displayName;
  const title = session.title.trim();
  if (title) return title;
  // The id fallback bounds by grapheme clusters, like the `created by` badge
  // and the answerer head: a unit-based cut halves an astral scalar and
  // renders U+FFFD in the strip (audit 3, F10 — the sibling of :275-276).
  return `${isAgentKind(session.kind) ? "Agent" : "Terminal"} ${boundByGraphemes(session.id, 8)}`;
}

/**
 * The display name for each paired device, keyed by device id. This is the
 * `DevicesList` map the session badge resolves against; revoked rows are kept,
 * because a session started by a device that has since been revoked still
 * belongs to it.
 */
export function peerDeviceNames(peers: readonly PeerRow[]): Map<string, string> {
  return new Map(peers.map((peer) => [peer.deviceId, peer.displayName]));
}

/**
 * The display name for each session the roster knows, keyed by session id. This
 * is the session-side twin of `peerDeviceNames`, and it exists for the same
 * reason: a `createdBy` is a session id and nothing more, so the badge that
 * names a child's creator has to resolve it against rows the app already holds.
 *
 * A session with no display name is left out rather than mapped to its title:
 * the map answers one question — has the roster named this session? — and a
 * title is not a name the daemon wrote. An id missing from the map is a
 * question the caller answers with the id itself, never with silence.
 */
export function sessionDisplayNames(sessions: readonly Session[]): Map<string, string> {
  return new Map(
    sessions.flatMap((session) => {
      const name = session.displayName?.trim();
      return name ? [[session.id, name] as const] : [];
    }),
  );
}

/** The badge for a session whose origin the daemon did not send at all. */
const UNKNOWN_ORIGIN_BADGE = "origin unknown";

/**
 * True when the app does not know where this session came from: the daemon sent
 * no origin at all, or it sent a kind this build cannot read. The daemon now
 * stamps an origin on every session, so an absent one only comes from an older
 * daemon; a kind of `unknown` is the daemon's own word for a journal row whose
 * origin column it could not interpret. Both are unknown provenance, never
 * local, and they get the same badge and the same mark on it.
 */
export function sessionOriginUnknown(session: Pick<Session, "origin">): boolean {
  const origin = session.origin;
  if (origin === undefined) return true;
  return origin.kind !== "local" && origin.kind !== "peer";
}

/**
 * The tab badge for a session of remote origin, or null for a local one.
 *
 * The device is named, never guessed: the daemon stamps only the device id on
 * the origin, and the name comes from the devices list the workspace holds. An
 * id the list does not know yet falls back to the id itself, which is still a
 * true statement about where the session came from. A peer origin that names no
 * device reads `from unknown` — the card's word for a field the daemon did not
 * send — rather than yielding no badge: the session still came from a peer, and
 * silence is the one reading the tab may not give.
 *
 * An absent origin is a third state, not a local one: the tab says `origin
 * unknown` rather than staying silent, which would read as a local session. It
 * is not this badge: a peer whose device is unknown is still a named peer. A
 * `kind` of `unknown` — and any other kind string this build does not know —
 * gets that same badge and that same `-unknown` mark, so `local` stays the only
 * silent kind.
 */
export function sessionOriginBadge(
  session: Pick<Session, "origin">,
  deviceNames: ReadonlyMap<string, string>,
): string | null {
  if (sessionOriginUnknown(session)) return UNKNOWN_ORIGIN_BADGE;
  const origin = session.origin;
  if (origin?.kind === "local") return null;
  if (origin?.kind !== "peer") return UNKNOWN_ORIGIN_BADGE;
  const { deviceId } = origin;
  const name = deviceId === undefined ? undefined : deviceNames.get(deviceId);
  const device = name ?? (deviceId === undefined ? "unknown" : deviceId);
  return `from ${device}`;
}

/**
 * The tab badge for a session an agent created, or null for one a person
 * started (`createdBy` absent — which is also every row written before the
 * daemon kept the field).
 *
 * The creator is a session id, so it is resolved against the roster's own
 * display names: a child of a named session reads `created by <that name>`. An
 * id no row has named yet — the creator is not in the roster at all, or it has
 * no display name of its own — falls back to the same short id prefix the
 * title fallback uses, because the child does have a creator and saying so is
 * still true.
 */
export function sessionCreatorBadge(
  session: Pick<Session, "createdBy">,
  creatorNames: ReadonlyMap<string, string>,
): string | null {
  const createdBy = session.createdBy?.trim();
  if (!createdBy) return null;
  const name = creatorNames.get(createdBy);
  // The id fallback bounds by grapheme clusters, like the permission card's
  // answerer head — a unit-based slice halves an astral scalar (re-audit F12).
  return `created by ${name ?? boundByGraphemes(createdBy, 8)}`;
}

/** What a row's delegation pill says, and how loudly. */
export interface DelegationBadge {
  /** `unattended` is the loud one; `unknown` the softer, present one. */
  tone: "unattended" | "unknown" | "active";
  label: string;
}

/** The loud pill's words, decided: it is what a person reads when they come
 * back to find agents ran all night. A fact of the child's birth — never
 * derived from the live switch, which a human can turn off while such a child
 * keeps running without asking. */
export const UNATTENDED_BADGE_LABEL = "runs unattended · created in an auto-accepting profile";

/** The softer marker's words: present, because "the daemon could not
 * establish" must never collapse into nothing — that is the collapse, in
 * pixels. */
export const UNATTENDED_UNKNOWN_BADGE_LABEL = "may run without asking — cannot establish";

/** The delegation ledger's own unknown marker: the row is a child, but no push
 * has said who answers it (or the state it sent is one this build cannot
 * read). Present, softer, and never collapsed into "off" — a fifth state
 * reading as nobody-answers is the roster's version of the confirmed defect. */
export const DELEGATION_UNKNOWN_BADGE_LABEL =
  "answering unknown — the daemon did not say who answers";

/**
 * The table the delegation state walks — one row per value of
 * `DelegationState["state"]`, so adding a union member without a rendering
 * decision is a compile error, not a silent benign render. Keyed at runtime by
 * the raw wire string (`delegationStateRow`), so a value from a newer daemon —
 * one TypeScript cannot have predicted — falls to the visible unknown badge
 * instead of falling out of the table into nothing.
 */
type KnownDelegationState = DelegationState["state"];
const DELEGATION_STATE_BADGES: Record<
  KnownDelegationState,
  (answered: number) => DelegationBadge | null
> = {
  // Off is a definite answer — the child exists, nobody answers for it — and
  // it renders nothing delegation-specific. Silence here is the truth, not
  // the collapse: the collapse is an UNKNOWN value taking this row.
  off: () => null,
  active: (answered) => ({
    tone: "active",
    label: `answers to its creator${answered > 0 ? ` · answered ×${answered}` : ""}`,
  }),
  unattended: () => ({ tone: "unattended", label: UNATTENDED_BADGE_LABEL }),
  unknown: () => ({ tone: "unknown", label: DELEGATION_UNKNOWN_BADGE_LABEL }),
};

/** The row an out-of-union wire value takes: present, softer, never benign. */
const UNKNOWN_WIRE_STATE_BADGE: (answered: number) => DelegationBadge | null = () => ({
  tone: "unknown",
  label: DELEGATION_UNKNOWN_BADGE_LABEL,
});

function delegationStateRow(state: string, answered: number): DelegationBadge | null {
  const row = Object.hasOwn(DELEGATION_STATE_BADGES, state)
    ? DELEGATION_STATE_BADGES[state as KnownDelegationState]
    : UNKNOWN_WIRE_STATE_BADGE;
  return row(answered);
}

/**
 * The tri-state marker's pill, one row per wire value of `UnattendedState` —
 * the walked-table rule the delegation ledger's state already follows, applied
 * to the field one column over on the same push. Adding a union member without
 * a rendering decision is a compile error; a value from a newer or misbehaving
 * daemon (`"unspecified"`, a fifth spelling) is keyed by its raw string and
 * falls to a visible arm, never out of a `===` chain into the silence that
 * reads as "a person answers for this row".
 */
const UNATTENDED_PILLS: Record<UnattendedState, DelegationBadge | null> = {
  // `no` is silent on purpose: the daemon said somebody must ask, and the
  // active badge (or the plain row) is the truth.
  no: null,
  yes: { tone: "unattended", label: UNATTENDED_BADGE_LABEL },
  unknown: { tone: "unknown", label: UNATTENDED_UNKNOWN_BADGE_LABEL },
};

/** The row an out-of-union tri-state takes: present, softer, never benign. */
const UNATTENDED_UNREADABLE_PILL: DelegationBadge = {
  tone: "unknown",
  label: UNATTENDED_UNKNOWN_BADGE_LABEL,
};

function unattendedPill(value: string): DelegationBadge | null {
  return Object.hasOwn(UNATTENDED_PILLS, value)
    ? UNATTENDED_PILLS[value as UnattendedState]
    : UNATTENDED_UNREADABLE_PILL;
}

/**
 * How much warning each tri-state value carries, for the ratchet below: the
 * marker is a fact of the session's birth, never re-derived and never
 * downgraded — the daemon ratchets it the same way, and the downgrade
 * direction is the one that removes a warning. A value this build cannot read
 * ranks with `unknown`: it may be a warning, and nothing readable may erase it.
 */
const UNATTENDED_RANK: Record<UnattendedState, number> = { no: 0, unknown: 1, yes: 2 };

function unattendedRank(value: UnattendedState): number {
  return Object.hasOwn(UNATTENDED_RANK, value) ? UNATTENDED_RANK[value] : 1;
}

/**
 * Merges one tri-state observation into the row's known one. Absence lets the
 * known value stand; a readable value wins only when it warns at least as
 * loudly as the one it would replace — so `no` can never erase `unknown`
 * (re-audit F8) any more than it could erase `yes`.
 */
function ratchetUnattended(
  previous: UnattendedState | undefined,
  next: UnattendedState | undefined,
): UnattendedState | undefined {
  if (next === undefined) return previous;
  if (previous === undefined) return next;
  return unattendedRank(next) >= unattendedRank(previous) ? next : previous;
}

/**
 * The delegation pills for a roster row, in render order; empty for every row
 * that owes none.
 *
 * Four cases on `delegation`, one per truth, and the absence is one of them:
 *
 * - `delegation` absent → **nothing**. Absent means this session is not an
 *   agent-created child (or no push has described it yet) — never "delegation
 *   is off". An "off" badge here is the absent-into-none collapse wearing a
 *   roster badge, and a pill on a human-started session is the same lie.
 * - `state: "off"` → nothing delegation-specific from the state itself. The
 *   child exists, nobody answers for it; that is today's ordinary shape.
 * - `state: "active"` → the quiet pill naming who answers, with the count of
 *   answered cards once one exists.
 * - `state: "unattended"` → the loud pill, **independent of the live
 *   switch**: the child was born able to run without asking and keeps that
 *   ability after a human turns delegation off, so this pill is the only
 *   thing telling them which sessions those are. Deriving it from the
 *   current setting is the named red mutation.
 * - any other `state` — `"unknown"`, or a value from a newer daemon this
 *   build cannot read — → the softer, present `unknown` pill. It is the one
 *   rendering a state value may never take the shape of: nothing. A fifth
 *   value rendering as a human-started row is the roster's version of the
 *   confirmed defect, and the table above is what closes it.
 *
 * Beside those, the tri-state marker (`DESIGN-what-unattended-means.md`): a
 * row whose `unattended` is `"yes"` is the loud pill's row even before a push
 * carries the ledger (the list carries the marker too), and `"unknown"`
 * renders the softer, present pill — additively, because a creator can be
 * answering a child whose mode it cannot establish — since "the daemon could
 * not establish" must never collapse into nothing. That is the collapse, in
 * pixels. `no`, and absent, render nothing here.
 */
export function sessionDelegationBadges(
  session: Pick<Session, "delegation" | "unattended">,
): DelegationBadge[] {
  const badges: DelegationBadge[] = [];
  const delegation = session.delegation;
  // The pill the row's tri-state earns. Absent renders nothing (nothing has
  // said otherwise about a row this app holds); "no" is silent on purpose;
  // "unknown" AND any value this build cannot read take the softer present
  // marker — an out-of-union value is not a "no", and falling into that
  // silence is the collapse the walked table exists to prevent.
  const pill = session.unattended === undefined ? null : unattendedPill(session.unattended);
  if (delegation === undefined) {
    // Not described by a push yet — but the list-carried tri-state is still
    // a birth fact this row owes its marker for: yes is the loud pill's row,
    // unknown (and unreadable) the softer present one. Never "no": that
    // value stays silent.
    if (pill !== null) badges.push(pill);
    return badges;
  }
  const stateBadge = delegationStateRow(delegation.state, delegation.answered);
  if (pill?.tone === "unattended" && delegation.state !== "unattended") {
    // The birth fact renders the loud pill in the ledger state's place — the
    // pill appears once, and never as two pills for one row.
    badges.push(pill);
  } else if (stateBadge !== null) {
    badges.push(stateBadge);
  }
  if (pill?.tone === "unknown") badges.push(pill);
  return badges;
}

/**
 * Whether this row is one a take-back can act on: a child whose creator
 * answers for it (`active`) — the one case where flipping the setting ends an
 * answering relationship that exists right now. Every other row says no,
 * because a control that cannot act is noise that trains the eye to ignore
 * the pill beside it:
 *
 * - `unattended` — the child was born able to run without asking and KEEPS
 *   that ability after a human turns delegation off, so the click cannot do
 *   what the button's own accessible name promises. The loud pill stays; the
 *   empty control does not.
 * - `off` — there is no answering relationship to end.
 * - `unknown` — the app cannot say whether a relationship exists, so it
 *   cannot promise the click acts; refusal beats a guess.
 * - no `delegation` at all, and any state value this build cannot read — no.
 *
 * This is the row-side condition only. The caller adds the setting being on —
 * the whole point of the control is that it is global, so it reads the one
 * switch, never a per-row fact.
 */
const TAKE_BACK_BY_STATE: Record<KnownDelegationState, boolean> = {
  off: false,
  active: true,
  unattended: false,
  unknown: false,
};

export function sessionDelegationTakeBack(session: Pick<Session, "delegation">): boolean {
  const state = session.delegation?.state;
  if (state === undefined) return false;
  return Object.hasOwn(TAKE_BACK_BY_STATE, state)
    ? TAKE_BACK_BY_STATE[state as KnownDelegationState]
    : false;
}

/**
 * Where a roster badge for the A2A `input_required` state would go — measured
 * and deliberately not written.
 *
 * `input_required` is not a roster fact. The daemon reports the task state of a
 * created child to its **creator**, in the finish envelope's `state:` line and
 * in the structured `child_finished` event that mirrors it
 * (`crates/devboule-daemon/src/session_envelopes.rs`, `agent_input_required_envelope`);
 * the parked-child notice has no event of its own, and `SessionStateSnapshot`
 * (`crates/devboule-protocol/src/session.rs`) carries no task state at all. The
 * one roster-level fact behind "this child is parked on a card a person has to
 * answer" is the attention the daemon raises for the child's own row
 * (`SessionRuntime::raise_attention_for_event`, reason `permission`), which the
 * tab already paints as `needs approval` beside the title.
 *
 * So a badge here said one thing twice in one strip, in a pill capped at 16ch.
 * If the roster is ever to name the A2A state, the state has to arrive on the
 * roster first — that is a wire change, not this function.
 */

/**
 * Merges one listed row with what earlier pushes and lists already said about
 * the same session. The list is authoritative for what it carries (title,
 * state, elapsed, attention, an explicit ledger) and stands in for nothing it
 * omits — the same rules `applySnapshot` applies to a pushed roster, because
 * `refresh()` erases rows just as a push replaces them (re-audit F5: the
 * known-child mint existed only on the push path, so every session exit and
 * daemon reconnect re-rendered a child as a human-started row until the next
 * push). Creator, origin and the tri-state are identity and birth facts, not
 * list state: an omitting list lets the row's known values stand, and a known
 * child no source has described still mints the unknown ledger.
 */
function carrySession(listed: Session, previous: Session | undefined): Session {
  const createdBy = listed.createdBy ?? previous?.createdBy;
  const knownChild = createdBy !== undefined || previous?.delegation !== undefined;
  return {
    ...listed,
    createdBy,
    origin: listed.origin ?? previous?.origin,
    delegation:
      listed.delegation ??
      previous?.delegation ??
      (knownChild ? { answered: 0, state: "unknown" as const } : undefined),
    unattended: ratchetUnattended(previous?.unattended, listed.unattended),
  };
}

export function createWorkspaceSessionController(
  source: WorkspaceSessionSource = DEFAULT_SOURCE,
): WorkspaceSessionController {
  let state: WorkspaceSessionState = {
    sessions: [],
    selectedSessionId: null,
    loading: true,
    creating: false,
    error: null,
  };
  let refreshGeneration = 0;
  // Refreshes between "started" and "settled". `loading` belongs to the newest
  // of them; the counter is what lets a superseded refresh know whether some
  // newer refresh still owns the flag it may no longer clear.
  let refreshesInFlight = 0;
  const listeners = new Set<() => void>();
  // Ids the user opened explicitly (from History) in this app run. They keep
  // their tab even when the daemon reports no running process.
  const openedIds = new Set<string>();
  let watchLeases = 0;
  let watchPromise: Promise<() => void> | null = null;
  let watchStop: (() => void) | null = null;

  const publish = (next: WorkspaceSessionState): void => {
    state = next;
    for (const listener of listeners) listener();
  };

  const stripSessions = (candidates: readonly Session[]): Session[] =>
    candidates.filter(
      (session) =>
        sessionHasProcess(session) || isRecoveredSession(session) || openedIds.has(session.id),
    );

  const chooseSelected = (
    candidates: readonly Session[],
    preferred: string | null,
  ): string | null =>
    preferred !== null && candidates.some((session) => session.id === preferred)
      ? preferred
      : (candidates[0]?.id ?? null);

  const refresh = async (): Promise<void> => {
    const generation = ++refreshGeneration;
    refreshesInFlight += 1;
    publish({ ...state, loading: true, error: null });
    try {
      // The list merges with what the app already knows (see `carrySession`);
      // it never publishes rows verbatim, or every refresh would strip the
      // ledger, creator and birth facts earlier pushes landed.
      const known = new Map(state.sessions.map((session) => [session.id, session]));
      const listed = stripSessions(
        workspaceSessions(await source.list()).map((row) => carrySession(row, known.get(row.id))),
      );
      if (generation !== refreshGeneration) return;
      publish({
        ...state,
        sessions: listed,
        selectedSessionId: chooseSelected(listed, state.selectedSessionId),
        loading: false,
        error: null,
      });
    } catch {
      if (generation !== refreshGeneration) return;
      publish({ ...state, loading: false, error: LIST_ERROR });
    } finally {
      refreshesInFlight -= 1;
      // A superseded refresh must not leave the flag it published dangling:
      // create() and open() bump the generation without touching `loading`,
      // so a list answer dropped after them would otherwise leave the strip
      // saying "Loading sessions…" until the next push (audit 3, F12). Clear
      // it here only when no newer refresh is in flight to own the flag —
      // one that is, publishes `loading: false` for itself when it settles.
      if (generation !== refreshGeneration && refreshesInFlight === 0 && state.loading) {
        publish({ ...state, loading: false });
      }
    }
  };

  const applySnapshot = (snapshots: SessionStateSnapshot[]): void => {
    // A pushed roster is authoritative. Cancel an older list response so a
    // slow initial request cannot put the tab strip back behind the daemon.
    ++refreshGeneration;
    const known = new Map(state.sessions.map((session) => [session.id, session]));
    const sessions = snapshots.map((snapshot): Session => {
      const previous = known.get(snapshot.id);
      // A child the app can already identify — the push names its creator, or
      // an earlier push described its ledger. "Absent means not a child" is
      // only true while nothing has said otherwise; for a row this app KNOWS
      // is a child, an omitting push is the daemon failing to describe it,
      // and the honest render is the carried ledger or the unknown marker —
      // never the benign nothing of a human-started row.
      const createdBy = snapshot.createdBy ?? previous?.createdBy;
      const knownChild = createdBy !== undefined || previous?.delegation !== undefined;
      const delegation =
        snapshot.delegation ??
        previous?.delegation ??
        (knownChild ? { answered: 0, state: "unknown" as const } : undefined);
      const carried = {
        title: snapshot.title,
        state: snapshot.state,
        elapsedMs: snapshot.elapsedMs,
        // Attention comes and goes with each roster push; assigning it
        // (even undefined) keeps a stale badge from surviving a cleared one.
        attention: snapshot.attention,
        // The delegation ledger rides the explicit value: `active` must
        // become `off` the moment a push SAYS so. A push that says nothing
        // does not un-say what an earlier one said — it carries the last
        // described ledger forward, and mints `unknown` for a known child no
        // push has ever described — so a known child can never render as a
        // session nobody answers for (the collapse audit P2.12 named).
        delegation,
        // The creator is identity, like origin: a push that carries it lands
        // it, and a push that omits it lets the row's known value stand.
        createdBy: snapshot.createdBy ?? previous?.createdBy,
        // Origin is session identity, not roster state: a push that stops
        // carrying it (or never did) must not erase what the list already
        // said about a row the app is holding, so the previous value stands in.
        origin: snapshot.origin ?? previous?.origin,
        // The unattended marker is a fact of the session's birth, never
        // re-derived and never downgraded (the daemon ratchets it the same
        // way). Absence lets the row's known value stand, and so does any
        // push warning less loudly than the value it would replace — a push
        // claiming "no" over a known "unknown" or "yes" changes nothing:
        // the downgrade direction is the one that removes a warning, and a
        // birth fact does not un-happen.
        unattended: ratchetUnattended(previous?.unattended, snapshot.unattended),
      };
      return previous
        ? { ...previous, ...carried }
        : {
            id: snapshot.id,
            workspaceId: snapshot.workspaceId,
            kind: snapshot.kind,
            ...carried,
          };
    });
    const visible = stripSessions(sessions);
    // The roster is authoritative for opened ids too: a session the daemon no
    // longer reports (deleted from the journal) must not keep a History tab.
    const rosterIds = new Set(sessions.map((session) => session.id));
    for (const id of openedIds) if (!rosterIds.has(id)) openedIds.delete(id);
    publish({
      ...state,
      sessions: visible,
      selectedSessionId: chooseSelected(visible, state.selectedSessionId),
      loading: false,
      error: null,
    });
  };

  const create = async (
    kind: SessionKind = "acp",
    provider: string | null = null,
    workspaceId: string | null = null,
  ): Promise<Session | null> => {
    if (state.creating) return null;
    ++refreshGeneration;
    publish({ ...state, creating: true, error: null });
    try {
      const session = await source.create(workspaceId, kind, provider);
      const listed = stripSessions([
        ...state.sessions.filter((current) => current.id !== session.id),
        session,
      ]);
      publish({
        ...state,
        sessions: listed,
        selectedSessionId: chooseSelected(listed, session.id),
        creating: false,
        error: null,
      });
      return session;
    } catch (cause) {
      // The daemon answered and rejected the start; surface its reason instead
      // of a generic claim about reachability.
      const message = rejectionMessage(cause);
      publish({
        ...state,
        creating: false,
        error: message.trim().length > 0 ? message : CREATE_FALLBACK_ERROR,
      });
      return null;
    }
  };

  const startWatch = (): void => {
    if (!source.watch || watchPromise !== null) return;
    watchPromise = source
      .watch(applySnapshot)
      .then((stop) => {
        watchStop = stop;
        if (watchLeases === 0) {
          stop();
          watchStop = null;
          watchPromise = null;
        }
        return stop;
      })
      .catch(() => {
        watchPromise = null;
        if (watchLeases > 0) {
          publish({ ...state, error: LIST_ERROR });
        }
        return () => undefined;
      });
  };

  const watch = (): (() => void) => {
    watchLeases += 1;
    let released = false;
    startWatch();
    return () => {
      if (released) return;
      released = true;
      watchLeases = Math.max(0, watchLeases - 1);
      if (watchLeases === 0 && watchStop !== null) {
        watchStop();
        watchStop = null;
        watchPromise = null;
      }
    };
  };

  /** Reloads after the daemon (re)connected and revives a watch that never came up.
   * Do NOT tear down a live watch here: the Rust bridge owns the roster
   * subscription and rebinds it across daemon recovery (RosterSubscription,
   * src-tauri/src/client/mod.rs), so a started watch survives a restart. Only
   * a failed initial watch needs retrying, and startWatch()'s catch resets
   * watchPromise for exactly that case. */
  const reconnect = async (): Promise<void> => {
    if (watchLeases > 0) startWatch();
    await refresh();
  };

  return {
    getState: () => state,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    refresh,
    create,
    watch,
    reconnect,
    select: (sessionId: string | null) => {
      // Null is the empty state: every tab closed, so nothing is active —
      // never an id that only points at a pending archive.
      if (sessionId === null) {
        publish({ ...state, selectedSessionId: null });
        return;
      }
      if (state.sessions.some((session) => session.id === sessionId)) {
        publish({ ...state, selectedSessionId: sessionId });
      }
    },
    open: (session) => {
      ++refreshGeneration;
      openedIds.add(session.id);
      publish({
        ...state,
        sessions: [...state.sessions.filter((current) => current.id !== session.id), session],
        selectedSessionId: session.id,
        error: null,
      });
    },
    dismissError: () => {
      if (state.error === null) return;
      publish({ ...state, error: null });
    },
  };
}

export function chatCapableProviders(providers: ProviderInfo[]): ProviderInfo[] {
  return providers.filter(
    (provider) =>
      provider.pickable !== false &&
      (provider.protocol === "acp" ||
        provider.protocol === "stream-json" ||
        provider.protocol === "pi-rpc" ||
        provider.protocol === "codex-app-server"),
  );
}

/** True when the provider spawns via npx and downloads third-party code on first run. */
export function requiresConsent(provider: ProviderInfo): boolean {
  return provider.origin === "npx-wrapper";
}

export function sessionCreateFromProvider(provider: ProviderInfo | undefined): {
  kind: SessionKind;
  provider: string | null;
} {
  if (provider === undefined) return { kind: "acp", provider: null };
  if (provider.protocol === "stream-json") return { kind: "claude", provider: null };
  if (provider.protocol === "pi-rpc") return { kind: "pi", provider: null };
  if (provider.protocol === "codex-app-server") return { kind: "codex", provider: null };
  if (provider.protocol === "acp") return { kind: "acp", provider: provider.id };
  return { kind: "acp", provider: null };
}

export function useWorkspaceSessions(workspaceId: string | null = null): WorkspaceSessionState & {
  refresh: () => Promise<void>;
  reconnect: () => Promise<void>;
  create: (
    kind?: SessionKind,
    provider?: string | null,
    workspaceId?: string | null,
  ) => Promise<Session | null>;
  select: (sessionId: string | null) => void;
  open: (session: Session) => void;
  dismissError: () => void;
} {
  const [controller] = useState<WorkspaceSessionController>(() =>
    createWorkspaceSessionController(),
  );
  const [state, setState] = useState(controller.getState);

  useEffect(() => {
    const unsubscribe = controller.subscribe(() => setState(controller.getState()));
    const releaseWatch = controller.watch();
    void controller.refresh();
    return () => {
      releaseWatch();
      unsubscribe();
    };
  }, [controller]);

  const refresh = useCallback(() => controller.refresh(), [controller]);
  const reconnect = useCallback(() => controller.reconnect(), [controller]);
  const create = useCallback(
    (kind?: SessionKind, provider?: string | null, requestedWorkspaceId?: string | null) =>
      controller.create(
        kind,
        provider,
        requestedWorkspaceId === undefined ? workspaceId : requestedWorkspaceId,
      ),
    [controller, workspaceId],
  );
  const select = useCallback(
    (sessionId: string | null) => controller.select(sessionId),
    [controller],
  );
  const open = useCallback((session: Session) => controller.open(session), [controller]);
  const dismissError = useCallback(() => controller.dismissError(), [controller]);

  return { ...state, refresh, reconnect, create, select, open, dismissError };
}
