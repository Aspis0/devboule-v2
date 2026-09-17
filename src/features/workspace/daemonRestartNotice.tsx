// One honest line when the daemon restarted under the app. Reads the
// `instanceId` the app already polls; invents no second detector.
import { useEffect, useRef, useState } from "react";

export function DaemonRestartNotice({
  instanceId,
  hasRecovered,
}: {
  instanceId: string | null;
  hasRecovered: boolean;
}) {
  const previous = useRef<string | null>(null);
  const [restarted, setRestarted] = useState(false);

  useEffect(() => {
    if (instanceId === null) return;
    if (previous.current === null) {
      previous.current = instanceId;
      return;
    }
    if (previous.current !== instanceId) {
      previous.current = instanceId;
      setRestarted(true);
    }
  }, [instanceId]);

  if (!restarted || !hasRecovered) return null;
  return (
    <div
      className="workspace-session-error workspace-session-notice"
      role="status"
      data-testid="daemon-restart-notice"
    >
      <span className="workspace-session-error-text">
        The daemon restarted — recovered conversations are read-only until reopened.
      </span>
    </div>
  );
}
