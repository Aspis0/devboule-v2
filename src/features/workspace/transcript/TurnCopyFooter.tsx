import { formatTaskDuration } from "../../../lib/backgroundTaskText";
import { MessageCopyButton } from "../timeline/MessageCopyButton";

/**
 * The footer under a completed turn: a copy of the whole turn, and how long it
 * worked, when this view saw it start. Paseo's turn footer, in our own type.
 */
export function TurnCopyFooter({ text, workedMs }: { text: string; workedMs?: number }) {
  const worked = workedMs === undefined ? null : formatTaskDuration(workedMs);
  return (
    <div className="workspace-chat-turn-footer">
      <MessageCopyButton text={text} />
      {worked === null ? null : (
        <span className="workspace-chat-turn-worked">{`Worked for ${worked}`}</span>
      )}
    </div>
  );
}
