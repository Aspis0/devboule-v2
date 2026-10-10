// The editor's data road: open once, poll the version, write through
// the model. Three spellings, one hook — a workspace path on this
// machine, a workspace path on a paired host (relayed over the held peer
// link, human-originated, no confirmation card; the daemon validates the
// device against the authenticated paired row), and an absolute or `~`
// path on this machine (app-only: the daemon refuses it to every peer,
// and no agent or MCP tool speaks it).
//
// The observation source polls the version (this daemon pushes nothing to
// the app outside session feeds and host status): a cheap stat every 5 s
// while mounted and visible, on window focus and visibility, plus an
// explicit refresh the conflict banner's Retry uses. Bytes are re-read
// only when the stamp moved past what the model holds — over the peer
// link especially, where every byte is a relayed round trip. One poll at
// a time, none while hidden, and a failed re-read feeds nothing (never
// the buffer as disk content) and retries next poll. A `missing` poll of
// a file that was missing at open is not an observation — there is
// nothing to conflict with yet, and feeding it to the model would paint
// "Deleted" under an empty editor that never existed. From the first
// landed write on, every poll is fed verbatim: the model's version turns
// `ready` exactly then, which is the flag this source watches.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  appFileOpen,
  appFileVersion,
  appFileWrite,
  remoteHostFileOpen,
  remoteHostFileVersion,
  remoteHostFileWrite,
  workspaceFileEditorOpen,
  workspaceFileEditorVersion,
  workspaceFileEditorWrite,
} from "../../../lib/tauri";
import type {
  WorkspaceEditableFile,
  WorkspaceFileVersion,
  WorkspaceFileWriteResult,
} from "../../../types/ipc";
import { LOCAL_HOST_ID, parseWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { FileEditorModel, type FileEditorObservation, type FileObservationSource } from "./model";
import { registerEditor } from "./editorRegistry";

export type EditableFileTarget =
  | { kind: "workspace"; workspaceId: string; path: string }
  | { kind: "remote"; deviceId: string; workspaceId: string; path: string }
  | { kind: "outside"; path: string };

/** An absolute path on either platform, or `~`: the human's own file,
 * never a workspace-relative spelling. */
export function isOutsidePath(path: string): boolean {
  const trimmed = path.trim();
  return (
    trimmed.startsWith("/") ||
    trimmed.startsWith("~") ||
    /^[A-Za-z]:[\\/]/.test(trimmed) ||
    trimmed.startsWith("\\\\")
  );
}

/** Which road a File tab reads and writes: outside paths go app-only,
 * remote hosts go over the held link, the rest is the local workspace. */
export function resolveEditableTarget(
  workspaceKey: WorkspaceKey,
  path: string,
): EditableFileTarget {
  if (isOutsidePath(path)) return { kind: "outside", path: path.trim() };
  const { hostId, workspaceId } = parseWorkspaceKey(workspaceKey);
  if (hostId !== LOCAL_HOST_ID) return { kind: "remote", deviceId: hostId, workspaceId, path };
  return { kind: "workspace", workspaceId, path };
}

/** Oldest daemon dialect the editor frames need: mirrors
 * `FILE_EDIT_MIN_VERSION` in `devboule-protocol` (a relay checks the far
 * hello against it; the tab checks the local daemon the same way). */
export const EDITOR_MIN_DIALECT = 33;

const POLL_MS = 5000;

async function openTarget(target: EditableFileTarget): Promise<WorkspaceEditableFile> {
  switch (target.kind) {
    case "workspace":
      return workspaceFileEditorOpen(target.workspaceId, target.path);
    case "remote":
      return remoteHostFileOpen(target.deviceId, target.workspaceId, target.path);
    case "outside":
      return appFileOpen(target.path);
  }
}

async function versionTarget(target: EditableFileTarget): Promise<WorkspaceFileVersion> {
  switch (target.kind) {
    case "workspace":
      return workspaceFileEditorVersion(target.workspaceId, target.path);
    case "remote":
      return remoteHostFileVersion(target.deviceId, target.workspaceId, target.path);
    case "outside":
      return appFileVersion(target.path);
  }
}

async function writeTarget(
  target: EditableFileTarget,
  content: string,
  expectedModifiedAt: number | null | undefined,
  expectedRevision: string | null | undefined,
  create: boolean,
): Promise<WorkspaceFileWriteResult> {
  const at = expectedModifiedAt ?? null;
  const revision = expectedRevision ?? null;
  switch (target.kind) {
    case "workspace":
      return workspaceFileEditorWrite(
        target.workspaceId,
        target.path,
        content,
        at,
        revision,
        create,
      );
    case "remote":
      return remoteHostFileWrite(
        target.deviceId,
        target.workspaceId,
        target.path,
        content,
        at,
        revision,
        create,
      );
    case "outside":
      return appFileWrite(target.path, content, at, revision, create);
  }
}

interface ObservationCell {
  listeners: Set<() => void>;
  observation: FileEditorObservation | null;
  /** Still a never-created file: `missing` polls are skipped, `ready`
   * polls are fed. Clears when the model's version turns `ready`. */
  openedMissing: boolean;
  bom: boolean;
  poll: () => void;
}

export interface EditableFile {
  /** The model the view binds: null until the open lands or when it
   * refuses. A new model per open — never reused across files. */
  model: FileEditorModel | null;
  loading: boolean;
  /** The open's refusal sentence, or a version poll's failure while the
   * model keeps the last good observation. Null when usable. */
  failure: string | null;
  /** Poll the version now (the conflict banner's Retry, the tab's
   * re-click). */
  refresh: () => void;
  /** Re-open from scratch (a refused open's retry). */
  reopen: () => void;
  reopening: boolean;
}

export function useEditableFile(
  workspaceKey: WorkspaceKey,
  path: string,
  refreshNonce: number,
): EditableFile {
  const target = useMemo(() => resolveEditableTarget(workspaceKey, path), [workspaceKey, path]);
  const key = useMemo(() => JSON.stringify(target), [target]);
  const [model, setModel] = useState<FileEditorModel | null>(null);
  const [loading, setLoading] = useState(true);
  const [failure, setFailure] = useState<string | null>(null);
  const [reopening, setReopening] = useState(false);
  // Bumped to re-run the open effect (retry) without changing the
  // target the effect is keyed on. The tab's own refresh nonce is NOT a
  // dep: re-clicking the active tab refreshes (polls) instead of
  // re-opening, so the buffer, the conflict banner and the undo history
  // survive the click.
  const [openNonce, setOpenNonce] = useState(0);
  // A manual reopen (the failure banner's Retry) holds `reopening` true
  // across the async open, so the button disables instead of launching
  // concurrent polls. Target changes re-open silently and never touch it.
  const reopenArmed = useRef(false);
  const road = useRef(target);
  road.current = target;
  const cell = useRef<ObservationCell | null>(null);
  // The live model, beside the state mirror: cleanup owns the lifecycle
  // directly (a `setModel` updater inside a cleanup never runs on an
  // unmounting component, which would leak the autosave timer and drop
  // the buffer).
  const modelRef = useRef<FileEditorModel | null>(null);
  const releaseEditor = useRef<(() => void) | null>(null);

  // The open: one model per landed file, disposed with the tab.
  useEffect(() => {
    let live = true;
    setLoading(true);
    setFailure(null);
    setModel(null);
    cell.current = null;
    const finish = () => {
      setLoading(false);
      if (reopenArmed.current) {
        reopenArmed.current = false;
        setReopening(false);
      }
    };
    void (async () => {
      let opened: WorkspaceEditableFile;
      try {
        opened = await openTarget(road.current);
      } catch (cause: unknown) {
        if (!live) return;
        setFailure(cause instanceof Error ? cause.message : String(cause));
        finish();
        return;
      }
      if (!live) return;
      if (opened.status !== "ok" || opened.content === null || opened.version === null) {
        setFailure(opened.error ?? "The file could not be opened.");
        finish();
        return;
      }
      const openedMissing = opened.version.status === "missing";
      const entry: ObservationCell = {
        listeners: new Set(),
        observation: null,
        openedMissing,
        bom: opened.hasBom ?? false,
        poll: () => undefined,
      };
      cell.current = entry;
      const next = new FileEditorModel({
        file: { content: opened.content, hasBom: entry.bom, version: opened.version },
        session: {
          write: (input) =>
            writeTarget(
              road.current,
              input.content,
              input.expectedModifiedAt,
              input.expectedRevision,
              input.create ?? false,
            ),
        },
        ...(openedMissing ? { missing: true } : {}),
      });
      const source: FileObservationSource = {
        subscribe: (listener) => {
          entry.listeners.add(listener);
          return () => {
            entry.listeners.delete(listener);
          };
        },
        getObservation: () => entry.observation,
        refresh: () => entry.poll(),
      };
      next.connectFileObservations(source);
      // A ready open is the first observation: the poll below would take
      // up to 5 s to adopt the precise revision otherwise. A missing open
      // feeds nothing — there is no file to observe yet.
      if (!openedMissing && opened.version.status === "ready") {
        entry.observation = {
          status: "ready",
          file: { content: opened.content, hasBom: entry.bom, version: opened.version },
        };
      }
      modelRef.current = next;
      releaseEditor.current = registerEditor(next, path);
      setModel(next);
      finish();
    })();
    return () => {
      live = false;
      cell.current = null;
      // Flush first: a dirty, failed or conflicted-with-edits buffer
      // saves now instead of dying with the debounce timer. `save()` is
      // a no-op on conflict by design (neither overwrite nor reload is
      // safe automatically) — the close flow reports those instead — so
      // calling it here is explicit, not operative.
      const retiring = modelRef.current;
      modelRef.current = null;
      releaseEditor.current?.();
      releaseEditor.current = null;
      if (retiring !== null) {
        const status = retiring.getSnapshot().status;
        if (status === "dirty" || status === "error" || status === "conflict") {
          void retiring.save();
        }
        retiring.dispose();
      }
      setModel(null);
    };
    // Keyed on the serialised target and the manual retry nonce only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, openNonce]);

  // The tab's re-click: poll now through the source instead of
  // rebuilding the model. Skips the mount nonce the open effect above
  // already served, so activating the tab never re-reads by itself.
  const lastRefreshRef = useRef(refreshNonce);
  useEffect(() => {
    if (lastRefreshRef.current === refreshNonce) return;
    lastRefreshRef.current = refreshNonce;
    cell.current?.poll();
  }, [refreshNonce]);

  // The version poll: a cheap stat, no bytes. Bytes are re-read only
  // when the stamp moved past what the model holds; a failed re-read
  // feeds nothing (never the buffer as disk content) and retries next
  // poll. One poll at a time (a slow remote relay must not stack them
  // against its own deadline), and none while the tab is hidden.
  useEffect(() => {
    const entry = cell.current;
    const boundModel = model;
    if (!entry || !boundModel) return;
    let live = true;
    let inflight = false;
    const notify = () => {
      const current = cell.current;
      if (!current) return;
      for (const listener of current.listeners) listener();
    };
    const poll = async () => {
      if (inflight || document.hidden) return;
      const current = cell.current;
      if (!current || !live) return;
      // The first landed write turns the version `ready`: from then on
      // every poll is fed verbatim, missing ones included.
      if (boundModel.getSnapshot().version.status === "ready") current.openedMissing = false;
      inflight = true;
      let version: WorkspaceFileVersion;
      try {
        version = await versionTarget(road.current);
      } catch (cause: unknown) {
        inflight = false;
        if (!live || cell.current !== current) return;
        setFailure(cause instanceof Error ? cause.message : String(cause));
        return;
      }
      if (!live || cell.current !== current) {
        inflight = false;
        return;
      }
      setFailure(null);
      if (version.status === "missing") {
        // Skipped while still a never-created file (see the module note);
        // a deletion from here on.
        if (current.openedMissing) {
          inflight = false;
          return;
        }
        current.observation = version;
        notify();
        inflight = false;
        return;
      }
      if (version.status === "error") {
        current.observation = version;
        notify();
        inflight = false;
        return;
      }
      // Ready: compare against what the model holds first. An unchanged
      // stamp is the common case and costs no bytes — over the peer link
      // especially, where every byte is a relayed round trip.
      const held = boundModel.getSnapshot().version;
      const moved =
        held.status !== "ready" ||
        held.modifiedAt !== version.modifiedAt ||
        held.revision !== version.revision ||
        held.size !== version.size;
      if (!moved) {
        inflight = false;
        return;
      }
      let content: string | null = null;
      let bom = current.bom;
      try {
        const reopened = await openTarget(road.current);
        if (reopened.status === "ok" && reopened.content !== null) {
          content = reopened.content;
          bom = reopened.hasBom ?? bom;
        }
      } catch {
        // Bytes failed: feed nothing and say so. Feeding the buffer as
        // disk content would paint a spurious conflict, and `reload()`
        // would then adopt our own bytes as persisted (dirty cleared
        // without a write). The next poll retries the bytes.
      }
      if (!live || cell.current !== current) {
        inflight = false;
        return;
      }
      if (content === null) {
        setFailure("The file could not be re-read.");
        inflight = false;
        return;
      }
      current.bom = bom;
      current.observation = {
        status: "ready",
        file: { content, hasBom: bom, version },
      };
      notify();
      inflight = false;
    };
    entry.poll = () => void poll();
    const timer = setInterval(() => void poll(), POLL_MS);
    const onFocus = () => void poll();
    const onVisible = () => {
      if (!document.hidden) void poll();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      live = false;
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [model]);

  const refresh = useCallback(() => {
    cell.current?.poll();
  }, []);
  const reopen = useCallback(() => {
    reopenArmed.current = true;
    setReopening(true);
    setOpenNonce((nonce) => nonce + 1);
  }, []);

  return { model, loading, failure, refresh, reopen, reopening };
}
