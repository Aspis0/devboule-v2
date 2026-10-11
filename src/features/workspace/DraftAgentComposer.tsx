import { PickerChip } from "../../components/PickerChip";
import type { ProviderInfo } from "../../types/ipc";
import { providerName } from "./providerNames";
import { WorkspaceComposer } from "./WorkspaceComposer";

export interface DraftAgentComposerProps {
  /** The chat-capable providers this machine can start, in the picker's order. */
  providers: readonly ProviderInfo[];
  /** The provider the person picked; the first of `providers` when none is picked. */
  providerId: string | null;
  onProviderChange: (providerId: string) => void;
  /** Starts the agent with the picked provider and sends `text` as its first
   * message. Answers false when the agent did not start, and the text stays. */
  onSend: (text: string) => Promise<boolean>;
  /** Why the composer cannot send yet, or null when it can. */
  disabledReason: string | null;
}

/**
 * The composer of a new workspace's first message. The agent is chosen here,
 * where it is used: the provider chip sits in the composer's controls, and the
 * message starts the agent. Model and effort are the agent's own chips once it
 * runs, since the provider's model list belongs to a running session.
 */
export function DraftAgentComposer({
  providers,
  providerId,
  onProviderChange,
  onSend,
  disabledReason,
}: DraftAgentComposerProps) {
  const current = providers.find((provider) => provider.id === providerId) ?? providers[0];
  return (
    <div className="workspace-draft-agent">
      <WorkspaceComposer
        streaming={false}
        turnActive={false}
        disabled={disabledReason !== null}
        disabledReason={disabledReason}
        onSend={(text) => onSend(text)}
        controls={
          providers.length === 0 ? null : (
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
          )
        }
      />
    </div>
  );
}
