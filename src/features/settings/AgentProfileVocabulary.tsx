/**
 * The provider-vocabulary half of the profile form: the reader that turns a
 * `providerVocabularyGet` reply into one decision per axis (`freeText`,
 * `hint`, `items`), the sentence each state renders, and the field the two
 * axes render through. The form (`AgentProfileForm.tsx`) asks the daemon and
 * owns the draft; this module decides what the answer renders as, and every
 * state the wire can reach is named here.
 */
import type { ReactNode } from "react";

/**
 * The sentence for the one absence the form can name without asking anyone:
 * the daemon predates the vocabulary query, so no answer exists to show. The
 * state sentences are pairwise distinguishable — no two are equal, and no one
 * is a substring of another, which the sentence-orthogonality test holds for
 * every rendered sentence — but they deliberately share tail clauses: the
 * discriminating words are each sentence's own reason, never the tail.
 */
export const VOCABULARY_UNAVAILABLE_TEXT =
  "This daemon is older than this app: it does not advertise the provider_vocabulary capability, so it cannot say what this provider offers. Type the model and mode below; what you type is checked when the session starts.";

/** `origin: "daemon"` — the honest sentence that travels with such a list. */
const DAEMON_VOCABULARY_TEXT =
  "This list is the daemon's own vocabulary for this provider, not something the provider published.";

/**
 * `present` whose `origin` arrived undeclared (absent, null, or a value this
 * app does not know): the reply names no author for the list. The items are
 * usable and stay offered as they arrived — what is missing is WHO authored
 * them, which is exactly what the honesty sentence exists to carry. No
 * sentence at all is what the eye reads as "the provider published this",
 * and that is the stronger of the two authorships: an undeclared list must
 * not be rendered as a declared one.
 */
function undeclaredOriginVocabularyText(axisWord: "models" | "modes"): string {
  return `This ${axisWord} list arrived with no author declared: the daemon did not say whether the provider published it or the daemon mapped it itself. Choose one from the list, or type your own instead.`;
}

/** `none`: the provider CAN answer and answered "I have none". The field stays required. */
function noneVocabularyText(axisWord: "models" | "modes"): string {
  return `This provider reports no ${axisWord}: type the one to use; a name it does not serve fails at the provider when the session starts.`;
}

/** `absent`: no source could answer. The spec's own fallback sentence. */
function absentVocabularyText(axisWord: "models" | "modes"): string {
  return `This provider did not publish its ${axisWord}; what you type is checked when the session starts.`;
}

/**
 * A reply that arrived without this axis at all: the daemon answered, and
 * what it sent cannot be read as an answer for this axis. Malformed is its
 * own state — not `absent` (which names a silent source) and not the query
 * failing (which names the transport) — and the other axis of the same
 * reply, when it arrived intact, is still shown: a malformed half must not
 * throw away a usable half.
 */
function malformedVocabularyText(axisWord: "models" | "modes"): string {
  return `The daemon's reply was malformed — it carried no ${axisWord} axis at all — so nothing is known about what this provider offers there. Type the one to use; what you type is checked when the session starts.`;
}

/**
 * `present` with an empty `items` — the one reply the spec forbids a daemon
 * to send (§5.1: a collapsed absence). "Published" and "listed none" cannot
 * both hold, so the contradiction is named and the control stays free text,
 * never a select with nothing to select.
 */
function emptyPresentVocabularyText(axisWord: "models" | "modes"): string {
  return `The daemon answered that this provider publishes its ${axisWord} and then listed none — a contradiction on the wire. Type the one to use; what you type is checked when the session starts.`;
}

/**
 * A `state` outside the `present`/`none`/`absent` union — a newer daemon's
 * fourth value, or corrupt wire. The received value is shown, never guessed
 * into one of the known states, and the field is never left without a
 * sentence: silence is the one answer that is never honest here.
 */
function unknownStateVocabularyText(axisWord: "models" | "modes", state: unknown): string {
  return `The daemon answered for the ${axisWord} axis with a value this app does not know (${
    JSON.stringify(state) ?? "undefined"
  }); it is none of present, none or absent. Type the one to use; what you type is checked when the session starts.`;
}

/**
 * What one vocabulary axis renders, decided in one place so the two axes
 * cannot drift: `freeText` picks the control, `hint` names WHICH state the
 * axis is in, `items` feed the select. The axis is read as the untrusted
 * wire value it is — every state it can reach, including the malformed and
 * the unknown, is named here, and absent is a third state that never
 * borrows another state's answer.
 */
export function vocabularyAxisView<T>(
  axis: { state: unknown; origin?: unknown; items?: readonly T[] } | undefined,
  replyArrived: boolean,
  queryFailed: boolean,
  axisWord: "models" | "modes",
  toItem: (item: T) => { value: string; label: string },
): { freeText: boolean; hint?: ReactNode; items: { value: string; label: string }[] } {
  // The query itself failed: the failure paragraph above the fields names
  // the transport reason once, and the fields stay free text under it.
  if (queryFailed) {
    return { freeText: true, items: [] };
  }
  if (axis === undefined) {
    // A reply that arrived without this axis is malformed, not absent.
    if (replyArrived) {
      return { freeText: true, hint: malformedVocabularyText(axisWord), items: [] };
    }
    // Still in flight: the fields are not rendered while the ask is out.
    return { freeText: true, items: [] };
  }
  if (axis.state === "present") {
    const items = axis.items ?? [];
    // `present` with nothing listed is the contradiction the spec forbids.
    if (items.length === 0) {
      return { freeText: true, hint: emptyPresentVocabularyText(axisWord), items: [] };
    }
    return {
      freeText: false,
      hint:
        axis.origin === "daemon"
          ? DAEMON_VOCABULARY_TEXT
          : axis.origin === "provider"
            ? undefined
            : undeclaredOriginVocabularyText(axisWord),
      items: items.map(toItem),
    };
  }
  if (axis.state === "none") {
    return { freeText: true, hint: noneVocabularyText(axisWord), items: [] };
  }
  if (axis.state === "absent") {
    return { freeText: true, hint: absentVocabularyText(axisWord), items: [] };
  }
  return { freeText: true, hint: unknownStateVocabularyText(axisWord, axis.state), items: [] };
}

/**
 * For an ACP provider whose modes are `absent`: the mode a session actually
 * runs in when the agent declares none. Prefilled once, labelled a
 * suggestion — never rendered as if the provider had said it.
 */
export const ACP_MODE_SUGGESTION = "default";
export const ACP_MODE_SUGGESTION_TEXT =
  'Suggested: "default" — the mode a session of this agent runs in when it declares none. A suggestion, not something the provider reported.';

/**
 * One vocabulary axis of the new-profile form: a select over the provider's
 * published items, or a free-text field when the answer is `none` or
 * `absent` or there is no answer at all. The `hint` names WHICH of those
 * happened: each state's sentence carries its own reason clause, no two
 * rendered sentences are equal or substrings of one another (the
 * sentence-orthogonality test holds them pairwise), though the sentences
 * deliberately share tail clauses.
 */
export function VocabularyField({
  label,
  value,
  busy,
  freeText,
  hint,
  suggestion,
  items,
  onChange,
  onSettle,
}: {
  label: string;
  value: string;
  busy: boolean;
  /** True: a free-text input. False: a select over `items`. */
  freeText: boolean;
  /** The sentence under the field naming why it reads what it reads. */
  hint?: ReactNode;
  /** The ACP mode suggestion, only where it applies; labelled a suggestion. */
  suggestion?: ReactNode;
  items: readonly { value: string; label: string }[];
  onChange: (next: string) => void;
  /** The field's value stopped changing and the answer must follow it. A select
   *  settles on its change; a free-text field settles on blur, so a human
   *  typing a model id does not start one provider read per character. */
  onSettle?: (next: string) => void;
}) {
  if (freeText) {
    return (
      <>
        <label className="device-field">
          {label}
          <input
            aria-label={label}
            value={value}
            disabled={busy}
            onChange={(event) => onChange(event.target.value)}
            onBlur={() => onSettle?.(value)}
          />
        </label>
        {hint === undefined ? null : <p className="device-field-hint">{hint}</p>}
        {suggestion === undefined ? null : <p className="device-field-hint">{suggestion}</p>}
      </>
    );
  }
  return (
    <label className="device-field">
      {label}
      <select
        aria-label={label}
        value={value}
        disabled={busy}
        onChange={(event) => {
          onChange(event.target.value);
          onSettle?.(event.target.value);
        }}
      >
        <option value="">Choose a {label.toLowerCase()}…</option>
        {items.map((item) => (
          <option key={item.value} value={item.value}>
            {item.label}
          </option>
        ))}
      </select>
      {hint === undefined ? null : <span className="device-field-hint">{hint}</span>}
    </label>
  );
}
