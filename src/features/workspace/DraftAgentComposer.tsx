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
   * message. Resolves true only once the agent confirms it; the text stays in
   * the composer until then. */
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
 * where it is used: the provider chip sits in the composer's controls, and the
 * message starts the agent. The text is kept until the agent confirms it, so a
 * refused or interrupted send leaves the prompt in the box.
 */
export function DraftAgentComposer({
  providers,
  providerId,
  onProviderChange,
  onSend,
  disabledReason,
  notice = null,
  restore = null,
}: DraftAgentComposerProps) {
  const current = providers.find((provider) => provider.id === providerId) ?? providers[0];
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
