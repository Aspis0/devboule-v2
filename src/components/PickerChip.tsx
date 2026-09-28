import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useMenuOpen } from "../lib/menuOpen";
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
}: PickerChipProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [openRequested, setOpenRequested] = useState(false);
  // A fatal event can flip `disabled` while the menu is open; deriving `open`
  // means the menu unmounts with the chip in the same render, and its option
  // buttons cannot reach `onSelect` on a gone view.
  const open = openRequested && !disabled;
  useMenuOpen(open, () => setOpenRequested(false));

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
  const shown = current?.name ?? currentId ?? label;

  const handleMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const optionButtons = [
      ...(menuRef.current?.querySelectorAll<HTMLButtonElement>("[role='option']") ?? []),
    ];
    const index = optionButtons.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      event.key === "ArrowDown"
        ? (optionButtons[Math.min(index + 1, optionButtons.length - 1)] ?? optionButtons[0])
        : (optionButtons[Math.max(index - 1, 0)] ?? optionButtons[0]);
    next?.focus();
  };

  return (
    <div ref={menuRef} className="workspace-mode-chip">
      <button
        type="button"
        className="workspace-mode-chip-trigger"
        data-testid={chipTestId}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
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
        <span className="workspace-mode-caret" aria-hidden="true">
          ▾
        </span>
      </button>
      {open ? (
        <div
          className="workspace-mode-menu"
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
