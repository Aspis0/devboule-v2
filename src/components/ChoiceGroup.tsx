import { useRef, type KeyboardEvent } from "react";
import { stepChoiceIndex, type ChoiceStep } from "./choiceNavigation";
import "./ChoiceGroup.css";

export interface ChoiceOption<T extends string> {
  value: T;
  label: string;
  disabled?: boolean;
  /** An offline host: shown with its state in words, and never selectable. */
  hint?: "offline";
}

export interface ChoiceGroupProps<T extends string> {
  options: readonly ChoiceOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** Horizontal is a segmented control for a few short labels; vertical is a list. */
  orientation?: "horizontal" | "vertical";
  /** Saving or otherwise busy: every option stays visible and none can be picked. */
  disabled?: boolean;
  ariaLabel?: string;
  labelledBy?: string;
  describedBy?: string;
}

const KEY_STEPS: Readonly<Record<string, ChoiceStep>> = {
  ArrowLeft: "previous",
  ArrowUp: "previous",
  ArrowRight: "next",
  ArrowDown: "next",
  Home: "first",
  End: "last",
};

/**
 * The one way to pick one value among a few named ones. Radio semantics on
 * buttons: arrows move focus and select together, as native radios do, so a
 * keyboard user never has to press Space after the arrow.
 */
export function ChoiceGroup<T extends string>({
  options,
  value,
  onChange,
  orientation = "horizontal",
  disabled = false,
  ariaLabel,
  labelledBy,
  describedBy,
}: ChoiceGroupProps<T>) {
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const off = options.map(
    (option) => disabled || option.disabled === true || option.hint === "offline",
  );
  const selected = options.findIndex((option) => option.value === value);
  // One tab stop: the selected option, or the first enabled one when the
  // selected one is disabled or unknown.
  const tabStop = selected !== -1 && !off[selected] ? selected : off.indexOf(false);

  function select(index: number) {
    if (off[index] || options[index].value === value) return;
    onChange(options[index].value);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const step = KEY_STEPS[event.key];
    if (step === undefined) return;
    event.preventDefault();
    const target = stepChoiceIndex(off, index, step);
    if (target === null) return;
    buttons.current[target]?.focus();
    select(target);
  }

  const className = [
    "choice-group",
    orientation === "vertical" ? "choice-group-vertical" : null,
    disabled ? "is-disabled" : null,
  ]
    .filter((name): name is string => name !== null)
    .join(" ");

  return (
    <div
      className={className}
      role="radiogroup"
      aria-orientation={orientation === "vertical" ? "vertical" : undefined}
      aria-label={ariaLabel}
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
    >
      {options.map((option, index) => {
        const offline = option.hint === "offline";
        return (
          <button
            key={option.value}
            ref={(node) => {
              buttons.current[index] = node;
            }}
            type="button"
            role="radio"
            className="choice-group-option"
            data-choice-value={option.value}
            aria-checked={index === selected}
            aria-disabled={off[index] ? true : undefined}
            tabIndex={index === tabStop ? 0 : -1}
            title={offline ? `${option.label} is offline` : undefined}
            onClick={() => select(index)}
            onKeyDown={(event) => handleKeyDown(event, index)}
          >
            {offline ? <span className="choice-group-dot" aria-hidden="true" /> : null}
            {option.label}
            {offline ? <span className="choice-group-offline">offline</span> : null}
          </button>
        );
      })}
    </div>
  );
}
