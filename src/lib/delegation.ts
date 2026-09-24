import { useCallback, useMemo, useSyncExternalStore } from "react";
import { delegationGet, delegationSet } from "./tauri";
import { errorSentence } from "./errorSentence";
import type { DelegationReply } from "../types/ipc";

/**
 * The handshake capability that gates every delegation RPC, spelled exactly
 * like the daemon's own name for it (`SPEC-slice-5b-delegation.md` §3, Pass
 * B). A daemon that does not advertise it cannot answer `delegation_get`, so
 * nothing may ask: the switch section is not drawn and no request is sent —
 * the section is absent, not broken, and the absence has a name.
 */
export const DELEGATION_CAPABILITY = "permission_delegation";

/** The two RPCs the controller speaks, injectable so tests can answer them. */
export interface DelegationSource {
  get: () => Promise<DelegationReply>;
  set: (enabled: boolean) => Promise<void>;
}

const DEFAULT_SOURCE: DelegationSource = { get: delegationGet, set: delegationSet };

/**
 * The one shape a reply must have before any of it is believed. The module
 * reaches the wire through a boundary cast (`tauri.ts`), so the compiler
 * cannot see a reply that omits `enabled` or `source` — and an absent
 * `enabled` read as a real answer would authorise a write from a guess, the
 * one thing this store exists to prevent. A reply failing this check is a
 * failed load, not a partial adoption.
 */
function replyIsComplete(reply: unknown): reply is DelegationReply {
  return (
    typeof reply === "object" &&
    reply !== null &&
    typeof (reply as DelegationReply).enabled === "boolean" &&
    typeof (reply as DelegationReply).source === "string"
  );
}

export interface DelegationState {
  /**
   * The store's last answer, with its `source`. Null while no answer has
   * landed: nothing on screen may be edited from a guess.
   */
  reply: DelegationReply | null;
  /**
   * What the UI shows the switch as. Null while no answer has landed: the
   * first fetch is in flight, or a rejected write's re-read also failed and
   * the store holds nothing it may show as definite. In both states nothing
   * on screen may be edited into existence from a guess — and the unknown is
   * rendered as unknown, never as a definite off.
   */
  enabled: boolean | null;
  /**
   * A failed first load is terminal, not a loading state: nothing will ever
   * arrive on its own, so the panel shows the daemon's sentence and a Retry.
   */
  loadFailed: boolean;
  /** The daemon's own sentence for the last refused write, verbatim. */
  error: { sentence: string; detail: string | null } | null;
}

export interface DelegationController {
  getState: () => DelegationState;
  subscribe: (listener: () => void) => () => void;
  /** Asks the daemon for the stored answer. Capability gating is the caller's. */
  load: () => Promise<void>;
  /**
   * The one write path. Both entry points — the Agents panel's switch and a
   * roster row's take-back — call this and nothing else: the write machinery
   * (optimistic swap, sequence guard, revert) exists here once. Resolves
   * true when the daemon accepted the write — including one a newer write
   * superseded, which the daemon still holds — and false when it refused it.
   */
  setEnabled: (next: boolean) => Promise<boolean>;
}

/**
 * The delegation switch's controller: an external store, because two surfaces
 * that never mount together (the Settings tab and the workspace tab strip)
 * must read and write the same setting — a take-back that flipped the switch
 * off while the panel still showed on would be two answers to one question.
 *
 * The write discipline is `ProviderToolSettings`' (the app audit's findings
 * 1/6/8, re-earned on exactly this shape of toggle), adapted to one boolean:
 *
 * - `enabledRef` — never the render closure — is what a second rapid write
 *   reads. `confirmedRef` — the last value the daemon accepted — is what a
 *   rejection reverts onto, so a refused write can never leave the panel
 *   showing a value the daemon refused to take.
 * - A monotonic sequence decides which write owns the UI when it settles: a
 *   rejection a newer write superseded reverts nothing and reports nothing.
 * - A fetch adopts its reply only when NO write overlapped it — none in
 *   flight when the fetch started (`writesInFlightRef`) and none started
 *   while it flew (`seqRef` unchanged). The guard answers "did a write
 *   overlap this fetch?", not just "is there a newer write?".
 *
 * The value carries no minted identity, so writes overlap freely — the
 * tool-toggle shape, not the whole-document one.
 */
export function createDelegationController(
  source: DelegationSource = DEFAULT_SOURCE,
): DelegationController {
  let state: DelegationState = {
    reply: null,
    enabled: null,
    loadFailed: false,
    error: null,
  };
  const listeners = new Set<() => void>();
  // Synchronous mirror of `state.enabled`. It — never the render closure — is
  // what a second rapid write reads.
  let enabledRef: boolean | null = null;
  // The last value the daemon actually accepted: adopted from a completed
  // fetch, or stamped by a write that confirmed. A rejection reverts onto
  // THIS — never onto the optimistic value an earlier in-flight write left in
  // `enabledRef`, which the daemon may never have accepted at all. Reverting
  // onto an unconfirmed value is how the panel ends up showing a state the
  // daemon does not hold, with no error saying so.
  let confirmedRef: boolean | null = null;
  // Monotonic write sequence: only the newest write owns the UI when it
  // settles.
  let seq = 0;
  // How many writes are currently between "sent" and "settled"; the load
  // effect reads it to tell "a write was in flight when this fetch started"
  // apart from "a write has settled at some point".
  let writesInFlight = 0;

  const publish = (next: DelegationState): void => {
    state = next;
    for (const listener of [...listeners]) listener();
  };

  /**
   * The read path. `invalidateOnFail` marks the read sent to settle the doubt
   * a rejected write created (see the refusal arm below): when THAT read also
   * fails, the store holds no answer it may show as definite, so the panel
   * goes to the named unknown instead of keeping a value the daemon may have
   * replaced. Every other reader — mount, reconnect, Retry — keeps its
   * already-shown value on failure, because nothing happened in between that
   * could have invalidated it.
   */
  const load = async (invalidateOnFail = false): Promise<void> => {
    const seqAtFetch = seq;
    const writeWasInFlight = writesInFlight > 0;
    try {
      const reply = await source.get();
      if (!replyIsComplete(reply)) {
        // The daemon answered without a usable answer. Treat it as the
        // failure it is — never as an answer missing its fields, which would
        // render a definite switch and a blank source sentence off silence.
        throw new Error(
          "The daemon's delegation answer was incomplete (enabled or source missing) — nothing was adopted.",
        );
      }
      // A write issued while this fetch was in flight is newer: keep it. A
      // write that was ALREADY in flight when the fetch started raced it:
      // whether the reply predates or postdates that write is unknowable, so
      // the reply adopts nothing — the write's own settle is the state of
      // record.
      if (seq !== seqAtFetch || writeWasInFlight) return;
      enabledRef = reply.enabled;
      confirmedRef = reply.enabled;
      publish({ reply, enabled: reply.enabled, loadFailed: false, error: null });
    } catch (cause: unknown) {
      if (enabledRef !== null && !invalidateOnFail) {
        // A later answer exists and nothing has shaken trust in it; the panel
        // already shows a value, so the failed refresh is reported, not
        // terminal.
        publish({ ...state, error: errorSentence(cause) });
        return;
      }
      // No answer the panel may show as definite survives a failed read here:
      // either none ever landed, or the one on screen lost the store's trust
      // to a rejected write. The named unknown is the honest state — the
      // switch renders it (never a definite off), and Retry re-asks.
      enabledRef = null;
      confirmedRef = null;
      publish({
        reply: null,
        enabled: null,
        loadFailed: true,
        error: errorSentence(cause),
      });
    }
  };

  const setEnabled = async (next: boolean): Promise<boolean> => {
    // The two directions of the switch are not symmetric, and the asymmetry is
    // the honesty:
    //
    // GRANTING authority (`true`) needs a stored answer first — until the
    // store has answered once there is nothing to write from and nothing to
    // revert onto, and the one thing this store exists to prevent is a write
    // authorised by a guess.
    //
    // TAKING authority back (`false`) does not. `false` can only reduce what
    // the daemon exercises, and the human reaching for it may be acting
    // precisely BECAUSE the answer cannot be read (a failed load, a rejected
    // write whose re-read failed) — the unknown state must not withdraw the
    // control that stops delegation. It is skipped only when the store
    // positively holds `false`, where the click could not act.
    if (next ? enabledRef !== false : enabledRef === false) return false;
    const thisSeq = ++seq;
    writesInFlight += 1;
    enabledRef = next;
    publish({ ...state, enabled: next, error: null });
    let accepted = false;
    let refusedNewest = false;
    try {
      await source.set(next);
      accepted = true;
      return true;
    } catch (cause) {
      // A newer write superseded this one: its optimistic value stands, this
      // rejection reports nothing.
      if (thisSeq !== seq) return false;
      // The rejection is a fact about the transport, not the store — the
      // daemon may have applied the write and lost the reply. The panel
      // falls back to the last CONFIRMED value — not onto `enabledRef` as it
      // stood when this write started, which an earlier still-in-flight write
      // may have optimistically moved to a value the daemon never accepted
      // (two refused rapid writes would otherwise leave the switch showing ON
      // over a daemon holding OFF) — with the refusal's sentence beside it,
      // and the re-read below goes and finds out what the daemon actually
      // holds.
      enabledRef = confirmedRef;
      publish({ ...state, enabled: confirmedRef, error: errorSentence(cause) });
      refusedNewest = true;
      return false;
    } finally {
      writesInFlight -= 1;
      if (accepted) {
        // Two decisions the old code folded into one boolean, now separate:
        //
        // WHAT THE DAEMON HOLDS — an acceptance is a fact about the daemon
        // and is stamped whenever it happens, even though a newer write owns
        // the UI. Skipping the stamp when superseded is what let a later
        // refusal revert onto a value the daemon no longer holds: an
        // accepted ON forgotten, then a refused OFF "restoring" the panel to
        // OFF over a daemon holding ON — the switch reading "off" while
        // agents answer their children's permission cards.
        //
        // WHO OWNS THE UI — only the newest write publishes the full panel
        // state (reply, error cleared). A superseded acceptance still moves
        // the displayed value onto the daemon's fact — a consent surface
        // never shows LESS authority than is live — but leaves any standing
        // refusal sentence alone: the newest write's settlement is free to
        // overwrite it, accepted or reverted onto the stamp above.
        //
        // The minted `source` carries an obligation on the daemon contract
        // (audit 3, F11): a successful `delegation_set` must leave
        // `delegation_get`'s `source` at `file`, or the app must stop minting
        // it — the app cannot check the provenance it asserts here, so the
        // sentence is only as true as the daemon makes it.
        const reply: DelegationReply = { enabled: next, source: "file" };
        confirmedRef = next;
        enabledRef = next;
        if (thisSeq === seq) {
          publish({ reply, enabled: next, loadFailed: false, error: null });
        } else {
          publish({ ...state, reply, enabled: next, loadFailed: false });
        }
      } else if (refusedNewest) {
        // The rejection did not say what the daemon holds, so ask. This runs
        // after the in-flight counter dropped, so the re-read is an ordinary
        // fetch: it obeys the same ownership guard as every read — a write
        // issued while it flies discards its reply — and on success the
        // daemon's answer replaces both the fallback value and the refusal
        // sentence. On failure the load's invalidated path takes the panel to
        // the named unknown: after a rejected write, `confirmedRef` is a
        // belief about a store the app just failed to reach twice, not a
        // fact, and a definite render of it is the false claim this file
        // exists to prevent.
        void load(true);
      }
    }
  };

  return {
    getState: () => state,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    load,
    setEnabled,
  };
}

/**
 * The app's one controller instance: the switch in Settings → Agents and the
 * take-back on a roster row are two entry points to the same setting, and a
 * value one flipped must be what the other reads.
 */
export const delegationController = createDelegationController();

/** Reads one controller as React state, with the load retry folded in. The
 * returned object is stable between state changes — a fresh identity every
 * render would break any memoized consumer the moment one exists. */
export function useDelegationState(
  controller: DelegationController,
): DelegationState & { retryLoad: () => void } {
  const state = useSyncExternalStore(controller.subscribe, controller.getState);
  const retryLoad = useCallback(() => {
    void controller.load();
  }, [controller]);
  return useMemo(() => ({ ...state, retryLoad }), [state, retryLoad]);
}
