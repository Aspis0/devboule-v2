import { useEffect, useRef, useState } from "react";
import { reasonFromCause, sessionPermissionRespond } from "../lib/tauri";
import type { DaemonConnectionState, PermissionRequest } from "../types/ipc";
import "./PermissionCard.css";

export type PermissionState = "waiting" | "submitting" | "allowed" | "denied";

export const PERMISSION_LABELS: Record<PermissionState, string> = {
  waiting: "Waiting on you",
  submitting: "Sending decision…",
  allowed: "Allowed once · running",
  denied: "Denied — the turn continues without it",
};

export function quotePermissionArg(value: string): string {
  if (value.length === 0 || /[\s"]/.test(value)) {
    return `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;
  }
  return value;
}

export function formatPermissionCommand(request: PermissionRequest): string | null {
  if (!request.command) return null;
  if (request.args === undefined || request.args.length === 0) return request.command;
  return [request.command, ...request.args].map(quotePermissionArg).join(" ");
}

export interface PermissionCardProps {
  sessionId: string;
  subscriptionId: number;
  request: PermissionRequest;
  capabilities: readonly string[];
  /** Design passes the live state so a disconnected daemon never hides a waiting request. */
  daemonState?: DaemonConnectionState;
  onRespond?: (outcome: "allow_once" | "deny") => Promise<void>;
  onResolved?: (sessionId: string, toolCallId: string) => void;
}

/** A real ACP permission prompt; it is inert unless the handshake negotiated typed_permissions. */
export function PermissionCard({
  sessionId,
  subscriptionId,
  request,
  capabilities,
  daemonState = "connected",
  onRespond,
  onResolved,
}: PermissionCardProps) {
  const [permission, setPermission] = useState<PermissionState>("waiting");
  const [error, setError] = useState<string | null>(null);
  const submittingRef = useRef(false);
  // A fresh subscription means the previous answer was sent over an attachment the
  // daemon no longer owns. Reset the card so the re-delivered request is answerable,
  // and bump the generation so the abandoned answer cannot overwrite this state.
  const generationRef = useRef(0);

  useEffect(() => {
    generationRef.current += 1;
    submittingRef.current = false;
    setPermission("waiting");
    setError(null);
  }, [sessionId, request.toolCallId, subscriptionId]);

  if (!capabilities.includes("typed_permissions") && daemonState === "connected") return null;

  const commandLine = formatPermissionCommand(request);
  const daemonReachable = daemonState === "connected";
  const allowSupported = request.options.some((option) => option.kind === "allow_once");
  const denySupported = request.options.some((option) => option.kind === "reject_once");

  const respond = async (outcome: "allow_once" | "deny") => {
    if (submittingRef.current || permission !== "waiting") return;
    const generation = generationRef.current;
    submittingRef.current = true;
    setPermission("submitting");
    setError(null);
    try {
      await (onRespond?.(outcome) ??
        sessionPermissionRespond(sessionId, subscriptionId, request.toolCallId, outcome));
      if (generationRef.current !== generation) return;
      setPermission(outcome === "allow_once" ? "allowed" : "denied");
      onResolved?.(sessionId, request.toolCallId);
    } catch (cause) {
      if (generationRef.current !== generation) return;
      submittingRef.current = false;
      setPermission("waiting");
      setError(reasonFromCause(cause));
    }
  };

  return (
    <div className="permission-card" aria-live="polite">
      <div className="permission-card-heading">
        <span className={`permission-card-dot permission-card-${permission}`} />
        <span>Permission · {request.title}</span>
        {request.cwd ? <span className="permission-card-context">{request.cwd}</span> : null}
      </div>
      {request.description ? (
        <div className="permission-card-description">{request.description}</div>
      ) : null}
      {commandLine ? <div className="permission-card-command">{commandLine}</div> : null}
      {request.env && request.env.length > 0 ? (
        <div className="permission-card-env">
          {request.env.map((variable) => `${variable.name}=${variable.value}`).join("\n")}
        </div>
      ) : null}
      {!daemonReachable ? (
        <div className="permission-card-unavailable" role="status">
          The daemon is not reachable. Reconnect to answer this request.
        </div>
      ) : null}
      {!allowSupported || !denySupported ? (
        <div className="permission-card-unavailable" role="status">
          {!allowSupported ? "Allow once is not offered for this request." : null}
          {!allowSupported && !denySupported ? " " : null}
          {!denySupported ? "Deny is not offered for this request." : null}
        </div>
      ) : null}
      <div className="permission-card-actions">
        <span className="permission-card-label">{PERMISSION_LABELS[permission]}</span>
        <button
          type="button"
          className="permission-card-secondary-action permission-card-deny-action"
          onClick={() => void respond("deny")}
          disabled={permission !== "waiting" || !daemonReachable || !denySupported}
        >
          Deny
        </button>
        <button
          type="button"
          className="permission-card-primary-action"
          onClick={() => void respond("allow_once")}
          disabled={permission !== "waiting" || !daemonReachable || !allowSupported}
        >
          Allow once
        </button>
      </div>
      {error ? <div role="alert">{error}</div> : null}
    </div>
  );
}
