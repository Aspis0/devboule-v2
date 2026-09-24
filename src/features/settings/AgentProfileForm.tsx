/**
 * The agent-profile form: one component for creating a profile and editing a
 * stored one, with the draft both modes share, the text caps mirrored from
 * the daemon's own constants, and the provider-vocabulary reader that decides
 * which control each axis gets. The Agents panel (`SettingsSurface.tsx`) owns
 * the document, its persistence and the rows; everything that is only about
 * authoring one profile lives here.
 */
import { useEffect, useId, useRef, useState } from "react";
import { providerVocabularyGet } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import { PEER_TOOLS } from "./profileOverlay";
import type { AgentProfile, ProviderInfo, ProviderVocabulary } from "../../types/ipc";
import {
  ACP_MODE_SUGGESTION,
  ACP_MODE_SUGGESTION_TEXT,
  VOCABULARY_UNAVAILABLE_TEXT,
  VocabularyField,
  vocabularyAxisView,
} from "./AgentProfileVocabulary";

/** The profile store's caps, the daemon's own constants mirrored. */
const MAX_PROFILE_NAME_CHARS = 60;
const MAX_PROFILE_NOTE_BYTES = 2 * 1024;
const MAX_PROFILE_ICON_BYTES = 64;
/** `MAX_PROFILE_SPAWN_PROMPT_BYTES` in `agent_profiles.rs`. */
const MAX_PROFILE_SPAWN_PROMPT_BYTES = 8 * 1024;
/** `MAX_PROFILE_FIELD_BYTES` in `agent_profiles.rs`: ids like the thinking option. */
const MAX_PROFILE_FIELD_BYTES = 128;
const MAX_FEATURE_KEY_BYTES = 64;
const MAX_FEATURE_VALUE_BYTES = 1024;
const MAX_TOOL_OVERLAY_NAME_BYTES = 128;
/** `AUTO_ACCEPT_FEATURE` in `provider_catalog.rs`: the one feature the daemon interprets. */
const AUTO_ACCEPT_FEATURE = "autoAccept";

/**
 * Every cap a profile write must fit, checked on the draft before the panel
 * sends anything: the daemon applies exactly these shapes in
 * `agent_profiles.rs`, and the spawn prompt is capped **after** trimming
 * there (`check_profile`), so the same trimmed bytes are counted here — a
 * leading space must not refuse a prompt the daemon would store. Refusals
 * are sized in the daemon's own units and nothing is ever truncated.
 */
export function profileDraftRefusal(draft: ProfileFormSeed): string | null {
  const trimmedChars = charCount(draft.name.trim());
  if (trimmedChars === 0) {
    return `A profile name is 1 to ${MAX_PROFILE_NAME_CHARS} characters.`;
  }
  if (trimmedChars > MAX_PROFILE_NAME_CHARS) {
    return `This name is ${trimmedChars} characters, over the ${MAX_PROFILE_NAME_CHARS}-character cap. Nothing was saved and nothing was truncated.`;
  }
  const noteBytes = utf8Bytes(draft.note);
  if (noteBytes > MAX_PROFILE_NOTE_BYTES) {
    return `This note is ${noteBytes} bytes, over the ${MAX_PROFILE_NOTE_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  const spawnBytes = utf8Bytes(draft.spawnPrompt.trim());
  if (spawnBytes > MAX_PROFILE_SPAWN_PROMPT_BYTES) {
    return `This spawn prompt is ${spawnBytes} bytes, over the ${MAX_PROFILE_SPAWN_PROMPT_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  const model = draft.model.trim();
  if (model === "") {
    return "Choose or type a model for the profile.";
  }
  const modeId = draft.modeId.trim();
  if (modeId === "") {
    return "Choose or type a mode for the profile.";
  }
  const thinkingBytes = utf8Bytes(draft.thinkingOptionId.trim());
  if (thinkingBytes > MAX_PROFILE_FIELD_BYTES) {
    return `The thinking option id is ${thinkingBytes} bytes, over the ${MAX_PROFILE_FIELD_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  const iconBytes = utf8Bytes(draft.icon.trim());
  if (iconBytes > MAX_PROFILE_ICON_BYTES) {
    return `The icon is ${iconBytes} bytes, over the ${MAX_PROFILE_ICON_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  for (const tool of draft.overlayExtras) {
    const toolBytes = utf8Bytes(tool.trim());
    if (toolBytes > MAX_TOOL_OVERLAY_NAME_BYTES) {
      return `A denied tool name is ${toolBytes} bytes, over the ${MAX_TOOL_OVERLAY_NAME_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
    }
  }
  for (const feature of draft.extraFeatures) {
    const keyBytes = utf8Bytes(feature.key.trim());
    if (keyBytes === 0) {
      return "A feature needs a key: type the name the provider uses, or remove the row.";
    }
    if (keyBytes > MAX_FEATURE_KEY_BYTES) {
      return `A feature key is ${keyBytes} bytes, over the ${MAX_FEATURE_KEY_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
    }
    const parsed = parseFeatureValue(feature.valueText);
    if (parsed.error !== null) {
      return `The value of feature '${feature.key.trim()}' is not valid JSON; wrap plain text in double quotes, or type a number, true or false.`;
    }
    const valueBytes = utf8Bytes(JSON.stringify(parsed.value));
    if (valueBytes > MAX_FEATURE_VALUE_BYTES) {
      return `The value of feature '${feature.key.trim()}' serializes to ${valueBytes} bytes, over the ${MAX_FEATURE_VALUE_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
    }
  }
  return null;
}

/** The feature value a value text carries: typed JSON, or a plain string. */
function parseFeatureValue(valueText: string): { value: unknown; error: string | null } {
  try {
    return { value: JSON.parse(valueText), error: null };
  } catch {
    return { value: undefined, error: "invalid JSON" };
  }
}

/**
 * The stored `features` map the draft saves: the tick the daemon interprets,
 * plus every other row as it parsed — unknown keys are words this form
 * carries for the provider, not noise it may drop or reshape.
 */
export function profileFeaturesFromDraft(draft: ProfileFormSeed): Record<string, unknown> {
  const features: Record<string, unknown> = {};
  if (draft.autoAccept) {
    features[AUTO_ACCEPT_FEATURE] = true;
  }
  for (const feature of draft.extraFeatures) {
    features[feature.key.trim()] = parseFeatureValue(feature.valueText).value;
  }
  return features;
}

/**
 * The stored `toolOverlay` the draft saves: the peer-restriction pair in the
 * broker's own order, then the kept stored names beyond it.
 */
export function toolOverlayFromDraft(draft: ProfileFormSeed): string[] {
  const pair = draft.restrictPeers ? [...PEER_TOOLS] : [];
  const extras = draft.overlayExtras
    .map((tool) => tool.trim())
    .filter((tool) => tool !== "" && !pair.includes(tool));
  return [...pair, ...extras];
}

/**
 * What the profile form's fields hold — in both modes. It is the seed the
 * form starts from, the draft an edit reports up, and what a save applies:
 * create and edit name the same whole profile, the way Paseo's one form
 * does. The feature rows beyond the daemon's own tick and the stored tool
 * denials beyond the peer pair travel as their saved values, editable and
 * removable — never silently dropped.
 */
export interface ProfileFormSeed {
  name: string;
  /** "" saves none; the daemon stores null. */
  icon: string;
  note: string;
  spawnPrompt: string;
  provider: string;
  model: string;
  modeId: string;
  /** "" means none; the daemon stores null. */
  thinkingOptionId: string;
  /** The one feature the daemon interprets, read the way the daemon reads it. */
  autoAccept: boolean;
  /** Every stored feature beyond the tick, as editable JSON text. */
  extraFeatures: { key: string; valueText: string }[];
  enabledForAgents: boolean;
  /** The peer-restriction pair, ticked when the saved overlay is exactly it. */
  restrictPeers: boolean;
  /** Stored tool denials beyond the peer pair. */
  overlayExtras: string[];
}

/** The seed a new profile starts from: empty fields, no provider chosen yet. */
export const EMPTY_PROFILE_FORM_SEED: ProfileFormSeed = {
  name: "",
  icon: "",
  note: "",
  spawnPrompt: "",
  provider: "",
  model: "",
  modeId: "",
  thinkingOptionId: "",
  autoAccept: false,
  extraFeatures: [],
  enabledForAgents: false,
  restrictPeers: false,
  overlayExtras: [],
};

/**
 * The stored row a form edits, as the fields hold it. `autoAccept` is the
 * JSON boolean `true` and nothing else — the daemon's own reading
 * (`profile_delivery.rs`) — and the tick on the peer pair matches only the
 * exact pair: any other stored overlay keeps its names as removable rows.
 * Feature values travel as their JSON text, so what the human edits is what
 * the daemon stored.
 */
export function seedFromProfile(profile: AgentProfile): ProfileFormSeed {
  const overlay = profile.toolOverlay ?? [];
  const isPeerPair =
    overlay.length === PEER_TOOLS.length && PEER_TOOLS.every((tool) => overlay.includes(tool));
  return {
    name: profile.name,
    icon: profile.icon ?? "",
    note: profile.note,
    spawnPrompt: profile.spawnPrompt ?? "",
    provider: profile.provider,
    model: profile.model,
    modeId: profile.modeId,
    thinkingOptionId: profile.thinkingOptionId ?? "",
    autoAccept: profile.features.autoAccept === true,
    extraFeatures: Object.entries(profile.features)
      .filter(([key]) => key !== AUTO_ACCEPT_FEATURE)
      .map(([key, value]) => ({ key, valueText: JSON.stringify(value) })),
    enabledForAgents: profile.enabledForAgents,
    restrictPeers: isPeerPair,
    overlayExtras: overlay.filter((tool) => !PEER_TOOLS.includes(tool)),
  };
}

/** The daemon counts UTF-8 bytes (`String::len`), so the on-screen counter must too. */
export function utf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/**
 * The daemon counts a profile name in Unicode scalar values
 * (`str::chars().count()`), so the cap must count the same unit: iteration
 * yields whole code points, and one astral-plane character (emoji, CJK
 * extensions) is one — where UTF-16 code-unit counting would call it two and
 * refuse names the daemon accepts.
 */
function charCount(text: string): number {
  return [...text].length;
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
 * cadence as the panel's document load: only the newest fetch may apply, so
 * a reply for the previously selected provider never lands in a form that
 * now shows another one.
 *
 * Both modes edit the same whole profile — ticks, icon, overlay and the
 * feature map included — because the way to different values should not
 * depend on whether the row exists yet. Provider-specific fields (the model,
 * the mode, the thinking option, the stored features beyond the daemon's own
 * tick) reset when the provider changes; `autoAccept` and the peer tools
 * overlay stay, because they are not the provider's vocabulary. The thinking
 * option is a free-text field: the vocabulary reply carries no thinking
 * axis, and the daemon publishes no list, so typed text is checked against
 * the daemon's caps here and by the provider itself when a session starts.
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
  const [autoAccept, setAutoAccept] = useState(seed.autoAccept);
  const [extraFeatures, setExtraFeatures] = useState(seed.extraFeatures);
  const [featureKey, setFeatureKey] = useState("");
  const [featureValue, setFeatureValue] = useState("");
  const [overlayTool, setOverlayTool] = useState("");
  const [enabledForAgents, setEnabledForAgents] = useState(seed.enabledForAgents);
  const [restrictPeers, setRestrictPeers] = useState(seed.restrictPeers);
  const [overlayExtras, setOverlayExtras] = useState(seed.overlayExtras);
  const [vocabulary, setVocabulary] = useState<ProviderVocabulary | null>(null);
  const [vocabularyError, setVocabularyError] = useState<ErrorSentence | null>(null);
  // Field-associated text for assistive technology: the ids the described-by
  // wiring points at. One useId per mount, so a create form and an edit form
  // open side by side cannot collide.
  const describedById = useId();
  const spawnHintId = `${describedById}-spawn-hint`;
  const spawnCounterId = `${describedById}-spawn-count`;
  const noteCounterId = `${describedById}-note-count`;
  const thinkingHintId = `${describedById}-thinking-hint`;
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
      autoAccept,
      extraFeatures,
      enabledForAgents,
      restrictPeers,
      overlayExtras,
    };
  }
  // Monotonic fetch sequence for the vocabulary query: a reply may apply
  // only while it is still the newest fetch. This — never the provider id
  // echoed back — is what keeps a slow answer for provider A out of a form
  // now showing provider B, so the guard is the single mechanism the
  // stale-reply test mutates.
  const vocabularySeqRef = useRef(0);

  // The catalog lands after the first paint; default the picker to the first
  // installed provider once there is one, and let the vocabulary effect run.
  useEffect(() => {
    if (!catalogLoading && providerId === "" && providers.length > 0) {
      setProviderId(providers[0].id);
    }
  }, [catalogLoading, providerId, providers]);

  useEffect(() => {
    // A daemon that never advertised `provider_vocabulary` would refuse this
    // request: it is never sent. The form's free-text fallback and the
    // older-daemon sentence are the whole UI for that case.
    if (!vocabularySupported || providerId === "") return;
    // No `cancelled` flag beside the sequence guard on purpose: every path
    // that could make a reply stale (the provider changed, the form closed)
    // bumps the sequence, so the guard below is the one mechanism — and the
    // one thing the stale-reply test mutates. A `setState` after unmount is
    // a safe no-op in React 18+.
    const seq = ++vocabularySeqRef.current;
    setVocabulary(null);
    setVocabularyError(null);
    void providerVocabularyGet(providerId, false)
      .then((reply) => {
        // A newer fetch (the provider changed again) owns the form: this
        // reply is stale no matter which provider it names.
        if (vocabularySeqRef.current !== seq) return;
        // The reply is adopted as it arrived; the per-axis reader below
        // treats it as the untrusted wire value it is. A reply missing an
        // axis is malformed — its own state, named in the render — and must
        // not throw: throwing here would report a successful query as
        // failed and discard the axis that arrived intact.
        setVocabulary(reply);
        // The one prefill allowed: an ACP agent that declared no modes runs
        // in "default". Typed text is never clobbered — the suggestion only
        // fills an empty field, and it is labelled a suggestion.
        if (reply.modes?.state === "absent") {
          const info = providers.find((provider) => provider.id === providerId);
          if (info?.protocol === "acp") {
            setModeId((current) => (current === "" ? ACP_MODE_SUGGESTION : current));
          }
        }
      })
      .catch((cause: unknown) => {
        if (vocabularySeqRef.current !== seq) return;
        setVocabularyError(errorSentence(cause));
      });
  }, [providerId, vocabularySupported, providers]);

  // The reply (or its failure) is what the fields read; before either, the
  // form shows the ask in flight and renders no field to guess into.
  const vocabularyKnown = vocabulary !== null || vocabularyError !== null;
  // One decision per axis, made by the shared reader: every state the wire
  // can reach — present, none, absent, malformed, the contradiction, the
  // unknown — is named there.
  const modelsView = vocabularyAxisView(
    vocabulary?.models,
    vocabulary !== null,
    vocabularyError !== null,
    "models",
    (item) => ({
      value: item.modelId,
      label:
        item.name && item.name !== item.modelId ? `${item.name} (${item.modelId})` : item.modelId,
    }),
  );
  const modesView = vocabularyAxisView(
    vocabulary?.modes,
    vocabulary !== null,
    vocabularyError !== null,
    "modes",
    (item) => ({
      value: item.id,
      label: item.name && item.name !== item.id ? `${item.name} (${item.id})` : item.id,
    }),
  );
  const noteBytes = utf8Bytes(note);
  const spawnBytes = utf8Bytes(spawnPrompt);

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
    // Reset everything that belongs to the old provider before the fetch
    // starts: its selection must not survive into the new one — model, mode,
    // thinking option and the stored features beyond the daemon's own tick.
    // `autoAccept` and the peer tools overlay stay: they are not the
    // provider's vocabulary.
    setProviderId(next);
    setModel("");
    setModeId("");
    setThinkingOptionId("");
    setExtraFeatures([]);
    onSeedChange?.({
      ...currentSeed(),
      provider: next,
      model: "",
      modeId: "",
      thinkingOptionId: "",
      extraFeatures: [],
    });
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
          />
          <VocabularyField
            label="Mode"
            value={modeId}
            busy={busy}
            freeText={modesView.freeText}
            hint={modesView.hint}
            suggestion={
              vocabulary?.modes?.state === "absent" &&
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
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          aria-label="Auto accept for children of this profile"
          checked={autoAccept}
          disabled={busy}
          onChange={(event) => {
            setAutoAccept(event.target.checked);
            onSeedChange?.({ ...currentSeed(), autoAccept: event.target.checked });
          }}
        />
        <span>
          <span>Auto accept</span>
          <span className="agent-profile-tick-note">
            Children created from this profile approve their own permission prompts instead of
            asking you. This is the most consequential control on the form: leave it off unless you
            mean it.
          </span>
        </span>
      </label>
      <div className="device-field">
        <span className="settings-subheading">Other stored features</span>
        {extraFeatures.length === 0 ? (
          <p className="device-field-hint">
            No other features are stored on this profile. The daemon passes feature keys it does not
            know to the provider unchanged.
          </p>
        ) : (
          extraFeatures.map((feature, index) => (
            <div className="device-inline-confirm" key={feature.key}>
              <label className="device-field">
                Feature key
                <input
                  aria-label={`Feature key ${index + 1}`}
                  value={feature.key}
                  disabled={busy}
                  onChange={(event) => {
                    const key = event.target.value;
                    setExtraFeatures((rows) =>
                      rows.map((row, at) => (at === index ? { ...row, key } : row)),
                    );
                  }}
                />
              </label>
              <label className="device-field">
                Feature value (JSON)
                <input
                  aria-label={`Feature value ${index + 1}`}
                  value={feature.valueText}
                  disabled={busy}
                  onChange={(event) => {
                    const valueText = event.target.value;
                    setExtraFeatures((rows) =>
                      rows.map((row, at) => (at === index ? { ...row, valueText } : row)),
                    );
                  }}
                />
              </label>
              <button
                type="button"
                className="settings-device-action"
                disabled={busy}
                aria-label={`Remove feature ${feature.key}`}
                onClick={() => {
                  setExtraFeatures((rows) => rows.filter((_, at) => at !== index));
                }}
              >
                Remove feature
              </button>
            </div>
          ))
        )}
        <div className="agent-profile-create-row">
          <label className="device-field">
            Add a feature — key
            <input
              aria-label="New feature key"
              value={featureKey}
              disabled={busy}
              onChange={(event) => setFeatureKey(event.target.value)}
            />
          </label>
          <label className="device-field">
            Add a feature — value (JSON)
            <input
              aria-label="New feature value"
              value={featureValue}
              disabled={busy}
              onChange={(event) => setFeatureValue(event.target.value)}
            />
          </label>
          <button
            type="button"
            className="settings-device-action"
            disabled={busy || featureKey.trim() === ""}
            onClick={() => {
              const key = featureKey.trim();
              if (key === "" || extraFeatures.some((row) => row.key === key)) return;
              setExtraFeatures((rows) => [...rows, { key, valueText: featureValue }]);
              setFeatureKey("");
              setFeatureValue("");
            }}
          >
            Add feature
          </button>
        </div>
      </div>
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
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          aria-label="Children cannot message peers or create further agents"
          checked={restrictPeers}
          disabled={busy}
          onChange={(event) => {
            setRestrictPeers(event.target.checked);
            onSeedChange?.({ ...currentSeed(), restrictPeers: event.target.checked });
          }}
        />
        <span>
          <span>No peer contact and no further agents for children</span>
          <span className="agent-profile-tick-note">
            Children created from this profile cannot message other agents or create further agents.
            They keep the agent roster, their read-only view.
          </span>
        </span>
      </label>
      {overlayExtras.length > 0 ? (
        <div className="device-field">
          <span className="settings-subheading">Other stored tool denials</span>
          {overlayExtras.map((tool) => (
            <div className="agent-profile-create-row" key={tool}>
              <span className="device-copy">{tool}</span>
              <button
                type="button"
                className="settings-device-action"
                disabled={busy}
                aria-label={`Remove denied tool ${tool}`}
                onClick={() => {
                  setOverlayExtras((rows) => rows.filter((kept) => kept !== tool));
                  onSeedChange?.({
                    ...currentSeed(),
                    overlayExtras: overlayExtras.filter((kept) => kept !== tool),
                  });
                }}
              >
                Remove denial
              </button>
            </div>
          ))}
        </div>
      ) : null}
      <div className="agent-profile-create-row">
        <label className="device-field">
          Deny a tool by name
          <input
            aria-label="New denied tool name"
            value={overlayTool}
            disabled={busy}
            onChange={(event) => setOverlayTool(event.target.value)}
          />
        </label>
        <button
          type="button"
          className="settings-device-action"
          disabled={busy || overlayTool.trim() === ""}
          onClick={() => {
            const tool = overlayTool.trim();
            if (tool === "" || PEER_TOOLS.includes(tool) || overlayExtras.includes(tool)) return;
            setOverlayExtras((rows) => [...rows, tool]);
            onSeedChange?.({ ...currentSeed(), overlayExtras: [...overlayExtras, tool] });
            setOverlayTool("");
          }}
        >
          Add denial
        </button>
      </div>
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
