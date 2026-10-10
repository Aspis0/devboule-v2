import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { placeMenu } from "./PickerChip";
import { useMenuOpen } from "../lib/menuOpen";
import { scrollRowIntoView } from "../lib/scrollRowIntoView";
import "./AppSelect.css";

export interface AppSelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface AppSelectProps {
  /** The field's name: the trigger's accessible name and the listbox's label. */
  label: string;
  value: string;
  options: readonly AppSelectOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  testId?: string;
}

/**
 * The app's own select: a field-shaped trigger that opens the composer chips'
 * listbox (the same placement, outside-press and arrow-key handling), in place
 * of a native popup. A value no option names is shown as the raw value.
 */
export function AppSelect({
  label,
  value,
  options,
  onChange,
  disabled = false,
  testId,
}: AppSelectProps) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [openRequested, setOpenRequested] = useState(false);
  // Derived, so a field that becomes disabled while open unmounts its menu too.
  const open = openRequested && !disabled;
  useMenuOpen(open, () => setOpenRequested(false));

  // Escape and a press outside the field close the menu without picking. The
  // trigger is excluded: its own click is the toggle.
  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpenRequested(false);
    };
    const closeOnOutsideClick = (event: MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (menuRef.current?.contains(target) || triggerRef.current?.contains(target)) return;
      setOpenRequested(false);
    };
    document.addEventListener("keydown", closeOnEscape);
    document.addEventListener("click", closeOnOutsideClick);
    return () => {
      document.removeEventListener("keydown", closeOnEscape);
      document.removeEventListener("click", closeOnOutsideClick);
    };
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    const menu = menuRef.current;
    const trigger = triggerRef.current;
    if (menu === null || trigger === null) return;
    const place = (): void => placeMenu(trigger, menu);
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [open]);

  const shown = options.find((option) => option.value === value)?.label ?? value;

  const handleMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const menu = menuRef.current;
    const rows = [...(menu?.querySelectorAll<HTMLButtonElement>("[role='option']:enabled") ?? [])];
    const index = rows.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      event.key === "ArrowDown"
        ? (rows[Math.min(index + 1, rows.length - 1)] ?? rows[0])
        : (rows[Math.max(index - 1, 0)] ?? rows[0]);
    next?.focus();
    if (next !== undefined && menu !== null) scrollRowIntoView(menu, next);
  };

  return (
    <div className="app-select">
      <button
        type="button"
        ref={triggerRef}
        className="app-select-trigger"
        data-testid={testId}
        data-value={value}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-label={label}
        disabled={disabled}
        onClick={() => setOpenRequested((current) => !current)}
      >
        <span className="app-select-value">{shown}</span>
        <svg
          className="app-select-caret"
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
          id={listId}
          ref={menuRef}
          className="workspace-mode-menu app-select-menu"
          role="listbox"
          aria-label={label}
          onKeyDown={handleMenuKeyDown}
        >
          {options.map((option) => (
            <button
              type="button"
              role="option"
              data-value={option.value}
              className="workspace-mode-option"
              key={option.value}
              aria-selected={option.value === value}
              disabled={option.disabled}
              onClick={() => {
                setOpenRequested(false);
                onChange(option.value);
              }}
            >
              <span className="workspace-mode-name">{option.label}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
