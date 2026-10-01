import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";
import type { AgentChatItem } from "../../../lib/agentSession";
import { currentTurnIndex } from "./currentTurn";
import { sameUserItems, userTurns, type UserTurn } from "./turnGrouping";
import { TurnStop } from "./TurnStop";
import { dayKey } from "../../../lib/dayClock";
import { useTurnRailIntent } from "./useTurnRailIntent";
import "./TurnRail.css";

const GUTTER_CLASS = "has-turn-rail";

/** The card spans 9…233 px of the content box (224 px wide, starting 3 px
 * short of the dot box's right edge); the column begins at the gutter's
 * 32 px, so the turn's bubble must leave 204 px of column beside the card —
 * 3 px of gap included — or the card covers the turn it previews. */
const PREVIEW_MIN_CANVAS_PX = 204;

/** The gutter reserved in the content box (TurnRail.css: `has-turn-rail …
 * content { padding-left: 32px }`): `clientWidth` includes it, the bubbles
 * live inside it, so the preview's canvas is the column behind it. */
const GUTTER_COLUMN_PX = 32;

interface TurnRailProps {
  scrollRef: RefObject<HTMLDivElement | null>;
  contentRef: RefObject<HTMLDivElement | null>;
  items: readonly AgentChatItem[];
}

interface TurnMeasure {
  /** The bubble's centre in the content box — where the dot parks. */
  center: number;
  /** The bubble's own width — how far the turn's row reaches left. */
  bubbleWidth: number;
}

interface RailMeasures {
  contentWidth: number;
  byId: Record<string, TurnMeasure>;
}

/** One turn's elements, resolved once per turn list instead of per pass. */
interface TurnFrame {
  anchor: HTMLElement;
  bubble: HTMLElement | null;
}

interface RailFrames {
  turns: readonly UserTurn[];
  byId: Map<string, TurnFrame>;
}

/** A scrollport with a pixel of travel or less is not a scrolling
 * transcript: layout residue must not hold up the gutter. */
function overflows(conversation: HTMLDivElement): boolean {
  return conversation.scrollHeight - conversation.clientHeight > 1;
}

/** The frames in turn order, or null when a turn's element is missing:
 * the current-turn rule and the gutter's hold then stand down, while the
 * measure pass works from the frames that exist and the render skips a dot
 * with no measure — a half-rendered transcript gets no wrong
 * measurements, only absent ones. */
function orderedFrames(frames: RailFrames, turns: readonly UserTurn[]): TurnFrame[] | null {
  const ordered: TurnFrame[] = [];
  for (const turn of turns) {
    const frame = frames.byId.get(turn.id);
    if (frame === undefined) return null;
    ordered.push(frame);
  }
  return ordered;
}

/** Whether the cache still serves this turn list: same list, every turn's
 * frame present, every anchor connected. `isConnected` reads no layout, so
 * a pass validates before it measures — a detached or missing row rebuilds
 * the cache from the document instead of measuring it: a detached element
 * reads 0, and stable zeros would never schedule a re-measure. */
function framesValid(frames: RailFrames, turns: readonly UserTurn[]): boolean {
  if (frames.turns !== turns) return false;
  for (const turn of turns) {
    const frame = frames.byId.get(turn.id);
    if (frame === undefined || !frame.anchor.isConnected) return false;
  }
  return true;
}

function sameMeasures(previous: RailMeasures | null, next: RailMeasures): boolean {
  if (previous === null || previous.contentWidth !== next.contentWidth) return false;
  const previousIds = Object.keys(previous.byId);
  if (previousIds.length !== Object.keys(next.byId).length) return false;
  return previousIds.every((id) => {
    const before = previous.byId[id];
    const after = next.byId[id];
    return (
      before !== undefined &&
      after !== undefined &&
      before.center === after.center &&
      before.bubbleWidth === after.bubbleWidth
    );
  });
}

/** Streaming rewrites the assistant's text and the tool rows, never the user
 * items a turn is keyed by, so a token-level update compares equal and the
 * rail skips its render — and the measuring effect with it — entirely. */
function sameTurnInputs(previous: TurnRailProps, next: TurnRailProps): boolean {
  if (previous.scrollRef !== next.scrollRef || previous.contentRef !== next.contentRef) {
    return false;
  }
  return sameUserItems(previous.items, next.items);
}

function TurnRailInner({ scrollRef, contentRef, items }: TurnRailProps) {
  const turns = useMemo(() => userTurns(items), [items]);
  const [overflowing, setOverflowing] = useState(false);
  // A geometry signal with no payload: the render it causes is what
  // re-measures the dots.
  const [geometryTick, setGeometryTick] = useState(0);
  const [measures, setMeasures] = useState<RailMeasures | null>(null);
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [pinnedId, setPinnedId] = useState<string | null>(null);
  const [openPreviewId, setOpenPreviewId] = useState<string | null>(null);
  // The stop follows the last-focused dot; the current turn takes it before
  // focus ever moves there. Mirrors the strip's roving stop, not its selection.
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const [followedActive, setFollowedActive] = useState<string | null>(null);
  const framesRef = useRef<RailFrames | null>(null);
  const pinnedTurnRef = useRef<UserTurn | null>(null);
  const openTurnRef = useRef<UserTurn | null>(null);
  // Set by a pointer press so the focus that press causes does not open
  // the preview; the document-level pointerup/pointercancel listeners end
  // the press, so a swallowed keyboard focus cannot outlive it.
  const pointerFocusRef = useRef(false);
  const shown = overflowing && turns.length > 0;
  // One day decision per render, handed to every dot: a per-stop clock
  // lets memoised dots straddle midnight showing two formats. No timer runs.
  // oxlint-disable-next-line react/purity -- the next rail render re-decides the day.
  const today = dayKey(Date.now());

  const updateCurrent = useCallback(() => {
    const conversation = scrollRef.current;
    const frames = framesRef.current;
    if (conversation === null || frames === null || frames.turns !== turns) return;
    const ordered = orderedFrames(frames, turns);
    if (ordered === null) return;
    const rect = conversation.getBoundingClientRect();
    const index = currentTurnIndex(
      (i) => ordered[i].anchor.getBoundingClientRect().top,
      ordered.length,
      rect.top,
    );
    // A jump does not ride on this rule: jumpTo pins the clicked turn and
    // the reader's scroll intent releases it.
    const nextId = (index >= 0 ? turns[index] : turns[0]).id;
    setCurrentId((previous) => (previous === nextId ? previous : nextId));
  }, [scrollRef, turns]);

  const jumpTo = useCallback((turn: UserTurn) => {
    const frame = framesRef.current?.byId.get(turn.id);
    if (frame === undefined) return;
    pointerFocusRef.current = false;
    // Neither a click nor a jump pins the preview open; the jump pins the
    // current turn instead, until the reader's scroll intent releases it.
    // The stop follows the click like the strip's focused chip does.
    setFocusedId(turn.id);
    setOpenPreviewId(null);
    pinnedTurnRef.current = turn;
    setPinnedId(turn.id);
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    frame.anchor.scrollIntoView({ behavior: reduced ? "auto" : "smooth", block: "start" });
  }, []);

  const pressStarted = useCallback(() => {
    pointerFocusRef.current = true;
  }, []);

  const openFromFocus = useCallback((turn: UserTurn) => {
    // The stop follows focus even when a pointer press suppresses the preview.
    setFocusedId(turn.id);
    const fromPress = pointerFocusRef.current;
    pointerFocusRef.current = false;
    if (fromPress) return;
    openTurnRef.current = turn;
    setOpenPreviewId(turn.id);
  }, []);

  const closePreview = useCallback(() => {
    setOpenPreviewId(null);
  }, []);

  // A card and a pin belong to a dot that is still on screen: the rail
  // hiding, a skipped dot, or a rebuilt turn list (a session switch reuses
  // ids) ends them here — a dot that unmounts takes its focus with it and
  // no blur event fires.
  useLayoutEffect(() => {
    const dotAlive = (turn: UserTurn | null): boolean =>
      turn !== null &&
      shown &&
      turns.includes(turn) &&
      measures !== null &&
      measures.byId[turn.id] !== undefined;
    if (openPreviewId !== null && !dotAlive(openTurnRef.current)) setOpenPreviewId(null);
    if (pinnedId !== null && !dotAlive(pinnedTurnRef.current)) setPinnedId(null);
  }, [shown, turns, measures, pinnedId, openPreviewId]);

  const releasePin = useCallback(() => {
    if (pinnedId === null) return;
    setPinnedId(null);
    updateCurrent();
  }, [pinnedId, updateCurrent]);

  useTurnRailIntent({ shown, scrollRef, pointerFocusRef, releasePin, closePreview });

  // One measuring pass per geometry change: turns, overflow, or the
  // observer's tick — never a streamed token, which never renders here.
  // The turn elements are resolved once per turn list (two subtree queries)
  // and reused, so a pass itself queries nothing.
  useLayoutEffect(() => {
    const conversation = scrollRef.current;
    const content = contentRef.current;
    if (conversation === null || content === null) return;

    if (turns.length > 0) {
      const cached = framesRef.current;
      if (cached === null || !framesValid(cached, turns)) {
        const byId = new Map<string, TurnFrame>();
        for (const element of content.querySelectorAll<HTMLElement>("[data-turn-anchor]")) {
          const id = element.getAttribute("data-turn-anchor");
          if (id !== null) byId.set(id, { anchor: element, bubble: null });
        }
        for (const bubble of content.querySelectorAll<HTMLElement>(".workspace-chat-bubble")) {
          const parentId = bubble.parentElement?.getAttribute("data-turn-anchor") ?? null;
          const frame = parentId === null ? undefined : byId.get(parentId);
          if (frame !== undefined) frame.bubble = bubble;
        }
        framesRef.current = { turns, byId };
      }
    }
    const frames = framesRef.current;

    const overflowingNow = overflows(conversation);
    if (overflowingNow !== overflowing) setOverflowing(overflowingNow);

    const ordered = frames === null ? null : orderedFrames(frames, turns);
    const shouldShow = overflowingNow && turns.length > 0;
    const gutterOpen = conversation.classList.contains(GUTTER_CLASS);

    if (shouldShow !== gutterOpen) {
      // The 24→56 gutter rewraps the column; hold the reader's scroll
      // anchor — the last bubble at or above the top edge — fixed in the
      // viewport. Rewrap between that anchor and the edge still shifts the
      // view by that much; nothing sits at the edge itself to hold.
      let holdFrame: TurnFrame | null = null;
      let holdTop = 0;
      if (ordered !== null && ordered.length > 0 && conversation.scrollTop > 0) {
        const viewTop = conversation.getBoundingClientRect().top;
        const holdIndex = Math.max(
          0,
          currentTurnIndex(
            (i) => ordered[i].anchor.getBoundingClientRect().top,
            ordered.length,
            viewTop,
          ),
        );
        holdFrame = ordered[holdIndex];
        holdTop = holdFrame.anchor.getBoundingClientRect().top;
      }
      conversation.classList.toggle(GUTTER_CLASS, shouldShow);
      // The composer shares the shell but not the conversation: it reads the
      // same boolean off the shell; the per-element insets live in Workspace.css.
      const shell = conversation.parentElement;
      if (shell !== null && shell.classList.contains("workspace-agent-shell")) {
        shell.classList.toggle(GUTTER_CLASS, shouldShow);
      }
      if (holdFrame !== null) {
        conversation.scrollTop += holdFrame.anchor.getBoundingClientRect().top - holdTop;
      }
    }

    if (!shouldShow || frames === null) return;

    // All reads of this pass before its state writes: the bubble centres
    // the dots park on and the widths the preview card has to clear.
    const contentWidth = content.clientWidth;
    const byId: Record<string, TurnMeasure> = {};
    frames.byId.forEach((frame, id) => {
      byId[id] = {
        center: frame.anchor.offsetTop + frame.anchor.offsetHeight / 2,
        bubbleWidth: frame.bubble === null ? 0 : frame.bubble.getBoundingClientRect().width,
      };
    });
    const next: RailMeasures = { contentWidth, byId };
    setMeasures((previous) => (sameMeasures(previous, next) ? previous : next));
    updateCurrent();
  }, [scrollRef, contentRef, overflowing, geometryTick, turns, updateCurrent]);

  // The gutter's lifetime, separate from measuring: set while shown, removed
  // on hide or unmount. The pass above owns the scroll-anchor hold, so this
  // one only converges the classes (a no-op duplicate on the transition
  // itself) and guarantees their removal.
  useLayoutEffect(() => {
    const conversation = scrollRef.current;
    // The measuring pass needs both nodes; the gutter needs only the
    // conversation, but it must not open without the content it insets.
    if (conversation === null || contentRef.current === null) return;
    const shell = conversation.parentElement;
    const shelled =
      shell !== null && shell.classList.contains("workspace-agent-shell") ? shell : null;
    conversation.classList.toggle(GUTTER_CLASS, shown);
    if (shelled !== null) shelled.classList.toggle(GUTTER_CLASS, shown);
    return () => {
      conversation.classList.remove(GUTTER_CLASS);
      shelled?.classList.remove(GUTTER_CLASS);
    };
  }, [scrollRef, contentRef, shown]);

  useEffect(() => {
    const conversation = scrollRef.current;
    const content = contentRef.current;
    if (conversation === null || content === null || turns.length === 0) return;
    const observer = new ResizeObserver(() => {
      const current = scrollRef.current;
      if (current === null) return;
      const overflowingNow = overflows(current);
      if (overflowingNow !== overflowing) setOverflowing(overflowingNow);
      if (measures === null) return;
      // The last bubble carries every bubble's geometry: content above it
      // moves it, content below it — a streamed token — does not. Streaming
      // therefore pays this probe and no re-measure.
      const lastTurn = turns[turns.length - 1];
      const last = framesRef.current?.byId.get(lastTurn.id);
      if (last === undefined || !document.contains(last.anchor)) {
        setGeometryTick((tick) => tick + 1);
        return;
      }
      const center = last.anchor.offsetTop + last.anchor.offsetHeight / 2;
      const previous = measures.byId[lastTurn.id];
      if (
        previous === undefined ||
        center !== previous.center ||
        content.clientWidth !== measures.contentWidth
      ) {
        setGeometryTick((tick) => tick + 1);
      }
    });
    observer.observe(conversation);
    observer.observe(content);
    return () => observer.disconnect();
  }, [scrollRef, contentRef, turns, overflowing, measures]);

  useEffect(() => {
    if (!shown) return;
    const conversation = scrollRef.current;
    if (conversation === null) return;
    let frame: number | null = null;
    const run = (): void => {
      frame = null;
      updateCurrent();
    };
    const onScroll = (): void => {
      if (frame === null) frame = requestAnimationFrame(run);
    };
    conversation.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      conversation.removeEventListener("scroll", onScroll);
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, [shown, scrollRef, updateCurrent]);

  if (!shown) return null;

  const activeId = pinnedId ?? currentId;
  // Dots that survived measuring, in turn order: the only tabbable set.
  const renderedIds = turns
    .filter((turn) => measures?.byId[turn.id] !== undefined)
    .map((turn) => turn.id);
  if (followedActive !== activeId) {
    setFollowedActive(activeId);
    // A moved focus keeps its stop; an untouched one tracks the turn.
    if (focusedId === followedActive) setFocusedId(activeId);
  }
  const stopId =
    focusedId !== null && renderedIds.includes(focusedId)
      ? focusedId
      : activeId !== null && renderedIds.includes(activeId)
        ? activeId
        : (renderedIds[0] ?? null);

  // Roving focus without selection semantics: arrows/Home/End move between
  // dots with no wrap, and the focus itself opens the preview. preventScroll
  // keeps the arrows from scrolling the transcript; the intent hook ignores
  // these rail keys so they never release the jump's pin either.
  const onRailKeyDown = (event: ReactKeyboardEvent<HTMLElement>): void => {
    const target = event.target;
    if (!(target instanceof HTMLElement)) return;
    const dots = [...event.currentTarget.querySelectorAll<HTMLButtonElement>(".turn-rail-dot")];
    const at = dots.indexOf(target as HTMLButtonElement);
    if (at === -1) return;
    let next: number | null = null;
    switch (event.key) {
      case "ArrowUp":
        next = at - 1;
        break;
      case "ArrowDown":
        next = at + 1;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = dots.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    if (next === null || next < 0 || next >= dots.length || next === at) return;
    setFocusedId(renderedIds[next] ?? null);
    dots[next]?.focus({ preventScroll: true });
  };

  return (
    <nav className="turn-rail" aria-label="Turns" onKeyDown={onRailKeyDown}>
      <span className="turn-rail-thread" aria-hidden="true" />
      {turns.map((turn, index) => {
        if (measures === null) return null;
        const measure = measures.byId[turn.id];
        if (measure === undefined) return null;
        const fits =
          measures.contentWidth - GUTTER_COLUMN_PX - measure.bubbleWidth >= PREVIEW_MIN_CANVAS_PX;
        return (
          <TurnStop
            key={turn.id}
            turn={turn}
            index={index}
            count={turns.length}
            center={measure.center}
            fits={fits}
            isCurrent={turn.id === activeId}
            isOpen={openPreviewId === turn.id}
            tabIndex={turn.id === stopId ? 0 : -1}
            today={today}
            jumpTo={jumpTo}
            openFromFocus={openFromFocus}
            pressStarted={pressStarted}
            closePreview={closePreview}
          />
        );
      })}
    </nav>
  );
}

export const TurnRail = memo(TurnRailInner, sameTurnInputs);
