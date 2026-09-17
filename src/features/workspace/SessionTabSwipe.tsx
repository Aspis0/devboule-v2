// One session tab's swipe surface. A pointer drag reveals the named act
// underneath — Archive to the right, Delete to the left — and releasing past
// SWIPE_COMMIT_PX commits it. Released short, the tab springs back. Pointer
// events, not mouse events: this strip also ships to touch platforms.

import { useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";

/** Pixels of horizontal travel that commit the gesture on release. */
export const SWIPE_COMMIT_PX = 90;

/** Travel that turns a press into a drag. Exported for the capture rule's boundary test. */
export const SWIPE_SLOP_PX = 8;
const SWIPE_MAX_PX = 160;

interface SessionTabSwipeProps {
  onCommit: (direction: "archive" | "delete") => void;
  children: ReactNode;
}

/**
 * True once the press has become a drag. Capture belongs exactly there:
 * capturing on press retargets the compatibility mouse events in WebView2,
 * so the click never reaches the tab button inside. A plain click must never
 * capture; a real drag still captures before it can leave the element.
 */
export function shouldCapturePointer(travelPx: number, capturing: boolean): boolean {
  if (capturing) return false;
  return Math.abs(travelPx) >= SWIPE_SLOP_PX;
}

function capturePointer(element: HTMLElement, pointerId: number): void {
  try {
    element.setPointerCapture?.(pointerId);
  } catch {
    // happy-dom and older webviews may not implement capture; the gesture
    // still completes from the events the element receives.
  }
}

function release(element: HTMLElement, pointerId: number): void {
  try {
    if (element.hasPointerCapture?.(pointerId)) element.releasePointerCapture(pointerId);
  } catch {
    // Same as above: releasing is a courtesy, not a requirement.
  }
}

export function SessionTabSwipe({ onCommit, children }: SessionTabSwipeProps) {
  const [offset, setOffset] = useState(0);
  const [dragging, setDragging] = useState(false);
  const startXRef = useRef(0);
  const activeRef = useRef(false);
  const committedRef = useRef(false);
  const capturingRef = useRef(false);
  // Travel in a ref as well as in state: the release reads it, and state
  // alone would still hold the pre-drag value when a full drag lands inside
  // one batched update.
  const offsetRef = useRef(0);

  const setTravel = (value: number): void => {
    offsetRef.current = value;
    setOffset(value);
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    activeRef.current = true;
    committedRef.current = false;
    capturingRef.current = false;
    startXRef.current = event.clientX;
    // No capture here: in WebView2 a press-time capture retargets the
    // compatibility mouse events, and the click never reaches the tab.
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (!activeRef.current || committedRef.current) return;
    const raw = event.clientX - startXRef.current;
    if (Math.abs(raw) < SWIPE_SLOP_PX) return;
    if (shouldCapturePointer(raw, capturingRef.current)) {
      capturingRef.current = true;
      capturePointer(event.currentTarget, event.pointerId);
    }
    if (!dragging) setDragging(true);
    setTravel(Math.max(-SWIPE_MAX_PX, Math.min(SWIPE_MAX_PX, raw)));
  };

  const endDrag = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (!activeRef.current) return;
    activeRef.current = false;
    if (capturingRef.current) {
      capturingRef.current = false;
      release(event.currentTarget, event.pointerId);
    }
    setDragging(false);
    if (committedRef.current) return;
    const travelled = offsetRef.current;
    if (travelled <= -SWIPE_COMMIT_PX) {
      committedRef.current = true;
      onCommit("archive");
      return;
    }
    if (travelled >= SWIPE_COMMIT_PX) {
      committedRef.current = true;
      onCommit("delete");
      return;
    }
    setTravel(0);
  };

  // Opacity follows travel so the name fades in with the reveal instead of
  // sitting at full strength behind a tab that barely moved.
  const reveal = Math.min(1, Math.abs(offset) / SWIPE_COMMIT_PX);

  // The strip's tablist owns the tab buttons by ARIA parent/child ownership,
  // which these two positioning boxes would break as plain generic divs.
  // Both are presentational, so the tabs (and the row's buttons, as before)
  // stay the tablist's owned elements; the underlays are aria-hidden anyway.
  // Method: reasoned from the WAI-ARIA ownership rules, not measured in a
  // browser — this environment has no accessibility-tree automation.
  return (
    <div className="session-swipe" role="presentation">
      <div
        className="session-swipe-underlay session-swipe-underlay-delete"
        aria-hidden="true"
        style={{ opacity: offset > 0 ? reveal : 0 }}
      >
        <span>Delete · destroys the session</span>
      </div>
      <div
        className="session-swipe-underlay session-swipe-underlay-archive"
        aria-hidden="true"
        style={{ opacity: offset < 0 ? reveal : 0 }}
      >
        <span>Archive · keeps messages</span>
      </div>
      <div
        className="session-swipe-content"
        role="presentation"
        data-testid="session-swipe-content"
        style={{
          transform: `translateX(${offset}px)`,
          transition: dragging ? "none" : "transform 180ms ease-out",
        }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
      >
        {children}
      </div>
    </div>
  );
}
