// Why: the rename dialog — the current
// name pre-filled and selected, Enter saves through the exact
// `sessionSetName` call, Escape cancels, a daemon refusal mapped next to the
// field (its own words kept as the detail) with the draft kept, and focus
// back to whatever opened it. The new name itself is never written locally:
// the daemon pushes the
// roster after a landed rename and every title reads that. Registers with
// the shell so the crescent stays shut while the dialog is up.

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { ErrorText } from "../../../components/ErrorText";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { getFocusableElements } from "../../../lib/focusableElements";
import { isImeComposition } from "../../../lib/imeComposition";
import { useModalOpen } from "../../../lib/modalOpen";
import { sessionSetName } from "../../../lib/tauri";
import { validateSessionRename } from "../../../lib/sessionRename";
import type { SessionRenameTarget } from "./useSessionRename";
import "../Workspace.css";

interface SessionRenameDialogProps {
  rename: SessionRenameTarget | null;
  onClose: () => void;
}

export function SessionRenameDialog({ rename, onClose }: SessionRenameDialogProps) {
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  // The trigger the dialog took focus from — the kebab or the tab. Given
  // back on close.
  const triggerRef = useRef<HTMLElement | null>(null);
  // The synchronous half of `submitting`: Enter on the field and the form's
  // own implicit submission both reach the handler in one press, and the
  // state half would not have landed by the second call.
  const submittingRef = useRef(false);

  // The reset runs during render, not in an effect: the input's first paint
  // after an open must already carry the title, or the focus effect selects
  // an empty field and the re-render leaves the cursor at the end. A title
  // change resyncs only an untouched draft — the user's edit
  // outranks the roster behind it: overwriting a draft the user has typed
  // into would throw that edit away.
  //
  // The submitting halves are deliberately NOT cleared here. The daemon's
  // rename pushes the roster and answers the RPC from independent tasks with
  // no ordering, so a title resync can land while a save is on the wire — and
  // clearing the lock in that window would re-open every exit mid-save. The
  // save's `finally` owns the settled case, and the lock blocks every exit
  // while a save is in flight, so a stale lock at open is not reachable.
  //
  // The error is the other half of the same conditional, and for the opposite
  // reason: a refusal must die with the session it was about (the dialog
  // stays mounted between opens, so a stale sentence would greet the next
  // session's valid name) — and a mid-dialog push must not eat a daemon
  // refusal the mirror cannot predict. Open and close clear it; a title-only
  // resync never does.
  const [prevRename, setPrevRename] = useState(rename);
  if (rename !== prevRename) {
    setPrevRename(rename);
    if (rename === null || prevRename === null || draft === prevRename.title) {
      setDraft(rename?.title ?? "");
    }
    if (rename === null || prevRename === null) {
      setError(null);
    }
  }

  useModalOpen(rename !== null);

  // The trigger the dialog takes focus from — the kebab or the tab, focused
  // by the caller before the dialog opens. Captured in a layout effect so
  // it reads the DOM after the reset's commit, and given back on close.
  // Only an opening captures: a re-entrant openRename while the dialog is
  // up would overwrite the trigger with the dialog's own input, and on
  // close the unmounted input refuses the restore.
  useLayoutEffect(() => {
    if (rename === null || triggerRef.current !== null) return;
    triggerRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }, [rename]);

  // Focus and select on open only: a title resync mid-dialog must not yank
  // the user's selection out from under them. The ref starts null so the
  // first effect run reads "was closed" and focuses.
  const prevRenameRef = useRef<SessionRenameTarget | null>(null);
  useEffect(() => {
    const previous = prevRenameRef.current;
    prevRenameRef.current = rename;
    if (rename === null || previous !== null) return;
    const input = inputRef.current;
    if (input === null) return;
    input.focus();
    input.setSelectionRange(0, input.value.length);
  }, [rename]);

  useEffect(() => {
    if (rename !== null) return;
    const trigger = triggerRef.current;
    triggerRef.current = null;
    if (trigger === null || !trigger.isConnected) return;
    trigger.focus({ preventScroll: true });
  }, [rename]);

  const handleSubmit = async () => {
    if (rename === null || submittingRef.current) return;
    // An unchanged name is a no-op on the wire — the daemon's
    // store computes `changed` and skips the roster push, so the cost of
    // skipping this is one IPC that changes nothing.
    if (draft === rename.title) return;
    // The client mirror of the daemon's own rule: a name it would refuse is
    // refused here with the daemon's sentence, before any call.
    const refused = validateSessionRename(draft);
    if (refused !== null) {
      setError({ sentence: refused, detail: null });
      return;
    }
    submittingRef.current = true;
    setSubmitting(true);
    setError(null);
    try {
      // The daemon trims before storing; the trimmed value is sent so the
      // call carries what will land.
      await sessionSetName(rename.sessionId, draft.trim());
      onClose();
    } catch (cause: unknown) {
      // The mapper's sentence for a refusal the mirror could not predict,
      // with the daemon's own words kept as its detail; the draft stays put.
      setError(errorSentence(cause));
    } finally {
      submittingRef.current = false;
      setSubmitting(false);
    }
  };

  if (rename === null) return null;

  // The refusal shows as soon as the draft is invalid — including on open.
  // The daemon's own title derivation keeps U+200C/U+200D, so a pre-fill can
  // be a name the rename door refuses; a grey button and no message is the
  // one outcome this dialog must never produce.
  const refusal = validateSessionRename(draft);
  const shownError = error !== null ? error.sentence : refusal;
  const shownDetail = error !== null ? error.detail : null;
  const saveDisabled = submitting || draft === rename.title || refusal !== null;

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      if (isImeComposition(event.nativeEvent)) return;
      event.preventDefault();
      if (!submitting) onClose();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = getFocusableElements(event.currentTarget);
    if (focusable.length === 0) {
      // Mid-save every focusable is disabled, so the query comes back empty:
      // hold the keyboard rather than letting Tab walk out from under a write
      // in flight (and take Escape with it — the handler is on this card).
      event.preventDefault();
      event.currentTarget.focus();
      return;
    }
    event.preventDefault();
    const first = focusable[0]!;
    const last = focusable[focusable.length - 1]!;
    if (!event.currentTarget.contains(document.activeElement)) {
      first.focus();
    } else if (event.shiftKey && document.activeElement === first) {
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      first.focus();
    }
  };

  return (
    <div
      className="workspace-rename-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !submitting) onClose();
      }}
    >
      <div
        className="workspace-rename-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="workspace-rename-dialog-title"
        // The trap's fallback target when every focusable is disabled mid-save:
        // without a tabindex the card cannot take focus.
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="workspace-rename-dialog-header">
          <h2 id="workspace-rename-dialog-title">Rename agent</h2>
          <button
            type="button"
            className="workspace-dialog-close"
            onClick={onClose}
            aria-label="Close rename dialog"
            disabled={submitting}
          >
            ×
          </button>
        </div>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void handleSubmit();
          }}
        >
          <label className="workspace-rename-input-label" htmlFor="workspace-rename-input">
            Name
          </label>
          <input
            ref={inputRef}
            id="workspace-rename-input"
            value={draft}
            onChange={(event) => {
              setDraft(event.target.value);
              setError(null);
            }}
            onKeyDown={(event) => {
              if (isImeComposition(event.nativeEvent)) return;
              if (event.key !== "Enter") return;
              // The form's own implicit submission would also fire on a real
              // browser; the submitting ref makes the second call a no-op.
              void handleSubmit();
            }}
            aria-invalid={shownError !== null}
            aria-describedby={shownError !== null ? "workspace-rename-error" : undefined}
            disabled={submitting}
          />
          {shownError !== null ? (
            <div id="workspace-rename-error" className="workspace-rename-error" role="alert">
              <ErrorText
                sentence={shownError}
                detail={shownDetail}
                id="workspace-rename-error-text"
              />
            </div>
          ) : null}
          <div className="workspace-rename-actions">
            <button
              type="button"
              className="workspace-secondary-action"
              onClick={onClose}
              disabled={submitting}
            >
              Cancel
            </button>
            <button type="submit" className="workspace-primary-action" disabled={saveDisabled}>
              {submitting ? "Saving…" : "Rename"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
