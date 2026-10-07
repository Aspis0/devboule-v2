/**
 * What a click on an attention toast means on this side: the one event that
 * carries the session the toast announced, and the road that opens it.
 *
 * The toast's words, its gate and its dedupe belong to `attentionNotice.ts`;
 * the click can only come back from the process that showed the OS toast, so
 * the Rust half shows it and publishes one app event here. The listener is
 * app-scope — a click must open its session whatever surface is on screen —
 * and so is the roster it reads.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AttentionTarget } from "../../lib/tauri";
import { useAppStore } from "../../store/appStore";
import { localWorkspaceKey } from "./hosts/hostIdentity";
import { setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
import { sharedSessionController } from "./workspaceSessions";

/** The event name both languages use; declared once, in the Rust half too. */
const ATTENTION_ACTIVATED_EVENT = "attention:activated";

function isText(value: unknown): value is string {
  return typeof value === "string";
}

/** The event's payload, or null for anything that is not exactly one target: a
 *  half-applied click would open the wrong thing. */
function attentionTargetFrom(value: unknown): AttentionTarget | null {
  if (typeof value !== "object" || value === null) return null;
  const row = value as Record<string, unknown>;
  if (!isText(row.sessionId) || row.sessionId === "") return null;
  if (row.workspaceId !== null && !isText(row.workspaceId)) return null;
  return { sessionId: row.sessionId, workspaceId: row.workspaceId };
}

/** How the surface that owns the strip opens a target: the same roads a row
 *  click uses. Null while that surface is not mounted. */
type AttentionOpener = (target: AttentionTarget) => void;

let opener: AttentionOpener | null = null;

export function setAttentionOpener(next: AttentionOpener | null): void {
  opener = next;
}

/**
 * The click arrived while no surface was showing the strip. The app-lifetime
 * roads are what is left: land on the Workspace surface, write the workspace
 * the surface reads on its way up, and open the session's tab so the strip
 * starts on it. A session the roster no longer names is never opened — the
 * workspace is the answer, with a fresh read.
 */
function openWithNoStrip(target: AttentionTarget): void {
  useAppStore.getState().selectSurface("workspace");
  const key = target.workspaceId === null ? null : localWorkspaceKey(target.workspaceId);
  if (key !== null) setLastSelectedWorkspaceKey(key);
  const controller = sharedSessionController();
  const session = controller.getState().sessions.find((row) => row.id === target.sessionId);
  if (session === undefined) {
    void controller.refresh();
    return;
  }
  controller.open(session);
}

/** One click, delivered: the mounted strip's own road when it is there, the
 *  app-lifetime roads when it is not. */
function applyAttentionActivation(payload: unknown): void {
  const target = attentionTargetFrom(payload);
  if (target === null) return;
  if (opener !== null) {
    opener(target);
    return;
  }
  openWithNoStrip(target);
}

let activationListener: Promise<UnlistenFn> | null = null;
let releaseListener: UnlistenFn | null = null;

/**
 * The one listener for the app run. Repeated calls share the first
 * registration — a second listener would apply one click twice, and React's
 * StrictMode asks for the effect twice.
 */
export function startAttentionActivation(): Promise<UnlistenFn> {
  activationListener ??= listen<unknown>(ATTENTION_ACTIVATED_EVENT, (event) =>
    applyAttentionActivation(event.payload),
  )
    .then((unlisten) => {
      releaseListener = unlisten;
      return unlisten;
    })
    .catch(() => {
      // No event bridge — a host without Tauri, or a test: nothing can click,
      // and there is nothing to unlisten.
      return () => undefined;
    });
  return activationListener;
}

/**
 * Releases the listener and forgets the opener. Vite replaces this module while
 * the webview stays alive, and the replaced copy's listener would otherwise
 * answer every click a second time, through the opener it captured.
 */
export function disposeAttentionActivation(): void {
  releaseListener?.();
  releaseListener = null;
  activationListener = null;
  opener = null;
}

import.meta.hot?.dispose(disposeAttentionActivation);
