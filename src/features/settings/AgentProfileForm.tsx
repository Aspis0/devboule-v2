/**
 * The agent-profile form: one component for creating a profile and editing a
 * stored one. The main form asks only what it must — name, provider, model,
 * effort, mode, note, instructions — every enumerable as a picker fed by the
 * daemon's vocabulary, never free text. Everything else lives under one
 * collapsed Advanced section. The fields' rules live in
 * `AgentProfileDraft.ts`; the panel owns the document.
 */
import { useEffect, useId, useRef, useState } from "react";
import { useProviderVocabulary } from "./useProviderVocabulary";
import { ErrorText } from "../../components/ErrorText";
import type { ErrorSentence } from "../../lib/errorSentence";
import type { ProviderInfo, SessionModel } from "../../types/ipc";
import { ByteCounter } from "./rows";
import {
  ACP_MODE_SUGGESTION,
  ACP_MODE_SUGGESTION_TEXT,
  PI_MODELS_READING_TEXT,
  PI_MODELS_UNREADABLE_TEXT,
  VOCABULARY_UNAVAILABLE_TEXT,
  VocabularyField,
  vocabularyAxisView,
} from "./AgentProfileVocabulary";
import {
  DEFAULT_IDLE_CLOSE_MINUTES,
  MAX_IDLE_CLOSE_MINUTES,
  MAX_PROFILE_SPAWN_PROMPT_BYTES,
  type ProfileFormSeed,
  matchModelItem,
  modelOptionLabel,
  modelRefOf,
  offeredFeatures,
  featuresAreProbing,
  featuresAskFailed,
  parseModelRef,
  rustTrim,
  utf8Bytes,
} from "./AgentProfileDraft";
import { AgentProfileFeatureFields } from "./AgentProfileFeatures";
import { AgentProfileOverlayEditor } from "./AgentProfileOverlay";

const MAX_PROFILE_NOTE_BYTES = 2 * 1024;

/**
 * The provider-specific slice of the draft: cleared when the provider
 * changes, restored from the per-provider cache when the human switches
 * back. Stored features are absent — they are the profile's saved keys, not
 * the provider's vocabulary, so they travel as saved whatever the picker
 * says.
 */
type ProviderSpecificFields = Pick<
  ProfileFormSeed,
  "model" | "modelProvider" | "modeId" | "thinkingOptionId"
>;

const CLEARED_PROVIDER_FIELDS: ProviderSpecificFields = {
  model: "",
  modelProvider: null,
  modeId: "",
  thinkingOptionId: "",
};

function idleMinutesOf(text: string): number | null {
  const trimmed = rustTrim(text);
  if (trimmed === "") return null;
  const minutes = Number(trimmed);
  if (!Number.isFinite(minutes)) return minutes;
  return minutes < 1 ? 1 : minutes;
}

/** Whether the stored effort is one the matched model still offers. */
function effortKept(item: SessionModel | undefined, effort: string): boolean {
  if (effort === "") return true;
  const levels = item?.efforts ?? null;
  if (levels === null) return true;
  return levels.some((level) => level.id === effort);
}

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
  onDirty,
  hideHeading = false,
  formError = null,
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
  /**
   * One call per human edit — never for the form's own reports. The
   * dialog's dirty check reads only this, so auto-defaults never arm it.
   */
  onDirty?: () => void;
  /**
   * True inside the dialog: the shell renders the title, so the form skips
   * its own heading rather than naming the profile twice.
   */
  hideHeading?: boolean;
  /**
   * The panel's error, if the failed write belongs to this dialog: refusal
   * sentences render here, above the buttons, instead of behind the scrim.
   */
  formError?: ErrorSentence | null;
  onCancel: () => void;
}) {
  const [name, setName] = useState(seed.name);
  const [icon, setIcon] = useState(seed.icon);
  const [note, setNote] = useState(seed.note);
  const [spawnPrompt, setSpawnPrompt] = useState(seed.spawnPrompt);
  const [providerId, setProviderId] = useState(seed.provider);
  const [model, setModel] = useState(seed.model);
  const [modelProvider, setModelProvider] = useState<string | null>(seed.modelProvider);
  const [modeId, setModeId] = useState(seed.modeId);
  const [thinkingOptionId, setThinkingOptionId] = useState(seed.thinkingOptionId);
  const [features, setFeatures] = useState<Record<string, boolean | string>>(seed.features);
  const [overlay, setOverlay] = useState(seed.overlay);
  const [enabledForAgents, setEnabledForAgents] = useState(seed.enabledForAgents);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [idleOff, setIdleOff] = useState(seed.idleCloseMinutes === 0);
  const [idleMinutes, setIdleMinutes] = useState(
    String(
      seed.idleCloseMinutes === null || seed.idleCloseMinutes === 0
        ? DEFAULT_IDLE_CLOSE_MINUTES
        : seed.idleCloseMinutes,
    ),
  );
  const describedById = useId();
  const iconHintId = `${describedById}-icon-hint`;
  const thinkingHintId = `${describedById}-thinking-hint`;
  const idleHintId = `${describedById}-idle-hint`;
  function currentSeed(): ProfileFormSeed {
    return {
      name,
      icon,
      note,
      spawnPrompt,
      provider: providerId,
      model,
      modelProvider,
      modeId,
      thinkingOptionId,
      features,
      offeredFeatures: offeredForSeed,
      overlay,
      enabledForAgents,
      idleCloseMinutes: idleOff ? 0 : idleMinutesOf(idleMinutes),
    };
  }
  function userChanged(patch: Partial<ProfileFormSeed>) {
    onSeedChange?.({ ...currentSeed(), ...patch });
    onDirty?.();
  }
  const providerDraftsRef = useRef(new Map<string, ProviderSpecificFields>());
  const formErrorRef = useRef<HTMLParagraphElement>(null);
  const scrolledErrorRef = useRef<string | null>(null);

  useEffect(() => {
    if (formError === null) {
      scrolledErrorRef.current = null;
      return;
    }
    const key = `${formError.sentence}\n${formError.detail ?? ""}`;
    if (scrolledErrorRef.current === key) return;
    scrolledErrorRef.current = key;
    formErrorRef.current?.scrollIntoView?.({ block: "nearest" });
  });

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
      onAcpWithoutModes: () =>
        setModeId((current) => (current === "" ? ACP_MODE_SUGGESTION : current)),
      onSettledModel: (next) => {
        setModel(next);
        onSeedChange?.({ ...currentSeed(), model: next });
      },
    });

  const modelsView = vocabularyAxisView(
    vocabularyCurrent?.models,
    vocabularyKnown,
    vocabularyError !== null && vocabularyCurrent === null,
    "models",
    (item) => ({ value: modelRefOf(item), label: modelOptionLabel(item) }),
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

  // The raw model items beside the mapped options: the effort picker and the
  // serving-provider pair read here, not from option strings.
  const modelItems: readonly SessionModel[] =
    vocabularyCurrent?.models?.state === "present" ? (vocabularyCurrent.models.items ?? []) : [];
  const matchedModel = matchModelItem(modelItems, model, modelProvider);
  const modelEfforts = matchedModel?.efforts ?? null;
  const modelsPresent = !modelsView.freeText && vocabularyCurrent?.models?.state === "present";

  const offered = offeredFeatures(vocabularyCurrent, model);
  const probing = vocabularyError === null && featuresAreProbing(vocabularyCurrent);
  // Pi reads its catalog by probe: while that read runs the models axis is
  // `absent` with the features axis probing, and after a failure with it
  // answered-unavailable. Either way the generic absent sentence would
  // claim pi published nothing — the one thing that is false — so the
  // form names the read instead.
  const piModelsHint =
    providerId === "pi" && vocabularyCurrent?.models?.state === "absent"
      ? probing
        ? PI_MODELS_READING_TEXT
        : PI_MODELS_UNREADABLE_TEXT
      : undefined;
  const askedAndFailed = vocabularyError !== null || featuresAskFailed(vocabularyCurrent);
  const offeredForSeed = offered === null ? null : [...offered];
  const offeredKey = offered === null ? "none" : offered.map((feature) => feature.id).join(",");
  const lastOfferedKeyRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    const key = `${offeredKey}|${probing ? "probing" : "known"}`;
    if (lastOfferedKeyRef.current === key) {
      return;
    }
    lastOfferedKeyRef.current = key;
    onSeedChange?.(currentSeed());
  });

  const noteBytes = utf8Bytes(note);
  const spawnBytes = utf8Bytes(rustTrim(spawnPrompt));

  const providerChoices =
    providerId !== "" && !providers.some((provider) => provider.id === providerId)
      ? [...providers.map((provider) => provider.id), providerId]
      : providers.map((provider) => provider.id);

  // An edit keeps a stored model the catalog no longer lists visible and
  // selected, labelled as the saved one. A legacy bare id the catalog lists
  // under any serving provider needs no such row: it already matches.
  const storedModelOption =
    mode === "edit" &&
    model !== "" &&
    matchedModel === undefined &&
    !modelsView.items.some((item) => parseModelRef(item.value).model === model)
      ? [
          {
            value: modelRefOf({ modelId: model, provider: modelProvider ?? undefined }),
            label: `${model} (the value saved on this profile)`,
          },
        ]
      : [];
  const storedModeOption =
    mode === "edit" && modeId !== "" && !modesView.items.some((item) => item.value === modeId)
      ? [{ value: modeId, label: modeId }]
      : [];

  // The effort control: a picker over exactly the levels the matched model
  // publishes, nothing when the model publishes none, and free text only
  // while the daemon cannot enumerate at all.
  const showEffortPicker =
    modelsPresent && matchedModel !== undefined && (modelEfforts?.length ?? 0) > 0;
  const dropEffort =
    modelsPresent && matchedModel !== undefined && (modelEfforts?.length ?? 0) === 0;

  function changeModelRef(ref: string) {
    const parsed = parseModelRef(ref);
    const next = matchModelItem(modelItems, parsed.model, parsed.modelProvider);
    const effort = effortKept(next, thinkingOptionId) ? thinkingOptionId : "";
    setModel(parsed.model);
    setModelProvider(parsed.modelProvider);
    setThinkingOptionId(effort);
    // Settles with the bare id: the hook re-asks the features read for a
    // finished choice, and the compound picker value must never leak into
    // the settled model the reply is trusted against.
    settleModel(parsed.model);
    userChanged({
      model: parsed.model,
      modelProvider: parsed.modelProvider,
      thinkingOptionId: effort,
    });
  }

  function changeProvider(next: string) {
    if (next === providerId) return;
    providerDraftsRef.current.set(providerId, { model, modelProvider, modeId, thinkingOptionId });
    const restored = providerDraftsRef.current.get(next) ?? CLEARED_PROVIDER_FIELDS;
    setProviderId(next);
    settleModel(restored.model, false);
    setModel(restored.model);
    setModelProvider(restored.modelProvider);
    setModeId(restored.modeId);
    setThinkingOptionId(restored.thinkingOptionId);
    userChanged({ provider: next, ...restored });
  }

  function submit() {
    const draft = currentSeed();
    // A model that publishes no levels cannot carry one: the spawn would
    // refuse it, so the save drops it instead of sending work it cannot keep.
    if (dropEffort) draft.thinkingOptionId = "";
    if (mode === "create") {
      onCreate(draft);
    } else {
      onSaveSeed?.(draft);
    }
  }

  return (
    <div
      className={
        mode === "create" ? "agent-inline-editor agent-profile-create" : "agent-inline-editor"
      }
    >
      {hideHeading ? null : (
        <span className="settings-subheading">
          {mode === "create" ? "New profile" : "Edit profile"}
        </span>
      )}
      <label className="device-field">
        Name
        <input
          aria-label="Profile name"
          value={name}
          disabled={busy}
          onChange={(event) => {
            setName(event.target.value);
            userChanged({ name: event.target.value });
          }}
        />
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
      {!catalogLoading && catalogError === null && providers.length === 0 ? (
        <p className="device-field-hint">No agent CLI is installed on this machine.</p>
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
            value={
              modelsView.freeText
                ? model
                : matchedModel !== undefined
                  ? modelRefOf(matchedModel)
                  : model
            }
            busy={busy}
            freeText={modelsView.freeText}
            hint={piModelsHint ?? modelsView.hint}
            items={[...modelsView.items, ...storedModelOption]}
            onChange={(next) => {
              if (modelsView.freeText) {
                const parsed = parseModelRef(next);
                setModel(parsed.model);
                setModelProvider(parsed.modelProvider);
                userChanged({ model: parsed.model, modelProvider: parsed.modelProvider });
              } else {
                changeModelRef(next);
              }
            }}
            onSettle={modelsView.freeText ? settleModel : undefined}
          />
          {showEffortPicker ? (
            <label className="device-field">
              Effort
              <select
                aria-label="Effort"
                value={thinkingOptionId}
                disabled={busy}
                onChange={(event) => {
                  setThinkingOptionId(event.target.value);
                  userChanged({ thinkingOptionId: event.target.value });
                }}
              >
                <option value="">The provider&apos;s own default</option>
                {(modelEfforts ?? []).map((level) => (
                  <option key={level.id} value={level.id}>
                    {level.label}
                    {level.default === true ? " (default)" : ""}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          {!showEffortPicker && !dropEffort && (!modelsPresent || model !== "") ? (
            <label className="device-field">
              Effort
              <input
                aria-label="Effort"
                aria-describedby={thinkingHintId}
                value={thinkingOptionId}
                disabled={busy}
                onChange={(event) => {
                  setThinkingOptionId(event.target.value);
                  userChanged({ thinkingOptionId: event.target.value });
                }}
              />
              <span className="device-field-hint" id={thinkingHintId}>
                The id the provider uses, or empty for none.
              </span>
            </label>
          ) : null}
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
              userChanged({ modeId: next });
            }}
          />
        </>
      ) : null}
      <label className="device-field">
        Note (optional)
        <textarea
          aria-label="Profile note"
          value={note}
          disabled={busy}
          rows={2}
          onChange={(event) => {
            setNote(event.target.value);
            userChanged({ note: event.target.value });
          }}
        />
        <ByteCounter bytes={noteBytes} cap={MAX_PROFILE_NOTE_BYTES} />
      </label>
      <label className="device-field">
        Instructions (optional)
        <textarea
          aria-label="Profile instructions"
          value={spawnPrompt}
          disabled={busy}
          rows={2}
          onChange={(event) => {
            setSpawnPrompt(event.target.value);
            userChanged({ spawnPrompt: event.target.value });
          }}
        />
        <ByteCounter bytes={spawnBytes} cap={MAX_PROFILE_SPAWN_PROMPT_BYTES} />
      </label>
      <div className="profile-advanced" data-profile-advanced>
        <button
          type="button"
          className="settings-device-action"
          aria-expanded={advancedOpen}
          onClick={() => setAdvancedOpen((open) => !open)}
        >
          Advanced
        </button>
        {advancedOpen ? (
          <>
            <label className="device-field">
              Icon
              <input
                aria-label="Profile icon"
                aria-describedby={iconHintId}
                value={icon}
                disabled={busy}
                onChange={(event) => {
                  setIcon(event.target.value);
                  userChanged({ icon: event.target.value });
                }}
              />
              <span className="device-field-hint" id={iconHintId}>
                One glyph for the row; empty shows the name&apos;s first letter.
              </span>
            </label>
            <AgentProfileFeatureFields
              offered={offered}
              probing={probing}
              askedAndFailed={askedAndFailed}
              features={features}
              busy={busy}
              onChange={(next) => {
                setFeatures(next);
                userChanged({ features: next });
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
                  userChanged({ enabledForAgents: event.target.checked });
                }}
              />
              <span>
                <span>Agents may create this</span>
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
                  setIdleMinutes(
                    minutes !== null && Number.isFinite(minutes) ? String(minutes) : raw,
                  );
                  userChanged({
                    idleCloseMinutes: idleOff ? 0 : minutes,
                  });
                }}
              />
              <span className="device-field-hint" id={idleHintId}>
                {DEFAULT_IDLE_CLOSE_MINUTES} by default, up to {MAX_IDLE_CLOSE_MINUTES}.
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
                  userChanged({
                    idleCloseMinutes: off ? 0 : idleMinutesOf(idleMinutes),
                  });
                }}
              />
              <span>
                <span>Never close idle children</span>
              </span>
            </label>
            <AgentProfileOverlayEditor
              overlay={overlay}
              busy={busy}
              onChange={(next) => {
                setOverlay(next);
                userChanged({ overlay: next });
              }}
            />
          </>
        ) : null}
      </div>
      {formError === null ? null : (
        <p role="alert" className="device-error" ref={formErrorRef}>
          <ErrorText
            sentence={formError.sentence}
            detail={formError.detail}
            id="settings-profile-dialog-error"
          />
        </p>
      )}
      <div className="device-actions profile-form-actions">
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
