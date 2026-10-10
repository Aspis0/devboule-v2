// The editor's data road: open once, poll the version, write through
// the model. Three spellings, one hook — a workspace path on this
// machine, a workspace path on a paired host (relayed over the held peer
// link, human-originated, no confirmation card; the daemon validates the
// device against the authenticated paired row), and an absolute or `~`
// path on this machine (app-only: the daemon refuses it to every peer,
// and no agent or MCP tool speaks it).
//
// The observation source polls the version (this daemon pushes nothing to
// the app outside session feeds and host status): every 5 s while mounted
// and on window focus, plus an explicit refresh the conflict banner's
// Retry uses. A `missing` poll of a file that was missing at open is not
// an observation — there is nothing to conflict with yet, and feeding it
// to the model would paint "Deleted" under an empty editor that never
// existed. From the first landed write on, every poll is fed verbatim:
// the model's version turns `ready` exactly then, which is the flag this
// source watches.

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
  // Bumped to re-run the open effect (re-click, retry) without changing
  // the target the effect is keyed on.
  const [openNonce, setOpenNonce] = useState(0);
  const road = useRef(target);
  road.current = target;
  const cell = useRef<ObservationCell | null>(null);

  // The open: one model per landed file, disposed with the tab.
  useEffect(() => {
    let live = true;
    setLoading(true);
    setFailure(null);
    setModel(null);
    cell.current = null;
    void (async () => {
      let opened: WorkspaceEditableFile;
      try {
        opened = await openTarget(road.current);
      } catch (cause: unknown) {
        if (!live) return;
        setFailure(cause instanceof Error ? cause.message : String(cause));
        setLoading(false);
        return;
      }
      if (!live) return;
      if (opened.status !== "ok" || opened.content === null || opened.version === null) {
        setFailure(opened.error ?? "The file could not be opened.");
        setLoading(false);
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
      setModel(next);
      setLoading(false);
    })();
    return () => {
      live = false;
      cell.current = null;
      setModel((current) => {
        current?.dispose();
        return null;
      });
    };
    // Keyed on the serialised target, the manual nonce and the tab's own
    // refresh: a re-click re-opens through the same road.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, openNonce, refreshNonce]);

  // The version poll: a cheap stat, no bytes — except when the stamp
  // moved, when the bytes are re-read so the model can adopt them (clean)
  // or conflict on them (dirty). A failed poll keeps the last good
  // observation and surfaces its sentence beside the editor.
  useEffect(() => {
    const entry = cell.current;
    const boundModel = model;
    if (!entry || !boundModel) return;
    let live = true;
    const notify = () => {
      const entry = cell.current;
      if (!entry) return;
      for (const listener of entry.listeners) listener();
    };
    const poll = async () => {
      const entry = cell.current;
      if (!entry || !live) return;
      // The first landed write turns the version `ready`: from then on
      // every poll is fed verbatim, missing ones included.
      if (boundModel.getSnapshot().version.status === "ready") entry.openedMissing = false;
      let version: WorkspaceFileVersion;
      try {
        version = await versionTarget(road.current);
      } catch (cause: unknown) {
        if (!live || cell.current !== entry) return;
        setFailure(cause instanceof Error ? cause.message : String(cause));
        return;
      }
      if (!live || cell.current !== entry) return;
      setFailure(null);
      if (version.status === "missing") {
        // Skipped while still a never-created file (see the module note);
        // a deletion from here on.
        if (entry.openedMissing) return;
        entry.observation = version;
        notify();
        return;
      }
      if (version.status === "error") {
        entry.observation = version;
        notify();
        return;
      }
      // Ready: re-read the bytes the stamp moved under. A failed re-read
      // still reports the stamp — the model learns something changed, and
      // the next poll retries the bytes.
      let content: string | null = null;
      let bom = entry.bom;
      try {
        const reopened = await openTarget(road.current);
        if (reopened.status === "ok" && reopened.content !== null) {
          content = reopened.content;
          bom = reopened.hasBom ?? bom;
        }
      } catch {
        // Kept: the version below still moves the stamp.
      }
      if (!live || cell.current !== entry) return;
      entry.bom = bom;
      entry.observation = {
        status: "ready",
        file: {
          content: content ?? boundModel.getSnapshot().content,
          hasBom: bom,
          version,
        },
      };
      notify();
    };
    entry.poll = () => void poll();
    const timer = setInterval(() => void poll(), POLL_MS);
    const onFocus = () => void poll();
    window.addEventListener("focus", onFocus);
    return () => {
      live = false;
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [model]);

  const refresh = useCallback(() => {
    cell.current?.poll();
  }, []);
  const reopen = useCallback(() => {
    setReopening(true);
    setOpenNonce((nonce) => nonce + 1);
    setReopening(false);
  }, []);

  return { model, loading, failure, refresh, reopen, reopening };
}
