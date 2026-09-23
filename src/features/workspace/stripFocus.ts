// Why this file exists: the tab strip's "+" owns two halves of one focus
// problem — the request its Terminal entry makes for the tab it just created,
// and the rule that takes focus back when a "+" flow ends without an outcome.
// Both live here so Workspace.tsx only wires them and the rule is stated once.

import { useCallback, useEffect, useRef, useState, type RefObject } from "react";

/**
 * Is focus still where a strip flow left it — lost (body or null), or still
 * sitting on "+"? The focus rule asks this before restoring, and the
 * terminal autofocus guard asks it before taking: one question, one answer.
 */
export function focusIsWhereTheFlowLeftIt(
  active: Element | null,
  addButton: HTMLElement | null,
): boolean {
  return active === null || active === document.body || active === addButton;
}

interface StripFocusArgs {
  addButtonRef: RefObject<HTMLButtonElement | null>;
  /** "+" is disabled while a create or a provider choice is in flight. */
  addDisabled: boolean;
  /** The screen's errors — snapshotted at flow start, so only an error that APPEARED brands this flow failed. */
  sessionsError: string | null;
  providerError: string | null;
  /** A provider picker is open: the choice can still be made, so nothing has ended. */
  pickerOpen: boolean;
  /** The strip's current tab: the terminal request dies when it moves away. */
  selectedSessionId: string | null;
  /** The strip's current workspace: the request dies when the user leaves the one the create ran under. */
  workspaceId: string | null;
}

/**
 * One focus rule for the strip's "+" — it fires when a flow that disabled
 * "+" ends WITHOUT the outcome the user keeps: a flow whose own error appears
 * with its settle (a refused terminal create, a failed provider lookup), or a
 * provider picker dismissed with Escape / an outside click (armed through
 * `noteChoiceDismissed`). The failure is judged as THIS flow's outcome — an
 * error that was already on screen when the flow started (a stale
 * providerError from an earlier Agent flow) must never brand a successful
 * Terminal create a failure. Focus goes back to "+" only when
 * `focusIsWhereTheFlowLeftIt` says it was lost or is still on "+". A
 * successful create re-enables "+" under the same conditions and deliberately
 * keeps focus where it is: the new tab is the outcome, not the button. A
 * dismissal arms in its own event and is consumed by the next settle. A
 * cancelled consent card is not this rule's business: it restores through
 * consentRestoreRef in Workspace.tsx.
 *
 * The same hook carries the Terminal entry's focus request: `armTerminalFocus`
 * names the session — and workspace — the menu just created, the request
 * dies when the strip selects another tab or leaves that workspace (during
 * the create or while the view is still opening), and `terminalAutoFocus` is
 * what TerminalSurface acts on while the request stands.
 */
export function useStripFocus({
  addButtonRef,
  addDisabled,
  sessionsError,
  providerError,
  pickerOpen,
  selectedSessionId,
  workspaceId,
}: StripFocusArgs): {
  /** Arm: the picker was dismissed without a choice (Escape / outside click). */
  noteChoiceDismissed: () => void;
  /** Pass to TerminalSurface: the + menu's Terminal entry created this tab and its request stands. */
  terminalAutoFocus: boolean;
  /** The + menu's Terminal entry reports the session it created, under the workspace the create ran in. */
  armTerminalFocus: (sessionId: string, createdInWorkspaceId: string | null) => void;
  /** The surface spent the request (focused, or declined because focus had moved). */
  takeTerminalFocus: () => void;
} {
  const wasDisabled = useRef(false);
  const choiceDismissedRef = useRef(false);
  const startedSessionsError = useRef<string | null>(null);
  const startedProviderError = useRef<string | null>(null);
  const [request, setRequest] = useState<{ sessionId: string; workspaceId: string | null } | null>(
    null,
  );

  // The request dies when the strip leaves the tab or the workspace it was
  // made for: the create's own tab in its own workspace takes focus when its
  // terminal opens — a later click on that tab, or the same session reached
  // from another workspace, never does. State adjusted during render (React's
  // documented derive-from-props form): an effect would have to setState, and
  // every selection and workspace path reaches here.
  if (
    request !== null &&
    (request.sessionId !== selectedSessionId || request.workspaceId !== workspaceId)
  ) {
    setRequest(null);
  }

  const noteChoiceDismissed = useCallback(() => {
    choiceDismissedRef.current = true;
  }, []);
  const armTerminalFocus = useCallback(
    (sessionId: string, createdInWorkspaceId: string | null) =>
      setRequest({ sessionId, workspaceId: createdInWorkspaceId }),
    [],
  );
  const takeTerminalFocus = useCallback(() => setRequest(null), []);

  useEffect(() => {
    if (addDisabled) {
      wasDisabled.current = true;
      // A new flow owns the outcome now; a dismissal it never saw is stale,
      // and the errors on screen at its start are the baseline it is judged
      // against — only an error that APPEARED since then is its failure.
      choiceDismissedRef.current = false;
      startedSessionsError.current = sessionsError;
      startedProviderError.current = providerError;
      return;
    }
    const dismissed = choiceDismissedRef.current;
    choiceDismissedRef.current = false;
    const failed =
      (sessionsError !== null && sessionsError !== startedSessionsError.current) ||
      (providerError !== null && providerError !== startedProviderError.current);
    const outcomeLess = (wasDisabled.current && failed) || dismissed;
    wasDisabled.current = false;
    if (!outcomeLess) return;
    if (focusIsWhereTheFlowLeftIt(document.activeElement, addButtonRef.current)) {
      addButtonRef.current?.focus();
    }
    // `pickerOpen` is a dependency, not a read: a dismissal that finds "+"
    // already re-enabled (consent card cancelled back into the option list,
    // then the picker dismissed) changes only the picker, and the rule must
    // still run to consume the armed flag.
  }, [addDisabled, sessionsError, providerError, pickerOpen, addButtonRef]);

  return {
    noteChoiceDismissed,
    terminalAutoFocus: request !== null && request.sessionId === selectedSessionId,
    armTerminalFocus,
    takeTerminalFocus,
  };
}
