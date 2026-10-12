import { useAppStore } from "../../store/appStore";
import { hasMirroredDesignCard, openMirroredDesign } from "../design/mirroredDesignOpen";

/**
 * The one action on a finished Design child's line. It is offered only while the
 * canvas holds that child's card, so it never points at an artifact that is gone,
 * and it is disabled while a generation runs, which would take the canvas over.
 */
export function DelegatedDesignOpenAction({ childSessionId }: { childSessionId: string }) {
  const mirrored = useAppStore((state) =>
    hasMirroredDesignCard(state.designSession.messages, childSessionId),
  );
  const generating = useAppStore((state) => state.designSession.generation !== null);
  if (!mirrored) return null;
  return (
    <button
      type="button"
      className="workspace-chat-activity-action"
      disabled={generating}
      title={generating ? "Design is generating" : undefined}
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
