import { PickerChip } from "../../components/PickerChip";
import type { ProviderInfo, SessionModel, SessionModeView } from "../../types/ipc";
import { modelLabel } from "./modelLabel";
import { providerName } from "./providerNames";
import { WorkspaceComposer } from "./WorkspaceComposer";

/** What the first message's agent can be started with: the provider's models
 * and modes as its catalog reports them, before any agent runs. */
export interface DraftChoices {
  models: readonly SessionModel[];
  modes: readonly SessionModeView[];
}

const NO_SELECTION: DraftSelection = { modelId: null, effort: null, modeId: null };

/** The picks made in the composer. `null` is 'nothing picked yet': the first choice shows. */
export interface DraftSelection {
  modelId: string | null;
  effort: string | null;
  modeId: string | null;
}

/** What the composer shows, and so what the agent is started with: each pick,
 * or the first choice the catalog lists when nothing was picked. */
export interface ResolvedDraftSelection {
  modelId: string | null;
  effort: string | null;
  modeId: string | null;
}

export function resolveDraftSelection(
  choices: DraftChoices | null,
  selection: DraftSelection,
): ResolvedDraftSelection {
  const models = choices?.models ?? [];
  const model = models.find((candidate) => candidate.modelId === selection.modelId) ?? models[0];
  const efforts = model?.efforts ?? [];
  const effort =
    efforts.find((candidate) => candidate.id === selection.effort) ??
    efforts.find((candidate) => candidate.default === true) ??
    efforts[0];
  const modes = choices?.modes ?? [];
  const mode = modes.find((candidate) => candidate.id === selection.modeId) ?? modes[0];
  return { modelId: model?.modelId ?? null, effort: effort?.id ?? null, modeId: mode?.id ?? null };
}

export interface DraftAgentComposerProps {
  /** The chat-capable providers this machine can start, in the picker's order. */
  providers: readonly ProviderInfo[];
  /** The provider the person picked; the first of `providers` when none is picked. */
  providerId: string | null;
  onProviderChange: (providerId: string) => void;
  /** The chosen provider's models and modes; null while they load or when unknown. */
  choices?: DraftChoices | null;
  selection?: DraftSelection;
  onSelectionChange?: (next: DraftSelection) => void;
  /** Starts the agent with the picks and sends `text` as its first message.
   * Resolves true only once the agent confirms it; the text stays in the
   * composer until then. */
  onSend: (text: string) => Promise<boolean>;
  /** Why the composer cannot send yet, or null when it can. */
  disabledReason: string | null;
  /** What happened to the last send, in one line, when it did not go through. */
  notice?: string | null;
  /** Text handed back to the composer, applied once per `nonce`. */
  restore?: { text: string; nonce: number } | null;
}

/**
 * The composer of a new workspace's first message. The agent is chosen here,
 * where it is used: the provider, the model, its effort and the mode sit in the
 * composer's controls, and the message starts the agent. The text is kept until
 * the agent confirms it, so a refused or interrupted send leaves the prompt in
 * the box.
 */
export function DraftAgentComposer({
  providers,
  providerId,
  onProviderChange,
  choices = null,
  selection = NO_SELECTION,
  onSelectionChange = () => undefined,
  onSend,
  disabledReason,
  notice = null,
  restore = null,
}: DraftAgentComposerProps) {
  const current = providers.find((provider) => provider.id === providerId) ?? providers[0];
  const models = choices?.models ?? [];
  const modes = choices?.modes ?? [];
  const shown = resolveDraftSelection(choices, selection);
  const model = models.find((candidate) => candidate.modelId === shown.modelId);
  const efforts = model?.efforts ?? [];
  const effort = efforts.find((candidate) => candidate.id === shown.effort);
  const mode = modes.find((candidate) => candidate.id === shown.modeId);
  return (
    <div className="workspace-draft-agent">
      {notice === null ? null : (
        <p className="workspace-draft-notice" role="status">
          {notice}
        </p>
      )}
      <WorkspaceComposer
        streaming={false}
        turnActive={false}
        disabled={disabledReason !== null}
        disabledReason={disabledReason}
        keepUntilSettled
        restoreDraft={
          restore === null ? null : { text: restore.text, focus: true, nonce: restore.nonce }
        }
        onSend={(text) => onSend(text)}
        controls={
          <>
            {providers.length === 0 ? null : (
              <PickerChip
                label="Agent"
                options={providers.map((provider) => ({
                  id: provider.id,
                  name: providerName(provider.id) ?? provider.id,
                }))}
                currentId={current?.id ?? null}
                onSelect={onProviderChange}
                chipTestId="draft-provider-chip"
                optionTestId={(id) => `draft-provider-option-${id}`}
              />
            )}
            {models.length === 0 ? null : (
              <PickerChip
                label="Model"
                options={models.map((candidate) => ({
                  id: candidate.modelId,
                  name: modelLabel(candidate),
                  description: candidate.description,
                }))}
                currentId={model?.modelId ?? null}
                onSelect={(modelId) => onSelectionChange({ ...selection, modelId, effort: null })}
                chipTestId="draft-model-chip"
                optionTestId={(id) => `draft-model-option-${id}`}
              />
            )}
            {efforts.length < 2 ? null : (
              <PickerChip
                label="Effort"
                options={efforts.map((candidate) => ({
                  id: candidate.id,
                  name: candidate.label,
                  description: candidate.description,
                }))}
                currentId={effort?.id ?? null}
                onSelect={(effortId) => onSelectionChange({ ...selection, effort: effortId })}
                chipTestId="draft-effort-chip"
                optionTestId={(id) => `draft-effort-option-${id}`}
              />
            )}
            {modes.length === 0 ? null : (
              <PickerChip
                label="Mode"
                options={modes.map((candidate) => ({
                  id: candidate.id,
                  name: candidate.name,
                  description: candidate.description,
                }))}
                currentId={mode?.id ?? null}
                onSelect={(modeId) => onSelectionChange({ ...selection, modeId })}
                chipTestId="draft-mode-chip"
                optionTestId={(id) => `draft-mode-option-${id}`}
              />
            )}
          </>
        }
      />
    </div>
  );
}
