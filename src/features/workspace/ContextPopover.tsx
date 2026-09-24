import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import type { PlanUsage } from "../../types/ipc";
import {
  formatContextTokens,
  planWindowLabel,
  planWindowMeta,
  type ContextMeterNumbers,
} from "./contextUsageView";
import { POPOVER_MARGIN } from "./popoverPlace";

const POPOVER_RING_RADIUS = 12;
const POPOVER_RING_CIRCUMFERENCE = 2 * Math.PI * POPOVER_RING_RADIUS;
/** The gap off the anchor — the spec's offset above the ring (Paseo `offset={8}`). */
const ANCHOR_GAP = 8;
/** `.workspace-context-popover`'s CSS width: happy-dom has no layout, so a
    zero measurement falls back to the number the stylesheet declares. */
const POPOVER_WIDTH_PX = 300;

export interface AnchorBox {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export interface ViewportBox {
  width: number;
  height: number;
}

export interface PopoverPlacement {
  left: number;
  top: number;
  width: number;
  above: boolean;
}

/**
 * Where the fixed-positioned popover goes: centred above its anchor with the
 * gap, clamped inside the viewport margins, flipped below when the top has no
 * room, and on the roomier side when neither side fits. Pure — the rects and
 * the viewport come in, so the arithmetic is testable without layout (the
 * live check measured exactly this through CDP).
 */
export function placeContextPopover(
  anchor: AnchorBox,
  popover: { width: number; height: number },
  viewport: ViewportBox,
  gap: number,
): PopoverPlacement {
  const width = Math.min(popover.width, Math.max(0, viewport.width - POPOVER_MARGIN * 2));
  const centre = anchor.left + (anchor.right - anchor.left) / 2;
  const left = Math.min(
    viewport.width - POPOVER_MARGIN - width,
    Math.max(POPOVER_MARGIN, centre - width / 2),
  );
  const roomAbove = anchor.top - gap - POPOVER_MARGIN;
  const roomBelow = viewport.height - POPOVER_MARGIN - anchor.bottom - gap;
  const above =
    roomAbove >= popover.height || (roomBelow < popover.height && roomAbove >= roomBelow);
  const rawTop = above ? anchor.top - gap - popover.height : anchor.bottom + gap;
  const top = Math.max(
    POPOVER_MARGIN,
    Math.min(rawTop, viewport.height - POPOVER_MARGIN - popover.height),
  );
  return { left, top, width, above };
}

export interface ContextPopoverProps {
  /** The meter's button: the panel is centred above it, and a press inside
      it does not count as an outside click. */
  anchorRef: RefObject<HTMLElement | null>;
  /** Close the popover — Escape or a click outside both elements. */
  onClose: () => void;
  numbers: ContextMeterNumbers;
  /** Whether the reading is a mid-turn push (`true`) or the end of the last
      turn (`false`, labelled as such). */
  live: boolean;
  /** The provider's latest plan frame, or null when it reports none. */
  plan: PlanUsage | null;
}

function readingBody(numbers: ContextMeterNumbers, live: boolean): ReactNode {
  const { used, max, percent } = numbers;
  const stale = live ? null : <div className="workspace-context-note">as of the last turn</div>;
  if (used !== null && max !== null && percent !== null) {
    const offset = POPOVER_RING_CIRCUMFERENCE * (1 - Math.max(0, Math.min(100, percent)) / 100);
    return (
      <>
        <svg
          width={26}
          height={26}
          viewBox="0 0 26 26"
          className="workspace-context-ring"
          aria-hidden="true"
        >
          <circle
            cx={13}
            cy={13}
            r={POPOVER_RING_RADIUS}
            fill="none"
            stroke="var(--line-strong)"
            strokeWidth={2}
          />
          <circle
            cx={13}
            cy={13}
            r={POPOVER_RING_RADIUS}
            fill="none"
            stroke="var(--accent)"
            strokeWidth={2}
            strokeLinecap="round"
            strokeDasharray={POPOVER_RING_CIRCUMFERENCE}
            strokeDashoffset={offset}
            transform="rotate(-90 13 13)"
          />
        </svg>
        <div className="workspace-context-reading">
          <div className="workspace-context-percent">{percent}% used</div>
          <div className="workspace-context-tokens">
            {formatContextTokens(used)} / {formatContextTokens(max)} tokens
          </div>
          {stale}
        </div>
      </>
    );
  }
  if (used !== null) {
    return (
      <div className="workspace-context-reading">
        <div className="workspace-context-tokens">{formatContextTokens(used)} tokens used</div>
        {stale}
      </div>
    );
  }
  return (
    <div className="workspace-context-reading">
      <div className="workspace-context-note">No context reading yet.</div>
    </div>
  );
}

function planBody(plan: PlanUsage | null, nowMs: number): ReactNode {
  if (plan === null) {
    return <div className="workspace-context-note">This provider does not report plan usage.</div>;
  }
  return (
    <>
      {plan.planLabel !== undefined ? (
        <span className="workspace-context-plan-chip">{plan.planLabel}</span>
      ) : null}
      {plan.windows.map((window, index) => {
        const meta = planWindowMeta(window, nowMs);
        return (
          // The pair (position, duration) — duration alone is not a key: the
          // frame is free to carry two windows of one length.
          <div className="workspace-context-window" key={`${index}-${window.durationMins}`}>
            <div className="workspace-context-window-row">
              <span className="workspace-context-window-label">
                {planWindowLabel(window.durationMins)}
              </span>
              {meta !== null ? <span className="workspace-context-window-meta">{meta}</span> : null}
            </div>
            <div className="workspace-context-window-bar">
              {window.usedPercent !== undefined ? (
                <div
                  className="workspace-context-window-fill"
                  style={{ width: `${Math.max(0, Math.min(100, window.usedPercent))}%` }}
                />
              ) : null}
            </div>
          </div>
        );
      })}
      {plan.credits !== undefined && plan.credits.unlimited ? (
        <div className="workspace-context-credits">Credits: unlimited</div>
      ) : plan.credits !== undefined && plan.credits.balance !== undefined ? (
        <div className="workspace-context-credits">Credits: {plan.credits.balance}</div>
      ) : null}
    </>
  );
}

/**
 * The context popover: 300 px of plain facts above the ring — the reading the
 * meter draws, then the provider's own plan windows, or one sentence saying
 * the provider sends none. Every number here is one the provider sent.
 *
 * It renders through a portal on `document.body` with fixed positioning: the
 * meter's pane (`.workspace-center-panel`) clips its own children, which is
 * what cut this panel's right edge off in the live check. The panel therefore
 * never inherits a clipping ancestor, and the placement arithmetic lives in
 * {@link placeContextPopover}.
 */
export function ContextPopover({ anchorRef, onClose, numbers, live, plan }: ContextPopoverProps) {
  const popoverRef = useRef<HTMLDivElement>(null);
  const [placement, setPlacement] = useState<PopoverPlacement | null>(null);
  // The countdown must keep counting while the popover sits open; a render
  // that never happens would freeze "resets in" at whatever it said on open.
  const [nowMs, setNowMs] = useState(() => Date.now());

  const update = useCallback(() => {
    const anchor = anchorRef.current;
    const popover = popoverRef.current;
    if (anchor === null || popover === null) return;
    const anchorRect = anchor.getBoundingClientRect();
    const popoverRect = popover.getBoundingClientRect();
    setPlacement(
      placeContextPopover(
        anchorRect,
        {
          width: popoverRect.width || POPOVER_WIDTH_PX,
          height: popoverRect.height,
        },
        { width: window.innerWidth, height: window.innerHeight },
        ANCHOR_GAP,
      ),
    );
  }, [anchorRef]);

  // Measure before paint so the panel never flashes at an unplaced position,
  // and again whenever its own content (a plan frame, a ticking countdown)
  // can change its height under a fixed top.
  useLayoutEffect(() => {
    update();
  }, [update, numbers, plan, nowMs]);

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onClose();
    };
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (
        target instanceof Node &&
        !anchorRef.current?.contains(target) &&
        !popoverRef.current?.contains(target)
      ) {
        onClose();
      }
    };
    // Fixed positioning ignores ancestor movement, so the panel only moves
    // when told: the window resizes, or the composer's own box does (a panel
    // drag changes its width; a growing textarea changes its height).
    const onViewport = () => update();
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("resize", onViewport);
    const composer = anchorRef.current?.closest(".workspace-composer");
    let observer: ResizeObserver | undefined;
    if (composer !== null && composer !== undefined && "ResizeObserver" in globalThis) {
      observer = new ResizeObserver(onViewport);
      observer.observe(composer);
    }
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("resize", onViewport);
      observer?.disconnect();
    };
  }, [anchorRef, onClose, update]);

  useEffect(() => {
    const timer = globalThis.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => globalThis.clearInterval(timer);
  }, []);

  const style: CSSProperties =
    placement === null
      ? { visibility: "hidden" }
      : {
          position: "fixed",
          left: placement.left,
          top: placement.top,
          width: placement.width,
        };

  return createPortal(
    <div
      ref={popoverRef}
      className="workspace-context-popover"
      role="dialog"
      aria-label="Context usage"
      style={style}
    >
      <div className="workspace-context-popover-head">{readingBody(numbers, live)}</div>
      <div className="workspace-context-popover-plan">{planBody(plan, nowMs)}</div>
    </div>,
    document.body,
  );
}
