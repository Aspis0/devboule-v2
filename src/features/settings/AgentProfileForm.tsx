/**
 * The agent-profile form: one component for creating a profile and editing a
 * stored one. The fields' rules live in `AgentProfileDraft.ts` (the draft,
 * the caps, the daemon's own trim and name comparison) and the tool-overlay
 * editor in `AgentProfileOverlay.tsx`; this component owns the fields, the
 * provider-vocabulary lifecycle and what a provider switch clears — which is
 * the provider's own vocabulary: model, mode and thinking option. The feature
 * values are the profile's own and stay across a switch, because the list that
 * decides each one's fate is the answer this fetch is about to receive, not the
 * picker's current value.
 */
import { useEffect, useId, useRef, useState } from "react";
import { useProviderVocabulary } from "./useProviderVocabulary";
import { ErrorText } from "../../components/ErrorText";
import type { ErrorSentence } from "../../lib/errorSentence";
import type { ProviderInfo } from "../../types/ipc";
import {
  ACP_MODE_SUGGESTION,
  ACP_MODE_SUGGESTION_TEXT,
  VOCABULARY_UNAVAILABLE_TEXT,
  VocabularyField,
  vocabularyAxisView,
} from "./AgentProfileVocabulary";
import {
  DEFAULT_IDLE_CLOSE_MINUTES,
  MAX_IDLE_CLOSE_MINUTES,
  MAX_PROFILE_SPAWN_PROMPT_BYTES,
  type ProfileFormSeed,
  offeredFeatures,
  featuresAreProbing,
  featuresAskFailed,
  rustTrim,
  utf8Bytes,
} from "./AgentProfileDraft";
import { AgentProfileFeatureFields } from "./AgentProfileFeatures";
import { AgentProfileOverlayEditor } from "./AgentProfileOverlay";

const MAX_PROFILE_NOTE_BYTES = 2 * 1024;

/**
 * The provider-specific slice of the draft: cleared when the provider
 * changes, restored from the per-provider cache when the human switches
 * back, so a wrong pick is not a loss. Stored features are deliberately
 * absent — they are the profile's saved keys, not the provider's
 * vocabulary, so they travel as saved whatever the picker says.
 */
type ProviderSpecificFields = Pick<ProfileFormSeed, "model" | "modeId" | "thinkingOptionId">;

const CLEARED_PROVIDER_FIELDS: ProviderSpecificFields = {
  model: "",
  modeId: "",
  thinkingOptionId: "",
};

/**
 * The minutes field's text as the draft holds it: blank is the profile
 * saying nothing (the default applies), and anything else is the number the
 * daemon's own cap will judge — `profileDraftRefusal` refuses what this
 * cannot be, rather than the form quietly rounding it. The one value this
 * does correct is a typed **0**: the field's own `min` is 1, and "never" has
 * a door of its own (the tick), so a 0 the human typed must not save as the
 * daemon's `Some(0)` — the opposite of what they asked for.
 *
 * A text that is not a number at all stays `NaN`: it is not a minute count
 * the field can hold, and the refusal has to see it instead of the form
 * inventing a value for words.
 */
function idleMinutesOf(text: string): number | null {
  const trimmed = rustTrim(text);
  if (trimmed === "") return null;
  const minutes = Number(trimmed);
  if (!Number.isFinite(minutes)) return minutes;
  return minutes < 1 ? 1 : minutes;
}

/**
 * The profile form, inline in the Agents panel — the panel's own shape (the
 * editor and the delete confirm are inline too; nothing here needs a modal).
 * One form serves creating a profile and editing a stored one, because the
 * fields are the same and the way to different values should not depend on
 * whether the row exists yet. Its one hard rule: model and modeId are the
 * provider's own vocabulary, stored verbatim, so the form never invents one.
 * It asks — through the `provider_vocabulary` handshake gate — and renders
 * the answer's three states distinctly; when the daemon predates the query
 * it says so in its own words and falls back to free text, so a human can
 * always finish.
 *
 * The vocabulary refetch on a provider change rides the same sequence-guard
 * cadence as the panel's document load: only the newest fetch may apply, AND
 * only a reply naming the provider the form currently shows — a reply for A
 * must not dress B's controls even in the moments before B's own ask leaves.
 *
 * The thinking option is a free-text field: the vocabulary reply carries no
 * thinking axis, and the daemon publishes no list, so typed text is checked
 * against the daemon's caps here and by the provider itself when a session
 * starts.
 */
export function AgentProfileForm({
  mode,
  seed,
  providers,
  catalogLoading,
  catalogError,
  vocabularySupported,
  busy,
  onCreate,
  onSaveSeed,
  onSeedChange,
  onCancel,
}: {
  /** "create" opens with empty fields; "edit" seeds from the stored row. */
  mode: "create" | "edit";
  /** The fields' starting values: EMPTY_PROFILE_FORM_SEED, or the row's draft. */
  seed: ProfileFormSeed;
  /** Installed providers only, catalog order. */
  providers: readonly ProviderInfo[];
  catalogLoading: boolean;
  catalogError: ErrorSentence | null;
  /** True only when the handshake advertised `provider_vocabulary`. */
  vocabularySupported: boolean;
  /** True while a panel write is in flight: Save must not start another. */
  busy: boolean;
  /** Create mode's save. The panel validates and persists. */
  onCreate: (draft: ProfileFormSeed) => void;
  /** Edit mode's save: the seed fields, applied to the one row. */
  onSaveSeed?: (seed: ProfileFormSeed) => void;
  /** Edit mode's every keystroke, reported up so the draft survives the row. */
  onSeedChange?: (seed: ProfileFormSeed) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(seed.name);
  const [icon, setIcon] = useState(seed.icon);
  const [note, setNote] = useState(seed.note);
  const [spawnPrompt, setSpawnPrompt] = useState(seed.spawnPrompt);
  const [providerId, setProviderId] = useState(seed.provider);
  const [model, setModel] = useState(seed.model);
  const [modeId, setModeId] = useState(seed.modeId);
  const [thinkingOptionId, setThinkingOptionId] = useState(seed.thinkingOptionId);
  const [features, setFeatures] = useState<Record<string, boolean | string>>(seed.features);
  const [overlay, setOverlay] = useState(seed.overlay);
  const [enabledForAgents, setEnabledForAgents] = useState(seed.enabledForAgents);
  // The idle-close timer as the two controls hold it: the off tick decides
  // between `0` and the minutes field, and the field shows the default when
  // the profile says nothing (or is off) — 30 is what a saved row without the
  // key means, so it is what an untouched field shows.
  const [idleOff, setIdleOff] = useState(seed.idleCloseMinutes === 0);
  const [idleMinutes, setIdleMinutes] = useState(
    String(
      seed.idleCloseMinutes === null || seed.idleCloseMinutes === 0
        ? DEFAULT_IDLE_CLOSE_MINUTES
        : seed.idleCloseMinutes,
    ),
  );
  // Field-associated text for assistive technology: the ids the described-by
  // wiring points at. One useId per mount, so a create form and an edit form
  // open side by side cannot collide.
  const describedById = useId();
  const spawnHintId = `${describedById}-spawn-hint`;
  const spawnCounterId = `${describedById}-spawn-count`;
  const noteCounterId = `${describedById}-note-count`;
  const thinkingHintId = `${describedById}-thinking-hint`;
  const idleHintId = `${describedById}-idle-hint`;
  // The fields as they stand this render: the base every change reports up
  // from, so the panel's draft always holds the whole form, never a patch.
  function currentSeed(): ProfileFormSeed {
    return {
      name,
      icon,
      note,
      spawnPrompt,
      provider: providerId,
      model,
      modeId,
      thinkingOptionId,
      features,
      offeredFeatures: offeredForSeed,
      overlay,
      enabledForAgents,
      idleCloseMinutes: idleOff ? 0 : idleMinutesOf(idleMinutes),
    };
  }
  // Provider-specific fields, cached by provider while the form is open:
  // switching away clears them (the new provider's vocabulary is its own),
  // and switching back restores what was typed.
  const providerDraftsRef = useRef(new Map<string, ProviderSpecificFields>());

  // The catalog lands after the first paint; default the picker to the first
  // installed provider once there is one, and let the vocabulary effect run.
  useEffect(() => {
    if (!catalogLoading && providerId === "" && providers.length > 0) {
      setProviderId(providers[0].id);
    }
  }, [catalogLoading, providerId, providers]);

  const { vocabularyCurrent, vocabularyAnswered, vocabularyError, vocabularyKnown, settleModel } =
    useProviderVocabulary({
      providerId,
      model,
      providers,
      supported: vocabularySupported,
      // The one prefill a reply may make: an ACP agent that declares no modes
      // runs in "default". Typed text is never clobbered — the hook only calls
      // this, and the form decides what to do with it.
      onAcpWithoutModes: () =>
        setModeId((current) => (current === "" ? ACP_MODE_SUGGESTION : current)),
      onSettledModel: (next) => {
        setModel(next);
        onSeedChange?.({ ...currentSeed(), model: next });
      },
    });

  // One decision per axis, made by the shared reader: every state the wire
  // can reach — present, none, absent, malformed, the contradiction, the
  // unknown — is named there.
  const modelsView = vocabularyAxisView(
    vocabularyCurrent?.models,
    vocabularyKnown,
    vocabularyError !== null && vocabularyCurrent === null,
    "models",
    (item) => ({
      value: item.modelId,
      label:
        item.name && item.name !== item.modelId ? `${item.name} (${item.modelId})` : item.modelId,
    }),
  );
  const modesView = vocabularyAxisView(
    vocabularyCurrent?.modes,
    vocabularyKnown,
    vocabularyError !== null && vocabularyCurrent === null,
    "modes",
    (item) => ({
      value: item.id,
      label: item.name && item.name !== item.id ? `${item.name} (${item.id})` : item.id,
    }),
  );

  // Provider declarations stay visible while a model edit is pending; the
  // current model filters only the model-gated rows.
  const offered = offeredFeatures(vocabularyCurrent, model);
  const probing = vocabularyError === null && featuresAreProbing(vocabularyCurrent);
  const askedAndFailed = vocabularyError !== null || featuresAskFailed(vocabularyCurrent);
  // The list the seed carries to the save. A fresh array each render, so the
  // report below keys on its content and not its identity.
  const offeredForSeed = offered === null ? null : [...offered];
  const offeredKey = offered === null ? "none" : offered.map((feature) => feature.id).join(",");
  const lastOfferedKeyRef = useRef<string | undefined>(undefined);

  // The offered list is part of the draft a save prunes by, and it can change
  // without the human touching a field: a reply lands, the hook's poll replaces a
  // `probing` answer with the real list, or a settled model moves the gate. Every
  // other field reports itself on the change that moved it; these have no change
  // event, so a save reading a stale list would prune by an answer the form had
  // already replaced.
  useEffect(() => {
    const key = `${offeredKey}|${probing ? "probing" : "known"}`;
    if (lastOfferedKeyRef.current === key) {
      return;
    }
    lastOfferedKeyRef.current = key;
    onSeedChange?.(currentSeed());
  });

  const noteBytes = utf8Bytes(note);
  // The counter shows the bytes Save will count — the daemon trims before it
  // caps, so an announcement of raw bytes would refuse a draft Save accepts.
  const spawnBytes = utf8Bytes(rustTrim(spawnPrompt));

  // A stored profile may name a provider that is not installed right now:
  // the row still exists, so the picker must still be able to say so — the
  // stored id is offered as its own option rather than rendering as a blank
  // the human cannot read or keep.
  const providerChoices =
    providerId !== "" && !providers.some((provider) => provider.id === providerId)
      ? [...providers.map((provider) => provider.id), providerId]
      : providers.map((provider) => provider.id);

  // An edit must not render a stored model or mode the provider no longer
  // lists as an empty field: a select over published items alone would hide
  // the very value the human opened the editor to change. The stored value
  // is appended, labelled as the saved one, so it stays visible, stays
  // selected, and is kept by a save that touches nothing else. Create mode
  // has nothing stored, so its list is the answer's own.
  const storedModelOption =
    mode === "edit" && model !== "" && !modelsView.items.some((item) => item.value === model)
      ? [{ value: model, label: `${model} (the value saved on this profile)` }]
      : [];
  const storedModeOption =
    mode === "edit" && modeId !== "" && !modesView.items.some((item) => item.value === modeId)
      ? [{ value: modeId, label: `${modeId} (the value saved on this profile)` }]
      : [];

  function changeProvider(next: string) {
    if (next === providerId) return;
    // The old provider's fields are cached under its id and the new
    // provider's own are restored if the human has been here before:
    // switching must not let one provider's vocabulary survive into
    // another, but a wrong pick followed by switching back must not be a
    // loss either. The feature values and the overlay stay: they belong to the
    // profile, not to the provider's vocabulary, and the new provider's own
    // list — which this switch is about to fetch — decides each value's fate at
    // save, dropping silently the keys that provider does not offer.
    providerDraftsRef.current.set(providerId, { model, modeId, thinkingOptionId });
    const restored = providerDraftsRef.current.get(next) ?? CLEARED_PROVIDER_FIELDS;
    setProviderId(next);
    // A provider switch settles its model in the same breath: the field is not
    // being typed in, the value is the whole point of the switch, and the ask
    // that follows is about that model.
    settleModel(restored.model, false);
    setModeId(restored.modeId);
    setThinkingOptionId(restored.thinkingOptionId);
    onSeedChange?.({ ...currentSeed(), provider: next, ...restored });
  }

  function submit() {
    if (mode === "create") {
      onCreate(currentSeed());
    } else {
      onSaveSeed?.(currentSeed());
    }
  }

  return (
    <div
      className={
        mode === "create" ? "agent-inline-editor agent-profile-create" : "agent-inline-editor"
      }
    >
      <span className="settings-subheading">
        {mode === "create" ? "New profile" : "Edit profile"}
      </span>
      <label className="device-field">
        Name
        <input
          aria-label="Profile name"
          value={name}
          disabled={busy}
          onChange={(event) => {
            setName(event.target.value);
            onSeedChange?.({ ...currentSeed(), name: event.target.value });
          }}
        />
      </label>
      <label className="device-field">
        Icon
        <input
          aria-label="Profile icon"
          value={icon}
          disabled={busy}
          onChange={(event) => {
            setIcon(event.target.value);
            onSeedChange?.({ ...currentSeed(), icon: event.target.value });
          }}
        />
      </label>
      <label className="device-field">
        Note — what a creating agent reads to choose this profile. Write it for the agent.
        <textarea
          aria-label="Profile note"
          value={note}
          disabled={busy}
          rows={3}
          onChange={(event) => {
            setNote(event.target.value);
            onSeedChange?.({ ...currentSeed(), note: event.target.value });
          }}
        />
        <span className="agent-byte-counter" id={noteCounterId} aria-live="polite">
          {noteBytes} / {MAX_PROFILE_NOTE_BYTES} bytes
        </span>
      </label>
      <label className="device-field">
        Spawn prompt — the profile's own instructions for its children
        <textarea
          aria-label="Profile spawn prompt"
          aria-describedby={`${spawnHintId} ${spawnCounterId}`}
          value={spawnPrompt}
          disabled={busy}
          rows={3}
          onChange={(event) => {
            setSpawnPrompt(event.target.value);
            onSeedChange?.({ ...currentSeed(), spawnPrompt: event.target.value });
          }}
        />
        <span className="agent-byte-counter" id={spawnCounterId} aria-live="polite">
          {spawnBytes} / {MAX_PROFILE_SPAWN_PROMPT_BYTES} bytes
        </span>
        <span className="device-field-hint" id={spawnHintId}>
          Sent at the start of every agent created from this profile, before the creator's prompt.
          Keep it short. An agent created from this profile also receives the standing instructions
          and the task written by the agent that creates it — write only what is specific to this
          kind of agent.
        </span>
      </label>
      <label className="device-field">
        Provider
        <select
          aria-label="Provider"
          value={providerId}
          disabled={busy || catalogLoading || providers.length === 0}
          onChange={(event) => changeProvider(event.target.value)}
        >
          {catalogLoading ? <option value="">Looking for installed providers…</option> : null}
          {!catalogLoading && catalogError !== null ? (
            <option value="">The catalog could not be read</option>
          ) : null}
          {!catalogLoading && catalogError === null && providers.length === 0 ? (
            <option value="">No provider installed</option>
          ) : null}
          {providerChoices.map((id) => (
            <option key={id} value={id}>
              {id}
            </option>
          ))}
        </select>
      </label>
      {catalogError !== null ? (
        <p className="device-field-hint" role="alert">
          <ErrorText
            sentence={`The provider catalog could not be read: ${catalogError.sentence}`}
            detail={catalogError.detail}
            id="settings-profile-catalog-error"
          />
        </p>
      ) : null}
      {/* A completed read that found nothing is the only state allowed to
          claim "no agent CLI": not-read is not empty, and a failed read is
          its own fact — the paragraph above names it. */}
      {!catalogLoading && catalogError === null && providers.length === 0 ? (
        <p className="device-field-hint">
          No agent CLI is installed on this machine: install one and restart Devboule, then create
          the profile.
        </p>
      ) : null}
      {!vocabularySupported ? (
        <p className="device-field-hint">{VOCABULARY_UNAVAILABLE_TEXT}</p>
      ) : null}
      {vocabularySupported && providerId !== "" && !vocabularyKnown ? (
        <div role="status">Asking the daemon what {providerId} offers…</div>
      ) : null}
      {vocabularyError !== null ? (
        <p className="device-field-hint">
          <ErrorText
            sentence={`The vocabulary query failed (${vocabularyError.sentence}); type the model and mode below; what you type is checked when the session starts.`}
            detail={vocabularyError.detail}
            id="settings-vocabulary-error"
          />
        </p>
      ) : null}
      {(!vocabularySupported || vocabularyKnown) && providers.length > 0 ? (
        <>
          <VocabularyField
            label="Model"
            value={model}
            busy={busy}
            freeText={modelsView.freeText}
            hint={modelsView.hint}
            items={[...modelsView.items, ...storedModelOption]}
            onChange={(next) => {
              setModel(next);
              onSeedChange?.({ ...currentSeed(), model: next });
            }}
            onSettle={settleModel}
          />
          <VocabularyField
            label="Mode"
            value={modeId}
            busy={busy}
            freeText={modesView.freeText}
            hint={modesView.hint}
            suggestion={
              vocabularyAnswered !== null &&
              vocabularyCurrent?.modes?.state === "absent" &&
              providers.find((provider) => provider.id === providerId)?.protocol === "acp"
                ? ACP_MODE_SUGGESTION_TEXT
                : undefined
            }
            items={[...modesView.items, ...storedModeOption]}
            onChange={(next) => {
              setModeId(next);
              onSeedChange?.({ ...currentSeed(), modeId: next });
            }}
          />
          <label className="device-field">
            Thinking option
            <input
              aria-label="Thinking option"
              aria-describedby={thinkingHintId}
              value={thinkingOptionId}
              disabled={busy}
              onChange={(event) => {
                setThinkingOptionId(event.target.value);
                onSeedChange?.({ ...currentSeed(), thinkingOptionId: event.target.value });
              }}
            />
            <span className="device-field-hint" id={thinkingHintId}>
              The daemon keeps no list of thinking options: type the id the provider uses, or leave
              this empty for none.
            </span>
          </label>
        </>
      ) : null}
      {/* One control per feature this provider offers, read from the reply the
          effect above fetched and never from a list written here. It sits
          outside the `vocabularyKnown` block on purpose: with no answer in hand
          the component still draws the daemon's own tick and says why, and the
          model and mode fields above handle their half of that silence. */}
      <AgentProfileFeatureFields
        offered={offered}
        probing={probing}
        askedAndFailed={askedAndFailed}
        features={features}
        busy={busy}
        onChange={(next) => {
          setFeatures(next);
          onSeedChange?.({ ...currentSeed(), features: next });
        }}
      />
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          aria-label="Available to agents"
          checked={enabledForAgents}
          disabled={busy}
          onChange={(event) => {
            setEnabledForAgents(event.target.checked);
            onSeedChange?.({ ...currentSeed(), enabledForAgents: event.target.checked });
          }}
        />
        <span>
          <span>Agents may create this</span>
          <span className="agent-profile-tick-note">
            Lets an agent start this kind of agent. If this profile answers its own permission
            cards, its children run unattended.
          </span>
        </span>
      </label>
      <label className="device-field">
        Close idle children after — minutes
        <input
          aria-label="Close idle children after minutes"
          aria-describedby={idleHintId}
          type="number"
          min={1}
          max={MAX_IDLE_CLOSE_MINUTES}
          step={1}
          value={idleMinutes}
          disabled={busy || idleOff}
          onChange={(event) => {
            const raw = event.target.value;
            const minutes = idleMinutesOf(raw);
            // The corrected value is what the field shows, so a typed 0
            // reads as the 1 it will be saved as — never as silence. Text
            // that parses to no number keeps its own characters: the save
            // refuses it, and the refusal must name what the human sees.
            setIdleMinutes(minutes !== null && Number.isFinite(minutes) ? String(minutes) : raw);
            onSeedChange?.({
              ...currentSeed(),
              idleCloseMinutes: idleOff ? 0 : minutes,
            });
          }}
        />
        <span className="device-field-hint" id={idleHintId}>
          {DEFAULT_IDLE_CLOSE_MINUTES} by default, up to {MAX_IDLE_CLOSE_MINUTES} (a week). A child
          created from this profile is closed after this long with no turn, no waiting permission
          card, nothing being sent to it, and nobody looking at it; a closed session cannot reopen.
          Only children an agent created are ever closed this way.
        </span>
      </label>
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          aria-label="Never close idle children"
          checked={idleOff}
          disabled={busy}
          onChange={(event) => {
            const off = event.target.checked;
            setIdleOff(off);
            onSeedChange?.({
              ...currentSeed(),
              idleCloseMinutes: off ? 0 : idleMinutesOf(idleMinutes),
            });
          }}
        />
        <span>
          <span>Never close idle children</span>
          <span className="agent-profile-tick-note">
            Keep them running until you close them yourself; the minutes above stop applying.
          </span>
        </span>
      </label>
      <AgentProfileOverlayEditor
        overlay={overlay}
        busy={busy}
        onChange={(next) => {
          setOverlay(next);
          onSeedChange?.({ ...currentSeed(), overlay: next });
        }}
      />
      {mode === "edit" ? (
        <p className="device-field-hint">
          Changes apply to agents created from now on. Agents already running keep what they started
          with.
        </p>
      ) : null}
      <div className="device-actions">
        <button
          type="button"
          className="settings-device-action"
          disabled={busy || (mode === "create" && (catalogLoading || providers.length === 0))}
          onClick={submit}
        >
          {mode === "create" ? "Create profile" : "Save"}
        </button>
        <button type="button" className="settings-device-action" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
