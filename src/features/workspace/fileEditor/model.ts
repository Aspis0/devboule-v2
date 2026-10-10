// Copied from Paseo's `packages/app/src/file-pane/editor/model.ts`
// (Apache-2.0, Copyright (c) 2025-present Mohamed Boudra): the editor
// model — autosave debounce 800 ms, Ctrl/Cmd+S through the view's keymap,
// `modified` vs `persistedContent`, and the conflict states
// changed/deleted/checkFailed with Overwrite/Reload. Only the wire types
// changed: Paseo's `FileVersion`/`FileWriteResult` are this app's
// `WorkspaceFileVersion`/`WorkspaceFileWriteResult` (the version names a
// workspace, empty for an app-file path). No vim, no behaviour change.

import type { WorkspaceFileVersion, WorkspaceFileWriteResult } from "../../../types/ipc";

export type FileEditorStatus = "clean" | "dirty" | "saving" | "conflict" | "error";
export type FileLineSeparator = "\n" | "\r\n" | "\r";

export interface FileEditorSnapshot {
  status: FileEditorStatus;
  content: string;
  lineSeparator: FileLineSeparator;
  modified: boolean;
  version: WorkspaceFileVersion;
  observedVersion: WorkspaceFileVersion;
  error: string | null;
  /** The last save's identity warning, if the bytes landed but the
   * target's owner, attributes or permissions did not fully follow.
   * Cleared by the next edit or save. */
  saveWarning: string | null;
}

export type FileConflictCallout =
  | { kind: "changed"; canOverwrite: boolean }
  | { kind: "deleted" }
  | { kind: "checkFailed" };

export interface FileEditorFile {
  content: string;
  hasBom: boolean;
  version: WorkspaceFileVersion;
}

export interface FileEditorSession {
  write(input: {
    content: string;
    expectedModifiedAt?: number | null;
    expectedRevision?: string | null;
    create?: boolean;
  }): Promise<WorkspaceFileWriteResult>;
}

export type FileEditorObservation =
  | { status: "ready"; file: FileEditorFile }
  | Extract<WorkspaceFileVersion, { status: "missing" | "error" }>;

export interface FileObservationSource {
  subscribe(listener: () => void): () => void;
  getObservation(): FileEditorObservation | null;
  refresh(): void;
}

type ObservedDiskState =
  | FileEditorObservation
  | { status: "unsettled"; version: WorkspaceFileVersion };

export interface FileEditorClock {
  setTimeout(callback: () => void, delayMs: number): ReturnType<typeof setTimeout>;
  clearTimeout(handle: ReturnType<typeof setTimeout>): void;
}

const systemClock: FileEditorClock = {
  setTimeout(callback, delay) {
    return globalThis.setTimeout(callback, delay);
  },
  clearTimeout(handle) {
    globalThis.clearTimeout(handle);
  },
};

export class FileEditorModel {
  private readonly session: FileEditorSession;
  private readonly clock: FileEditorClock;
  private readonly listeners = new Set<() => void>();
  private snapshot: FileEditorSnapshot;
  private autosave: ReturnType<typeof setTimeout> | null = null;
  private saveSequence = 0;
  private disposed = false;
  private observedWhileSaving: FileEditorObservation | null = null;
  private observed: ObservedDiskState;
  private lastReceivedObservation: FileEditorObservation | null = null;
  private refreshObservation: (() => void) | null = null;
  private reloadRequested = false;
  private persistedContent: string;
  private hasBom: boolean;
  // DIVERGENCE from Paseo (owner decision): a missing file opens as an
  // empty editor and the first save creates it, where Paseo errors with
  // ENOENT. While this is set, a `missing` poll is not a conflict — there
  // is nothing to conflict with yet — and saving attempts a create (a
  // write that names no expected version, which the daemon refuses with
  // `conflict` when anything arrived first). The flag clears on the
  // first landed write; from then on a `missing` poll is a deletion like
  // anywhere else.
  private wasMissingAtOpen: boolean;
  private unsubscribeObservationSource: (() => void) | null = null;

  constructor(input: {
    file: FileEditorFile;
    session: FileEditorSession;
    clock?: FileEditorClock;
    missing?: boolean;
  }) {
    this.session = input.session;
    this.clock = input.clock ?? systemClock;
    this.persistedContent = input.file.content;
    this.hasBom = input.file.hasBom;
    this.wasMissingAtOpen = input.missing ?? false;
    this.observed = { status: "ready", file: input.file };
    this.snapshot = {
      status: "clean",
      content: input.file.content,
      lineSeparator: detectLineSeparator(input.file.content),
      modified: false,
      version: input.file.version,
      observedVersion: input.file.version,
      error: null,
      saveWarning: null,
    };
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = (): FileEditorSnapshot => this.snapshot;

  connectFileObservations(source: FileObservationSource): void {
    this.disconnectFileObservations();
    this.refreshObservation = source.refresh;
    const receiveObservation = () => {
      const observation = source.getObservation();
      if (observation) this.receiveFileObservation(observation);
    };
    this.unsubscribeObservationSource = source.subscribe(receiveObservation);
    receiveObservation();
  }

  disconnectFileObservations(): void {
    this.unsubscribeObservationSource?.();
    this.unsubscribeObservationSource = null;
    this.refreshObservation = null;
  }

  edit(content: string): void {
    if (this.disposed || content === this.snapshot.content) return;
    this.reloadRequested = false;
    // The view joins every line with the first-found separator, so a
    // mixed-ending file would normalise wholesale on the first keystroke.
    // Merge each edited line back onto its persisted terminator instead;
    // added or removed lines keep the joined form (documented fallback).
    const merged = mergeLineEndings(this.persistedContent, content, this.snapshot.lineSeparator);
    const modified = merged !== this.persistedContent;
    let status: FileEditorStatus = modified ? "dirty" : "clean";
    if (this.snapshot.status === "conflict") {
      status = "conflict";
    }
    this.setSnapshot({
      ...this.snapshot,
      status,
      content: merged,
      modified,
      error: null,
      saveWarning: null,
    });
    if (status === "dirty") this.scheduleAutosave();
    else this.clearAutosave();
  }

  async save(): Promise<void> {
    if (this.disposed || (this.snapshot.status !== "dirty" && this.snapshot.status !== "error")) {
      return;
    }
    if (this.snapshot.observedVersion.status !== "ready") {
      // A file that was missing at open and still is: the first save
      // creates it. A file that went missing under an existing edit is
      // a deletion — the conflict road, as before.
      if (
        this.snapshot.observedVersion.status === "missing" &&
        this.wasMissingAtOpen &&
        this.snapshot.modified
      ) {
        await this.performCreate();
        return;
      }
      this.enterConflict(this.snapshot.observedVersion);
      return;
    }
    await this.performWrite(this.snapshot.observedVersion);
  }

  receiveFileObservation(observation: FileEditorObservation): void {
    if (this.disposed || observation === this.lastReceivedObservation) return;
    this.lastReceivedObservation = observation;
    const version = observationVersion(observation);
    this.observed = observation;
    this.setSnapshot({ ...this.snapshot, observedVersion: version });
    if (this.snapshot.status === "saving") {
      this.observedWhileSaving = observation;
      return;
    }
    if (observation.status !== "ready") {
      this.reloadRequested = false;
      this.enterConflict(version);
      return;
    }
    if (this.reloadRequested) {
      this.reloadRequested = false;
      this.applyFile(observation.file);
      return;
    }
    if (observation.file.content === this.persistedContent) {
      this.adoptUnchangedFile(observation.file);
      return;
    }
    if (this.snapshot.status === "clean") {
      this.applyFile(observation.file);
      return;
    }
    this.enterConflict(version);
  }

  async overwrite(): Promise<void> {
    if (this.disposed || this.snapshot.status !== "conflict") return;
    if (this.snapshot.observedVersion.status !== "ready") return;
    await this.performWrite(this.snapshot.observedVersion);
  }

  async reload(): Promise<void> {
    if (this.disposed) return;
    if (this.observed.status !== "ready") {
      this.reloadRequested = true;
      this.refreshObservation?.();
      return;
    }
    this.applyFile(this.observed.file);
  }

  dispose(): void {
    this.disposed = true;
    this.reloadRequested = false;
    this.saveSequence += 1;
    this.clearAutosave();
    this.disconnectFileObservations();
    this.listeners.clear();
  }

  suspendAutosave(): () => void {
    const wasScheduled = this.autosave !== null;
    this.clearAutosave();
    let resumed = false;
    return () => {
      if (resumed || this.disposed) return;
      resumed = true;
      if (wasScheduled && this.snapshot.status === "dirty") this.scheduleAutosave();
    };
  }

  private async performWrite(
    expectedVersion: Extract<WorkspaceFileVersion, { status: "ready" }>,
  ): Promise<void> {
    this.clearAutosave();
    const sequence = ++this.saveSequence;
    const content = this.snapshot.content;
    const hasBom = this.hasBom;
    this.observedWhileSaving = null;
    this.setSnapshot({ ...this.snapshot, status: "saving", error: null, saveWarning: null });
    const serializedContent = hasBom ? String.fromCharCode(0xfeff) + content : content;
    let result: WorkspaceFileWriteResult;
    try {
      result = await this.session.write({
        content: serializedContent,
        expectedModifiedAt: expectedVersion.modifiedAt,
        expectedRevision: expectedVersion.revision ?? null,
      });
    } catch (error) {
      if (this.disposed || sequence !== this.saveSequence) return;
      this.setSnapshot({
        ...this.snapshot,
        status: "error",
        error: error instanceof Error ? error.message : String(error),
        saveWarning: null,
      });
      return;
    }
    if (this.disposed || sequence !== this.saveSequence) return;
    if (result.status === "error") {
      this.setSnapshot({
        ...this.snapshot,
        status: "error",
        error: result.error,
        saveWarning: null,
      });
      return;
    }
    if (result.status === "conflict") {
      this.observed = { status: "unsettled", version: result.version };
      this.enterConflict(result.version);
      return;
    }

    const writtenVersion: Extract<WorkspaceFileVersion, { status: "ready" }> = {
      status: "ready",
      workspaceId: this.snapshot.version.workspaceId,
      path: this.snapshot.version.path,
      size: result.size,
      modifiedAt: result.modifiedAt,
      revision: result.revision,
    };
    const pending = this.takeObservedWhileSaving();
    this.persistedContent = content;
    if (pending && !observationMatchesWrite(pending, content, hasBom)) {
      const pendingVersion = observationVersion(pending);
      this.observed = pending;
      this.setSnapshot({
        ...this.snapshot,
        status: "conflict",
        modified: this.snapshot.content !== this.persistedContent,
        version: writtenVersion,
        observedVersion: pendingVersion,
        error: null,
        saveWarning: result.warning ?? null,
      });
      return;
    }
    const settledVersion = pending?.status === "ready" ? pending.file.version : writtenVersion;
    this.observed = pending ?? { status: "unsettled", version: writtenVersion };
    const modified = this.snapshot.content !== this.persistedContent;
    this.setSnapshot({
      ...this.snapshot,
      status: modified ? "dirty" : "clean",
      modified,
      version: settledVersion,
      observedVersion: settledVersion,
      error: null,
      saveWarning: result.warning ?? null,
    });
    if (modified) this.scheduleAutosave();
  }

  /** The first save of a file that was missing at open: a write that
   * names no expected version. The daemon creates only when still
   * missing — anything that arrived first answers `conflict` with its
   * ready version, and from then on this editor is an ordinary
   * conflicting one (Overwrite works against that version). */
  private async performCreate(): Promise<void> {
    this.clearAutosave();
    const sequence = ++this.saveSequence;
    const content = this.snapshot.content;
    const hasBom = this.hasBom;
    this.observedWhileSaving = null;
    this.setSnapshot({ ...this.snapshot, status: "saving", error: null, saveWarning: null });
    const serializedContent = hasBom ? String.fromCharCode(0xfeff) + content : content;
    let result: WorkspaceFileWriteResult;
    try {
      result = await this.session.write({
        content: serializedContent,
        expectedModifiedAt: null,
        expectedRevision: null,
        create: true,
      });
    } catch (error) {
      if (this.disposed || sequence !== this.saveSequence) return;
      this.setSnapshot({
        ...this.snapshot,
        status: "error",
        error: error instanceof Error ? error.message : String(error),
        saveWarning: null,
      });
      return;
    }
    if (this.disposed || sequence !== this.saveSequence) return;
    if (result.status === "error") {
      this.setSnapshot({
        ...this.snapshot,
        status: "error",
        error: result.error,
        saveWarning: null,
      });
      return;
    }
    if (result.status === "conflict") {
      this.wasMissingAtOpen = false;
      this.observed = { status: "unsettled", version: result.version };
      this.enterConflict(result.version);
      return;
    }
    const writtenVersion: Extract<WorkspaceFileVersion, { status: "ready" }> = {
      status: "ready",
      workspaceId: this.snapshot.version.workspaceId,
      path: this.snapshot.version.path,
      size: result.size,
      modifiedAt: result.modifiedAt,
      revision: result.revision,
    };
    this.wasMissingAtOpen = false;
    this.persistedContent = content;
    this.observed = { status: "unsettled", version: writtenVersion };
    const modified = this.snapshot.content !== this.persistedContent;
    this.setSnapshot({
      ...this.snapshot,
      status: modified ? "dirty" : "clean",
      modified,
      version: writtenVersion,
      observedVersion: writtenVersion,
      error: null,
      saveWarning: result.warning ?? null,
    });
    if (modified) this.scheduleAutosave();
  }

  private applyFile(file: FileEditorFile): void {
    this.clearAutosave();
    this.saveSequence += 1;
    this.persistedContent = file.content;
    this.hasBom = file.hasBom;
    this.observed = { status: "ready", file };
    this.setSnapshot({
      status: "clean",
      content: file.content,
      lineSeparator: detectLineSeparator(file.content),
      modified: false,
      version: file.version,
      observedVersion: file.version,
      error: null,
      saveWarning: null,
    });
  }

  private takeObservedWhileSaving(): FileEditorObservation | null {
    const observation = this.observedWhileSaving;
    this.observedWhileSaving = null;
    return observation;
  }

  private enterConflict(version: WorkspaceFileVersion): void {
    this.clearAutosave();
    this.setSnapshot({
      ...this.snapshot,
      status: "conflict",
      modified: this.snapshot.content !== this.persistedContent,
      observedVersion: version,
      error: version.status === "error" ? version.error : null,
      saveWarning: null,
    });
  }

  private adoptUnchangedFile(file: FileEditorFile): void {
    this.hasBom = file.hasBom;
    this.observed = { status: "ready", file };
    const modified = this.snapshot.content !== this.persistedContent;
    const recovering = this.snapshot.status === "conflict";
    let status = this.snapshot.status;
    if (recovering) status = modified ? "dirty" : "clean";
    this.setSnapshot({
      ...this.snapshot,
      status,
      modified,
      version: file.version,
      observedVersion: file.version,
      error: recovering ? null : this.snapshot.error,
    });
    if (status === "dirty") this.scheduleAutosave();
    else this.clearAutosave();
  }

  private scheduleAutosave(): void {
    this.clearAutosave();
    this.autosave = this.clock.setTimeout(() => {
      this.autosave = null;
      void this.save();
    }, 800);
  }

  private clearAutosave(): void {
    if (!this.autosave) return;
    this.clock.clearTimeout(this.autosave);
    this.autosave = null;
  }

  private setSnapshot(snapshot: FileEditorSnapshot): void {
    this.snapshot = snapshot;
    for (const listener of this.listeners) listener();
  }
}

export function getFileConflictCallout(snapshot: FileEditorSnapshot): FileConflictCallout | null {
  if (snapshot.status !== "conflict") return null;
  switch (snapshot.observedVersion.status) {
    case "ready":
      return { kind: "changed", canOverwrite: snapshot.modified };
    case "missing":
      return { kind: "deleted" };
    case "error":
      return { kind: "checkFailed" };
    default:
      return assertNever(snapshot.observedVersion);
  }
}

function assertNever(value: never): never {
  throw new Error(`Unexpected file version: ${JSON.stringify(value)}`);
}

function detectLineSeparator(content: string): FileLineSeparator {
  for (let index = 0; index < content.length; index += 1) {
    const character = content.charCodeAt(index);
    if (character === 10) return "\n";
    if (character === 13) return content.charCodeAt(index + 1) === 10 ? "\r\n" : "\r";
  }
  return "\n";
}

/** Rejoin edited lines onto persisted terminators, one line each. The
 * view hands back the whole document joined with a single separator, so
 * without this every line but the first style would flip on first edit.
 * When the edit adds or removes lines there is no 1:1 mapping and the
 * joined form stands (the fallback Paseo's whole-document join is). */
export function mergeLineEndings(
  persisted: string,
  edited: string,
  separator: FileLineSeparator,
): string {
  const base = splitLines(persisted);
  const next = edited.split(separator);
  if (next.length !== base.lines.length) return edited;
  return next.map((line, index) => line + (base.endings[index] ?? "")).join("");
}

function splitLines(content: string): { lines: string[]; endings: string[] } {
  const lines: string[] = [];
  const endings: string[] = [];
  const pattern = /\r\n|\r|\n/g;
  let start = 0;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(content)) !== null) {
    lines.push(content.slice(start, match.index));
    endings.push(match[0]);
    start = match.index + match[0].length;
  }
  lines.push(content.slice(start));
  return { lines, endings };
}

function observationVersion(observation: FileEditorObservation): WorkspaceFileVersion {
  return observation.status === "ready" ? observation.file.version : observation;
}

function observationMatchesWrite(
  observation: FileEditorObservation,
  content: string,
  hasBom: boolean,
): boolean {
  return (
    observation.status === "ready" &&
    observation.file.content === content &&
    observation.file.hasBom === hasBom
  );
}
