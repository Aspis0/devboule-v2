import type { SessionManifest, SessionModel } from "../../types/ipc";
import type { ArtifactSection, ArtifactStructure } from "./artifactStructure";
import type {
  DesignDocument,
  DesignMessage,
  DesignTranscriptItem,
  SectionNote,
} from "./designHost";
import type { ResolvedSectionNote } from "./sectionNotes";
import type { MessageAction } from "./designSurfaceTypes";

export const EMPTY_DESIGN_MESSAGES: readonly DesignMessage[] = [];

export const EMPTY_TRANSCRIPT: readonly DesignTranscriptItem[] = [];

export const EMPTY_SECTIONS: readonly ArtifactSection[] = [];

export const EMPTY_ARTIFACT_STRUCTURE: ArtifactStructure = { sections: EMPTY_SECTIONS };

export const EMPTY_SECTION_NOTES: readonly SectionNote[] = [];

export const EMPTY_RESOLVED_NOTES: readonly ResolvedSectionNote[] = [];

export const HISTORY_OPEN_MESSAGE_PREFIX = "design-history-open-";

export function isHistoryOpenMessage(message: DesignMessage): boolean {
  return message.id.startsWith(HISTORY_OPEN_MESSAGE_PREFIX);
}

export function cloneMessages(document: DesignDocument): DesignMessage[] {
  return cloneMessageList(document.messages).map((message) =>
    normalizeIncompleteMessage(message, "loaded"),
  );
}

function cloneMessageList(messages: readonly DesignMessage[]): DesignMessage[] {
  return messages.map((message) =>
    message.role === "user"
      ? { ...message }
      : {
          ...message,
          sources: [...message.sources],
          nodeIds: [...message.nodeIds],
          ...(message.transcript === undefined ? {} : { transcript: [...message.transcript] }),
        },
  );
}

function normalizeIncompleteMessage(
  message: DesignMessage,
  phase: "loaded" | "saved",
): DesignMessage {
  if (message.role !== "assistant" || message.status !== "working") return message;
  const boundary = phase === "saved" ? "saved" : "loaded";
  return {
    ...message,
    status: "error",
    title: "Generation incomplete",
    desc: `This generation did not complete before the document was ${boundary}.`,
  };
}

export function terminalMessagesForSave(messages: readonly DesignMessage[]): DesignMessage[] {
  return cloneMessageList(messages).map((message) => normalizeIncompleteMessage(message, "saved"));
}

/**
 * The actions a card may offer. `canRegenerate` is `promptForMessage`'s answer
 * for this card — the same call the action itself makes — because an action with
 * no prompt behind it is a button whose only possible result is nothing: a
 * reopened history entry that points at a commissioned child carries no
 * instruction (see `historyEntryInstruction`).
 */
export function messageActions(
  message: DesignMessage,
  canGenerate: boolean,
  canRegenerate: boolean,
): readonly MessageAction[] {
  if (message.role === "user") return [];
  if (message.status === "working") return canGenerate ? ["stop"] : [];
  const regenerate: readonly MessageAction[] = canGenerate && canRegenerate ? ["regenerate"] : [];
  if (message.status === "error") return canGenerate && canRegenerate ? ["retry"] : [];
  if (message.nodeIds.length === 0) return regenerate;
  return canGenerate ? ["select", ...regenerate] : ["select"];
}

export function promptForMessage(
  messages: readonly DesignMessage[],
  message: DesignMessage,
): string | null {
  if (message.role !== "assistant") return null;
  const messageIndex = messages.findIndex((candidate) => candidate.id === message.id);
  const previousMessage = messageIndex > 0 ? messages[messageIndex - 1] : undefined;
  return previousMessage?.role === "user" ? previousMessage.text : (message.instruction ?? null);
}

export function manifestModel(manifest: SessionManifest | null): SessionModel | null {
  if (manifest === null || manifest.currentModelId === undefined) return null;
  return manifest.models.find((model) => model.modelId === manifest.currentModelId) ?? null;
}

export function confirmedEffort(model: SessionModel | null): string {
  if (
    model?.currentEffort !== undefined &&
    model.efforts?.some((entry) => entry.id === model.currentEffort)
  ) {
    return model.currentEffort;
  }
  return "";
}
