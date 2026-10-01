import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useMenuOpen } from "../lib/menuOpen";
import { scrollRowIntoView } from "../lib/scrollRowIntoView";
import { POPOVER_MARGIN, placePopover } from "../features/workspace/popoverPlace";
import "./PickerChip.css";

// Moved out of AgentChatSurface unchanged, so Design renders the same control.
// The class names stay `workspace-mode-*` and their rules moved to PickerChip.css
// with it: neither surface changes how it draws.
type ModeTier = "planning" | "safe" | "moderate" | "dangerous" | "neutral";

const MODE_TIERS: Record<string, ModeTier> = {
  plan: "planning",
  default: "safe",
  ask: "safe",
  acceptEdits: "moderate",
  auto: "moderate",
  "auto-edit": "moderate",
  auto_accept: "moderate",
  "auto-review": "moderate",
  bypassPermissions: "dangerous",
  bypass: "dangerous",
  yolo: "dangerous",
  "full-access": "dangerous",
};

function modeTier(modeId: string): ModeTier {
  return MODE_TIERS[modeId] ?? "neutral";
}

export function modeDotClass(modeId: string): string {
  return `workspace-mode-dot workspace-mode-${modeTier(modeId)}`;
}

interface PickerOption {
  id: string;
  name: string;
  description?: string;
}

/** The menu's side and cap, reusing the anchored popovers' arithmetic on an
 * inline measurement — not the portal, so outside-click and z-index stay as
 * they are. The probe reads the content with the cap lifted: through the
 * sheet's fallback cap it reports the capped box, never the content. `clip`
 * scopes the room to the box that actually clips the menu; left off, the
 * workspace panel (or the window) sets it, exactly as before. */
export function menuPlacement(
  trigger: HTMLElement,
  menu: HTMLElement,
  clip?: Element | null,
): { maxHeight: number; below: boolean } {
  const anchorBox = trigger.getBoundingClientRect();
  const inlineCap = menu.style.maxHeight;
  menu.style.maxHeight = "none";
  const probe = { width: menu.scrollWidth, height: menu.scrollHeight };
  menu.style.maxHeight = inlineCap;
  const panel = clip ?? trigger.closest(".workspace-center-panel");
  let anchor: { left: number; right: number; top: number; bottom: number } = anchorBox;
  let viewport = { width: window.innerWidth, height: window.innerHeight };
  if (panel !== null) {
    const box = panel.getBoundingClientRect();
    anchor = {
      left: anchorBox.left - box.left,
      right: anchorBox.right - box.left,
      top: anchorBox.top - box.top,
      bottom: anchorBox.bottom - box.top,
    };
    viewport = { width: box.width, height: box.height };
  }
  const placed = placePopover(anchor, probe, viewport, POPOVER_MARGIN, true);
  // placePopover keeps the side to itself; the top gives it away: opening
  // below starts the menu past the anchor's top edge, opening above ends
  // it before that edge.
  return { maxHeight: placed.maxHeight, below: placed.top > anchor.top };
}

/** Sets the open's side and cap on the menu and brings the selected row
 * along: the open effect and its resize paths share it, so a recompute
 * can never drift from the open. */
function placeMenu(trigger: HTMLElement, menu: HTMLElement): void {
  const placed = menuPlacement(trigger, menu);
  if (placed.maxHeight > 0) menu.style.maxHeight = `${placed.maxHeight}px`;
  if (placed.below) {
    menu.style.top = "calc(100% + 6px)";
    menu.style.bottom = "auto";
  } else {
    menu.style.bottom = "calc(100% + 6px)";
    menu.style.top = "auto";
  }
  const selected = menu.querySelector<HTMLElement>("[aria-selected='true']");
  if (selected !== null) scrollRowIntoView(menu, selected);
}

interface PickerChipProps {
  label: string;
  options: PickerOption[];
  currentId: string | null;
  onSelect: (id: string) => void;
  chipTestId: string;
  optionTestId: (id: string) => string;
  dotFor?: (id: string) => string;
  /** Terminal-session guard: the chip must be unclickable, not merely styled. */
  disabled?: boolean;
  /** What the chip switches, for a chip whose value alone does not say. */
  tooltip?: string;
  /** Drawn before the current name, joined with " · ": the provider half of
   * the provider·model chip, which must show both names on one trigger. */
  prefix?: string;
  /** Replaces the current name while a switch is in flight, so the pending
   * target stays visible without a second line above the transcript. */
  pendingCopy?: string;
}

/** One chip + listbox picker shared by the mode, model, and effort controls. */
export function PickerChip({
  label,
  options,
  currentId,
  onSelect,
  chipTestId,
  optionTestId,
  dotFor,
  disabled = false,
  tooltip,
  prefix,
  pendingCopy,
}: PickerChipProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuBodyRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [openRequested, setOpenRequested] = useState(false);
  // A fatal event can flip `disabled` while the menu is open; deriving `open`
  // means the menu unmounts with the chip in the same render, and its option
  // buttons cannot reach `onSelect` on a gone view.
  const open = openRequested && !disabled;
  useMenuOpen(open, () => setOpenRequested(false));

  // The menu must stay inside the centre panel's hidden overflow: open on
  // the side menuPlacement chose, capped to that side's room, and bring the
  // current row along. The cap is the trigger's room, so a window resize or
  // a row change re-runs the same placement while open. happy-dom measures
  // every rectangle as zero, so the geometry is pinned by the menuPlacement
  // unit tests and only the scroll is exercised through React here.
  useLayoutEffect(() => {
    if (!open || options.length === 0) return;
    const menu = menuBodyRef.current;
    const trigger = triggerRef.current;
    if (menu === null || trigger === null) return;
    const place = (): void => placeMenu(trigger, menu);
    place();
    window.addEventListener("resize", place);
    const sizes = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);
    if (sizes !== null) sizes.observe(menu);
    return () => {
      window.removeEventListener("resize", place);
      sizes?.disconnect();
    };
  }, [open, currentId, options.length]);

  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpenRequested(false);
    };
    const closeOnOutsideClick = (event: MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Node) || !menuRef.current?.contains(target)) setOpenRequested(false);
    };
    document.addEventListener("keydown", closeOnEscape);
    document.addEventListener("click", closeOnOutsideClick);
    return () => {
      document.removeEventListener("keydown", closeOnEscape);
      document.removeEventListener("click", closeOnOutsideClick);
    };
  }, [open]);

  if (options.length === 0) return null;

  const current = options.find((option) => option.id === currentId) ?? null;
  const name = pendingCopy ?? current?.name ?? currentId ?? null;
  const shown =
    [prefix, name]
      .filter((part): part is string => part !== undefined && part !== null && part !== "")
      .join(" · ") || label;

  const handleMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const menu = menuBodyRef.current;
    const optionButtons = [...(menu?.querySelectorAll<HTMLButtonElement>("[role='option']") ?? [])];
    const index = optionButtons.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      event.key === "ArrowDown"
        ? (optionButtons[Math.min(index + 1, optionButtons.length - 1)] ?? optionButtons[0])
        : (optionButtons[Math.max(index - 1, 0)] ?? optionButtons[0]);
    next?.focus();
    if (next !== undefined && menu !== null) scrollRowIntoView(menu, next);
  };

  return (
    <div ref={menuRef} className="workspace-mode-chip">
      <button
        type="button"
        ref={triggerRef}
        className="workspace-mode-chip-trigger"
        data-testid={chipTestId}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-busy={pendingCopy !== undefined}
        title={tooltip}
        aria-label={tooltip === undefined ? undefined : `${tooltip} (${shown})`}
        disabled={disabled}
        onClick={() => {
          if (disabled) return;
          setOpenRequested((value) => !value);
        }}
      >
        {current !== null && dotFor !== undefined ? (
          <span className={dotFor(current.id)} aria-hidden="true" />
        ) : null}
        <span>{shown}</span>
        <svg
          className="workspace-mode-caret"
          width={12}
          height={12}
          viewBox="0 0 24 24"
          aria-hidden="true"
          focusable="false"
        >
          <path d="m6 9 6 6 6-6" />
        </svg>
      </button>
      {open ? (
        <div
          className="workspace-mode-menu"
          ref={menuBodyRef}
          id={listId}
          role="listbox"
          aria-label={label}
          onKeyDown={handleMenuKeyDown}
        >
          {options.map((option) => (
            <button
              type="button"
              role="option"
              className="workspace-mode-option"
              key={option.id}
              aria-selected={option.id === currentId}
              data-testid={optionTestId(option.id)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => {
                onSelect(option.id);
                setOpenRequested(false);
              }}
            >
              <span className="workspace-mode-name">{option.name}</span>
              {option.description ? (
                <span className="workspace-mode-description">{option.description}</span>
              ) : null}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
