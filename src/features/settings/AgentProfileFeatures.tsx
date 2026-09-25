/**
 * The feature controls of the profile form: one per feature the selected
 * provider actually offers, drawn from the daemon's answer and nowhere else.
 *
 * The rule this file exists for is the one the reverted feature editor died
 * for: a control is drawn only for a value the spawn path really applies, and
 * the daemon is the only thing that knows which those are. So nothing here
 * names a provider, a model or a feature — the vocabulary reply carries the
 * list, the label, the control kind and a row's choices, and this component
 * draws what it is handed and writes the values back keyed as it received them
 * (AgentProfileDraft.ts owns the storage rules).
 *
 * It also owns the two things a form must say when it has no list to draw:
 * whether the provider's answer is still being read (an ACP provider's list is
 * learned by starting it once, so the first open waits) and whether this daemon
 * predates the question entirely. Both are silences a human would otherwise
 * read as "this provider offers nothing", which is a different fact and the one
 * that makes them leave a tick off.
 */
import type { VocabularyFeature } from "../../types/ipc";
import { AUTO_ACCEPT_FEATURE } from "./AgentProfileDraft";

/** The consent-bearing sentence every agent family's tick needs and no other
 *  feature does: the difference between a child that will ask this human and
 *  one that will not. The daemon supplies the label; this is the one thing the
 *  form says in its own words, and only for the key the daemon reads as a
 *  constraint on the delivered mode. */
const AUTO_ACCEPT_NOTE =
  "Children created from this profile approve their own permission prompts instead of asking you.";
const UNSET_SELECT_VALUE = "__devboule_profile_feature_unset__";

function unsetSelectValue(feature: VocabularyFeature): string {
  let value = UNSET_SELECT_VALUE;
  while (feature.options?.some((option) => option.id === value)) value += "_";
  return value;
}

/** One control, keyed by the provider's own feature id. */
function FeatureControl({
  feature,
  value,
  busy,
  onChange,
}: {
  feature: VocabularyFeature;
  value: boolean | string | undefined;
  busy: boolean;
  onChange: (id: string, value: boolean | string | undefined) => void;
}) {
  if (feature.type === "select") {
    const unsetValue = unsetSelectValue(feature);
    return (
      <label className="device-field">
        {feature.label}
        <select
          aria-label={`Profile feature ${feature.id}`}
          value={typeof value === "string" ? value : unsetValue}
          disabled={busy}
          onChange={(event) =>
            onChange(feature.id, event.target.value === unsetValue ? undefined : event.target.value)
          }
        >
          {/* The default position is distinct from a declared empty-string
              choice, which remains a provider value. */}
          <option value={unsetValue}>The provider's own default</option>
          {(feature.options ?? []).map((option) => (
            <option key={option.id} value={option.id}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
    );
  }
  const checked = value === true;
  return (
    <label className="agent-profile-tick">
      <input
        type="checkbox"
        aria-label={
          feature.id === AUTO_ACCEPT_FEATURE
            ? "Auto accept for children of this profile"
            : `Profile feature ${feature.id}`
        }
        checked={checked}
        disabled={busy}
        onChange={(event) => onChange(feature.id, event.target.checked)}
      />
      <span>
        <span>{feature.label}</span>
        {feature.id === AUTO_ACCEPT_FEATURE ? (
          <span className="agent-profile-tick-note">{AUTO_ACCEPT_NOTE}</span>
        ) : null}
      </span>
    </label>
  );
}

/**
 * The row the form draws when the provider's list is unknown: the tick, with
 * the daemon's own reading of it. Every agent family applies `autoAccept` and
 * every one refuses a contradiction, so a form that hid it would take a control
 * away from a profile that still works — and a save with no list to prune by
 * carries the stored key regardless, which is a tick the human can neither see
 * nor clear. That is true for a daemon predating the features axis, for a failed
 * query, and for an ACP provider whose read has not landed yet.
 *
 * Synthesised in the caller rather than stored on the draft, so it can never be
 * mistaken for the provider's answer: `author: "daemon"` says who wrote it.
 */
export const AUTO_ACCEPT_FALLBACK_FEATURE: VocabularyFeature = {
  id: AUTO_ACCEPT_FEATURE,
  label: "Auto accept",
  author: "daemon",
  type: "toggle",
};

/**
 * The feature section of the profile form.
 *
 * `offered === null` is the case with no list to draw — the query failed, the
 * daemon predates the axis, or an ACP read has not landed. The tick is still
 * drawn (see `AUTO_ACCEPT_FALLBACK_FEATURE`), and nothing stored is judged away;
 * `probing` names the one of those cases where waiting changes what appears,
 * because it is the only one where a second look is worth the human's patience.
 */
export function AgentProfileFeatureFields({
  offered,
  probing,
  askedAndFailed,
  features,
  busy,
  onChange,
}: {
  /** The provider's offered features for the current model, or `null` when no
   *  answer is in hand. */
  offered: readonly VocabularyFeature[] | null;
  /** The answer is still being read: an ACP provider's first open. */
  probing: boolean;
  /** The read was made and could not be answered — the provider is not running
   *  or it did not answer. Distinct from `offered === null` with no read at
   *  all, which is a daemon older than the axis and says nothing. */
  askedAndFailed: boolean;
  /** The draft's stored values, keyed as the provider spells them. */
  features: Record<string, boolean | string>;
  busy: boolean;
  onChange: (features: Record<string, boolean | string>) => void;
}) {
  function change(id: string, value: boolean | string | undefined) {
    // Selecting the provider default removes a value; the empty string remains
    // available for a provider that explicitly declares it.
    if (value === undefined) {
      const next = { ...features };
      delete next[id];
      onChange(next);
    } else {
      onChange({ ...features, [id]: value });
    }
  }

  if (probing) {
    // The tick is drawn while the read runs, and disabled by nothing but the
    // panel's own `busy`. Hiding it was the review's point: `autoAccept` is not
    // one of the rows the read is out to discover — every agent family carries
    // it — so a form that withheld it during an ACP cold start removed a control
    // from a profile that could always have used it, and left the human with
    // nothing to edit until they closed the editor and opened it again.
    return (
      <>
        <div role="status" className="device-field-hint">
          Checking what this provider offers…
        </div>
        <FeatureControl
          feature={AUTO_ACCEPT_FALLBACK_FEATURE}
          value={features[AUTO_ACCEPT_FEATURE]}
          busy={busy}
          onChange={change}
        />
      </>
    );
  }
  // No answer in hand: the tick alone. A stored key the provider was never
  // asked about is **not** drawn — with no declaration there is no control that
  // knows its type, and a form that guessed "toggle" would write a boolean over
  // an agent's choice value, turning a key it cannot read into a value the
  // provider will refuse. It stays in the draft and travels through the save
  // untouched, which is `profileFeaturesFromDraft`'s no-answer branch.
  const rows = offered ?? [AUTO_ACCEPT_FALLBACK_FEATURE];
  if (askedAndFailed) {
    // One plain sentence, and the tick below it. An empty feature section
    // otherwise reads as "this provider has no features", which is a different
    // fact from "it could not be asked", and the one that tells a human to stop
    // looking.
    return (
      <>
        <p className="device-field-hint">
          This provider could not be asked what it offers; it is either not running or it did not
          answer. Nothing stored on this profile was changed.
        </p>
        <FeatureControl
          feature={AUTO_ACCEPT_FALLBACK_FEATURE}
          value={features[AUTO_ACCEPT_FEATURE]}
          busy={busy}
          onChange={change}
        />
      </>
    );
  }
  if (rows.length === 0) {
    return <p className="device-field-hint">This provider offers no features to a profile.</p>;
  }
  return (
    <>
      {rows.map((feature) => (
        <FeatureControl
          key={feature.id}
          feature={feature}
          value={features[feature.id]}
          busy={busy}
          onChange={change}
        />
      ))}
    </>
  );
}
