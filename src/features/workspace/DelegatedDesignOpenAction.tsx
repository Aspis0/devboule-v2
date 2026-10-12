import { useAppStore } from "../../store/appStore";
import { hasMirroredDesignCard, openMirroredDesign } from "../design/mirroredDesignOpen";

/**
 * The one action on a finished Design child's line. It is offered only while the
 * canvas holds that child's card, so it never points at an artifact that is gone.
 */
export function DelegatedDesignOpenAction({ childSessionId }: { childSessionId: string }) {
  const mirrored = useAppStore((state) =>
    hasMirroredDesignCard(state.designSession.messages, childSessionId),
  );
  if (!mirrored) return null;
  return (
    <button
      type="button"
      className="workspace-chat-activity-action"
      onClick={(event) => {
        // The button sits inside the line's summary, whose click would fold the detail.
        event.preventDefault();
        openMirroredDesign(childSessionId);
      }}
    >
      Open in Design
    </button>
  );
}
