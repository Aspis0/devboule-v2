import { useCallback, useEffect, useRef, type RefObject } from "react";
import { useAppStore } from "../../store/appStore";

/**
 * Where focus lands across an ask and its rows. The dialog's own return
 * misses the confirm path — the row trigger stands `disabled` while the
 * act runs, and focusing a disabled button is a no-op — so the hook parks
 * focus on the panel root once the ask closes onto the wire and repairs
 * toward the borrowed trigger once the wire answers. A declined ask ends
 * on the trigger through the repair; a focus the person moved mid-act is
 * theirs and ends the ask; dead space inside the panel leaves the root
 * holding focus, while outside it the body is nobody's and the row
 * reclaims it. Exactly one ask arms the sequence, and the arm dies
 * with the act's own re-read.
 *
 * Beside the arm, the rescue answers rows changes — the act's
 * re-read, a poll, a refresh — asked or not: the last control inside the
 * panel that held focus, once a new tree removes it, drops focus to the
 * body, and the panel root reclaims it. Only that shape moves: a focus
 * the person holds elsewhere, or a modal holding the keyboard, moves
 * nothing. A removal under a modal heals on the next poll, never on the
 * modal's close.
 */
export function useAskFocus(
  rowsKey: unknown,
  acting: boolean,
): {
  armAsk: () => void;
  menuAnchorRef: RefObject<HTMLElement | null>;
  panelRef: RefObject<HTMLDivElement | null>;
} {
  const armedRef = useRef(false);
  const menuAnchorRef = useRef<HTMLElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  // The anchor the ask borrowed, stashed at park time: the live ref is
  // cleared then, so the landing below can only ever fire behind an ask.
  const returnRef = useRef<HTMLElement | null>(null);
  // The tree the ask opened over: the settle below answers only to a newer
  // one — the acting flip lands before the act's own re-read returns, and
  // settling on the stale rows would spend the arm while the row is still
  // on screen.
  const armedRowsRef = useRef<{ key: unknown } | null>(null);
  // The last control inside the panel that held focus: a removed row
  // drops focus to the body silently, so only a stash says whose removal
  // stranded it when the next tree lands.
  const lastInsideRef = useRef<Element | null>(null);
  const openModals = useAppStore((state) => state.modalOpenTokens.size);

  // The key the next arm snapshots, held in a ref so arming stays a
  // stable callback: the memoised rows hang their discard/delete prop off
  // it, and a fresh closure per read would re-render every row per poll.
  // Arms fire from press handlers only, after this effect, so the ref is
  // current at every arm.
  const rowsRef = useRef(rowsKey);
  useEffect(() => {
    rowsRef.current = rowsKey;
  }, [rowsKey]);

  const armAsk = useCallback((): void => {
    armedRef.current = true;
    armedRowsRef.current = { key: rowsRef.current };
  }, []);

  // The ask closed and the wire owns the trigger now: park on the panel
  // root, never on the body — but only once no modal holds the keyboard,
  // and only if focus is still adrift on the body. The host answers the ask
  // after the dialog unregisters, so this count never holds the ask itself.
  useEffect(() => {
    if (!armedRef.current || !acting || openModals !== 0) return;
    if (document.activeElement !== document.body) return;
    returnRef.current = menuAnchorRef.current;
    menuAnchorRef.current = null;
    panelRef.current?.focus({ preventScroll: true });
  }, [acting, openModals, menuAnchorRef, panelRef]);

  // The wire answered: repair toward the borrowed trigger when focus is
  // still parked or adrift — the re-read has not landed yet, and the
  // rescue below answers for it when it takes the row. A focus anywhere
  // else is the person's own — a control they tabbed or clicked onto —
  // and ends the ask here, anchor and all.
  useEffect(() => {
    if (!armedRef.current || acting) return;
    if (document.activeElement !== panelRef.current && document.activeElement !== document.body)
      return consume();
    returnRef.current = returnRef.current ?? menuAnchorRef.current;
    menuAnchorRef.current = null;
    const anchor = returnRef.current;
    if (anchor?.isConnected === true) anchor.focus({ preventScroll: true });

    function consume(): void {
      armedRef.current = false;
      armedRowsRef.current = null;
      returnRef.current = null;
      menuAnchorRef.current = null;
    }
  }, [acting, menuAnchorRef, panelRef]);

  // The re-read landed: the act is over either way, so the arm dies
  // here. Settles only against a newer tree than the ask opened over —
  // the flip above runs first on the stale rows, and settling there
  // would spend the arm while the row is still on screen. The landing
  // itself is the rescue's below: a row the re-read takes moves focus
  // through it, so this moves nothing. A re-read with no arm never
  // spends anything, and a mid-wire poll keeps the arm (`acting` guard).
  useEffect(() => {
    if (!armedRef.current || acting) return;
    const armedRows = armedRowsRef.current;
    if (armedRows === null || rowsKey === armedRows.key) return;
    armedRef.current = false;
    armedRowsRef.current = null;
    returnRef.current = null;
    menuAnchorRef.current = null;
  }, [rowsKey, acting]);

  // The stash behind the rescue: which control inside the panel held
  // focus last. Only a focusin elsewhere with a real target clears it —
  // a focusout onto the body or nowhere keeps it, because a removal reads
  // either way depending on the engine and neither may spend the stash.
  useEffect(() => {
    function onFocusIn(event: FocusEvent): void {
      const target = event.target as Element | null;
      lastInsideRef.current =
        target !== null && panelRef.current?.contains(target) === true ? target : null;
    }
    function onFocusOut(event: FocusEvent): void {
      const next = event.relatedTarget as Element | null;
      if (next === null || next === document.body) return;
      if (panelRef.current?.contains(next) !== true) lastInsideRef.current = null;
    }
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
    };
  }, [panelRef]);

  // A row that takes focus with it: the stashed control is gone and
  // focus fell back to the body, so the panel root reclaims it — on a rows
  // change, asked or not. Never while a modal holds the keyboard, and
  // never away from a focus the person still holds. Answers the rows key
  // only: a removal under a modal waits for the next poll, which is never
  // more than one poll away.
  useEffect(() => {
    if (useAppStore.getState().modalOpenTokens.size !== 0) return;
    const gone = lastInsideRef.current;
    if (gone !== null && gone.isConnected !== true && document.activeElement === document.body)
      panelRef.current?.focus({ preventScroll: true });
  }, [rowsKey, panelRef]);

  return { armAsk, menuAnchorRef, panelRef };
}
