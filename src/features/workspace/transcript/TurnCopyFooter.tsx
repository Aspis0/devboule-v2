import { formatTaskDuration } from "../../../lib/backgroundTaskText";
import { MessageCopyButton } from "../timeline/MessageCopyButton";
import { abnormalStop } from "./turnEnds";

/**
 * The footer under a completed turn: a copy of the whole turn, and how long it
 * worked, when this view saw it start. An abnormal stop reads as such instead,
 * with the provider's reason in the title. Paseo's turn footer, in our own type.
 */
export function TurnCopyFooter({
  text,
  workedMs,
  stopReason,
}: {
  text: string;
  workedMs?: number;
  stopReason?: string;
}) {
  const worked = workedMs === undefined ? null : formatTaskDuration(workedMs);
  const stopped = abnormalStop(stopReason);
  return (
    <div className="workspace-chat-turn-footer">
      <MessageCopyButton text={text} />
      {stopped !== null ? (
        <span className="workspace-chat-turn-stopped" title={`Stopped: ${stopped}`}>
          {worked === null ? "Stopped" : `Stopped after ${worked}`}
        </span>
      ) : worked === null ? null : (
        <span className="workspace-chat-turn-worked">{`Worked for ${worked}`}</span>
      )}
    </div>
  );
}
