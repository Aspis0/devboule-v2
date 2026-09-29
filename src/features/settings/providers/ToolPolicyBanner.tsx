import { useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { daemonRestart } from "../../../lib/tauri";
import { isCommandError } from "../../../lib/commandError";

/**
 * The failed-closed tool-policy banner: the daemon denied every restrictable
 * broker tool at startup and keeps denying until the file is fixed or removed
 * and the daemon restarts. Toggles stay locked while this shows — a write
 * would be refused by the daemon anyway. The reason is the daemon's own
 * sentence, shown verbatim.
 */
export function ToolPolicyBanner({ reason }: { reason: string }) {
  const [restarting, setRestarting] = useState(false);
  const [restartError, setRestartError] = useState<string | null>(null);

  return (
    <div className="prov-policy-banner" role="alert">
      <span className="prov-policy-banner-title">Tool policy failed closed.</span>{" "}
      <span className="prov-policy-banner-reason">{reason}</span>{" "}
      <span className="prov-policy-banner-remedy">Fix the file or remove it, then restart.</span>{" "}
      <button type="button" disabled={restarting} onClick={() => void restartDaemon()}>
        {restarting ? "Restarting…" : "Restart daemon"}
      </button>
      {restartError ? <span className="prov-policy-banner-error">{restartError}</span> : null}
    </div>
  );

  /**
   * Restarting kills live turns (transcripts survive in the journal), so the
   * click asks first — the same consent the workspace recovery flow requires
   * before a restart. Failures say what they are on the banner instead of
   * vanishing into an unhandled promise, and the button locks while the
   * confirm or the restart is in flight so a double click cannot stack dialogs.
   */
  async function restartDaemon(): Promise<void> {
    if (restarting) return;
    setRestarting(true);
    setRestartError(null);
    try {
      let confirmed: boolean;
      try {
        confirmed = await ask(
          "Restarting the daemon will stop the agents running right now — conversations are kept and can be reopened. Restart now?",
          { title: "Restart daemon", kind: "warning", okLabel: "Restart", cancelLabel: "Cancel" },
        );
      } catch {
        setRestartError("The confirmation did not open.");
        return;
      }
      if (!confirmed) return;
      try {
        await daemonRestart();
      } catch (cause) {
        setRestartError(`Restart failed: ${commandMessage(cause)}`);
      }
    } finally {
      setRestarting(false);
    }
  }
}

/**
 * The command's own message, verbatim: a bridge or daemon sentence the banner
 * must not rewrite, only introduce.
 */
function commandMessage(cause: unknown): string {
  if (isCommandError(cause)) return cause.message;
  if (cause instanceof Error && cause.message !== "") return cause.message;
  return "the request was rejected without a readable error";
}
