// Ported from Paseo's `packages/app/src/file-pane/editor/model.test.ts`
// (Apache-2.0, Copyright (c) 2025-present Mohamed Boudra): every case kept,
// only the version stamps changed — Paseo's ISO strings are this wire's
// millisecond numbers, and `cwd` is `workspaceId`.

import { describe, expect, test } from "vitest";
import type { WorkspaceFileVersion, WorkspaceFileWriteResult } from "../../../types/ipc";
import {
  FileEditorModel,
  getFileConflictCallout,
  type FileEditorClock,
  type FileEditorFile,
  type FileEditorObservation,
  type FileEditorSession,
  type FileObservationSource,
} from "./model";

class TestClock implements FileEditorClock {
  private callback: (() => void) | null = null;

  setTimeout(callback: () => void): ReturnType<typeof setTimeout> {
    this.callback = callback;
    return 1 as unknown as ReturnType<typeof setTimeout>;
  }

  clearTimeout(): void {
    this.callback = null;
  }

  fire(): void {
    const callback = this.callback;
    this.callback = null;
    callback?.();
  }
}

class FileSession implements FileEditorSession {
  file: FileEditorFile;
  writes: Array<{
    content: string;
    expectedModifiedAt?: number | null;
    expectedRevision?: string | null;
    create?: boolean;
  }> = [];
  nextWrite: WorkspaceFileWriteResult | Error | null = null;
  private pendingWrite: Promise<WorkspaceFileWriteResult> | null = null;
  private resolvePendingWrite: ((result: WorkspaceFileWriteResult) => void) | null = null;

  constructor(file: FileEditorFile) {
    this.file = file;
  }

  async write(input: {
    content: string;
    expectedModifiedAt?: number | null;
    expectedRevision?: string | null;
    create?: boolean;
  }): Promise<WorkspaceFileWriteResult> {
    this.writes.push(input);
    if (this.pendingWrite) return this.pendingWrite;
    if (this.nextWrite instanceof Error) throw this.nextWrite;
    if (this.nextWrite) return this.nextWrite;
    return { status: "written", modifiedAt: 101, size: input.content.length, revision: "4:101" };
  }

  holdNextWrite(): void {
    this.pendingWrite = new Promise((resolve) => {
      this.resolvePendingWrite = resolve;
    });
  }

  finishHeldWrite(result: WorkspaceFileWriteResult): void {
    this.resolvePendingWrite?.(result);
    this.pendingWrite = null;
    this.resolvePendingWrite = null;
  }
}

class ObservationSource implements FileObservationSource {
  observation: FileEditorObservation | null;
  refreshes = 0;
  private readonly listeners = new Set<() => void>();

  constructor(observation: FileEditorObservation | null) {
    this.observation = observation;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  getObservation(): FileEditorObservation | null {
    return this.observation;
  }

  refresh = (): void => {
    this.refreshes += 1;
  };

  emit(observation: FileEditorObservation): void {
    this.observation = observation;
    for (const listener of this.listeners) listener();
  }
}

function ready(modifiedAt = 100, size = 3): Extract<WorkspaceFileVersion, { status: "ready" }> {
  return { status: "ready", workspaceId: "w", path: "file.ts", size, modifiedAt };
}

interface MakeModelInput {
  content?: string;
  hasBom?: boolean;
}

function makeModel(input: MakeModelInput = {}) {
  const file = {
    content: input.content ?? "one",
    hasBom: input.hasBom ?? false,
    version: ready() as Extract<WorkspaceFileVersion, { status: "ready" }>,
  };
  const session = new FileSession(file);
  const clock = new TestClock();
  return { model: new FileEditorModel({ file, session, clock }), session, clock };
}

function observeFile(model: FileEditorModel, file: FileEditorFile): void {
  model.receiveFileObservation({ status: "ready", file });
}

function observeVersion(model: FileEditorModel, version: FileEditorObservation): void {
  model.receiveFileObservation(version);
}

describe("FileEditorModel", () => {
  test("tracks whether the current buffer differs from persisted content", async () => {
    const { model } = makeModel();

    expect(model.getSnapshot().modified).toBe(false);
    model.edit("two");
    expect(model.getSnapshot().modified).toBe(true);
    model.edit("one");
    expect(model.getSnapshot()).toMatchObject({ status: "clean", modified: false });

    model.edit("saved");
    await model.save();
    expect(model.getSnapshot()).toMatchObject({ status: "clean", modified: false });
  });

  test("adopts a precise revision for otherwise unchanged initial metadata", () => {
    const { model } = makeModel();

    observeFile(model, {
      content: "one",
      hasBom: false,
      version: { ...ready(), revision: "precise-revision" },
    });

    expect(model.getSnapshot().observedVersion).toMatchObject({ revision: "precise-revision" });
  });

  test("adopts an unchanged disk revision without disturbing a dirty buffer", async () => {
    const { model, session } = makeModel();
    model.edit("local");
    observeFile(model, {
      content: "one",
      hasBom: false,
      version: { ...ready(), revision: "replacement-revision" },
    });

    expect(model.getSnapshot()).toMatchObject({ status: "dirty", content: "local" });
    await model.save();

    expect(session.writes).toEqual([
      { content: "local", expectedModifiedAt: 100, expectedRevision: "replacement-revision" },
    ]);
  });

  test("adopts an external BOM change before saving a dirty buffer", async () => {
    const { model, session } = makeModel();
    model.edit("local");
    observeFile(model, {
      content: "one",
      hasBom: true,
      version: { ...ready(), revision: "replacement-revision" },
    });

    await model.save();

    expect(session.writes).toEqual([
      {
        content: "\uFEFFlocal",
        expectedModifiedAt: 100,
        expectedRevision: "replacement-revision",
      },
    ]);
  });

  test("conflicts when a same-content observation changes the BOM during a save", async () => {
    const { model, session } = makeModel();
    session.holdNextWrite();
    model.edit("saved");
    const save = model.save();
    observeFile(model, { content: "saved", hasBom: true, version: ready(102, 6) });
    session.finishHeldWrite({ status: "written", modifiedAt: 101, size: 5, revision: "5:101" });

    await save;

    expect(model.getSnapshot()).toMatchObject({
      status: "conflict",
      observedVersion: { modifiedAt: 102 },
    });
  });

  test("ignores a settled observation that was already consumed", () => {
    const { model } = makeModel();
    const observation: FileEditorObservation = {
      status: "ready",
      file: { content: "one", hasBom: false, version: ready() },
    };
    let emissions = 0;
    model.subscribe(() => {
      emissions += 1;
    });
    model.receiveFileObservation(observation);
    const emissionsAfterFirstDelivery = emissions;

    model.receiveFileObservation(observation);

    expect(emissions).toBe(emissionsAfterFirstDelivery);
  });

  test("does not replay the pre-save observation while its refresh is pending", async () => {
    const { model } = makeModel();
    const observation: FileEditorObservation = {
      status: "ready",
      file: { content: "one", hasBom: false, version: ready() },
    };
    model.receiveFileObservation(observation);
    model.edit("saved");
    await model.save();

    model.receiveFileObservation(observation);

    expect(model.getSnapshot()).toMatchObject({ status: "clean", content: "saved" });
  });

  test("does not reload stale bytes after a write conflict", async () => {
    const { model, session } = makeModel();
    session.nextWrite = { status: "conflict", version: ready(102, 8) };
    model.edit("important local work");
    await model.save();

    await model.reload();

    expect(model.getSnapshot()).toMatchObject({
      status: "conflict",
      content: "important local work",
      observedVersion: { modifiedAt: 102 },
    });
  });

  test("reloads a write conflict only after refreshed bytes arrive", async () => {
    const { model, session } = makeModel();
    const source = new ObservationSource({
      status: "ready",
      file: { content: "one", hasBom: false, version: ready() },
    });
    model.connectFileObservations(source);
    session.nextWrite = { status: "conflict", version: ready(102, 4) };
    model.edit("local");
    await model.save();

    await model.reload();
    expect(source.refreshes).toBe(1);
    expect(model.getSnapshot().content).toBe("local");
    source.emit({
      status: "ready",
      file: { content: "disk", hasBom: false, version: ready(102, 4) },
    });

    expect(model.getSnapshot()).toMatchObject({ status: "clean", content: "disk" });
  });

  test("abandons a deferred reload when its refresh fails", async () => {
    const { model, session } = makeModel();
    const source = new ObservationSource({
      status: "ready",
      file: { content: "one", hasBom: false, version: ready() },
    });
    model.connectFileObservations(source);
    session.nextWrite = { status: "conflict", version: ready(102, 4) };
    model.edit("local");
    await model.save();
    await model.reload();
    source.emit({ status: "error", workspaceId: "w", path: "file.ts", error: "File unavailable." });
    model.edit("new local work");

    source.emit({
      status: "ready",
      file: { content: "disk", hasBom: false, version: ready(103, 4) },
    });

    expect(model.getSnapshot()).toMatchObject({ status: "conflict", content: "new local work" });
  });

  test("keeps a newer edit modified when an older save finishes", async () => {
    const { model, session } = makeModel();
    session.holdNextWrite();
    model.edit("saving");

    const save = model.save();
    model.edit("newer edit");
    session.finishHeldWrite({ status: "written", modifiedAt: 101, size: 6, revision: "6:101" });
    await save;

    expect(model.getSnapshot()).toMatchObject({
      status: "dirty",
      content: "newer edit",
      modified: true,
    });
  });

  test("autosaves the latest edit after inactivity", async () => {
    const { model, session, clock } = makeModel();

    model.edit("two");
    model.edit("three");
    clock.fire();
    await Promise.resolve();

    expect(session.writes).toEqual([
      { content: "three", expectedModifiedAt: 100, expectedRevision: null },
    ]);
    expect(model.getSnapshot().status).toBe("clean");
  });

  test("keeps CRLF content in file form", async () => {
    const { model, session } = makeModel({ content: "one\r\ntwo\r\n" });

    expect(model.getSnapshot()).toMatchObject({ content: "one\r\ntwo\r\n", lineSeparator: "\r\n" });
    model.edit("one\r\ntwo\r\nthree\r\n");
    await model.save();

    expect(session.writes).toEqual([
      { content: "one\r\ntwo\r\nthree\r\n", expectedModifiedAt: 100, expectedRevision: null },
    ]);
  });

  test("restores a UTF-8 BOM before writing a CRLF file", async () => {
    const { model, session } = makeModel({ content: "one\r\n", hasBom: true });

    model.edit("saved\r\n");
    await model.save();
    model.edit("saved again\r\n");
    await model.save();

    expect(session.writes).toEqual([
      { content: "\uFEFFsaved\r\n", expectedModifiedAt: 100, expectedRevision: null },
      { content: "\uFEFFsaved again\r\n", expectedModifiedAt: 101, expectedRevision: "4:101" },
    ]);
  });

  test("uses the first line separator when a file mixes styles", () => {
    const { model } = makeModel({ content: "one\r\ntwo\nthree\r" });

    expect(model.getSnapshot().lineSeparator).toBe("\r\n");
  });

  test("keeps each line's own ending on edit", async () => {
    const { model, session } = makeModel({ content: "one\r\ntwo\nthree\r" });

    // The view joins every line with the first-found separator, so the
    // middle and last lines arrive normalised; the model maps them back.
    model.edit("one\r\nTWO\r\nthree\r\n");
    expect(model.getSnapshot().content).toBe("one\r\nTWO\nthree\r");

    await model.save();
    expect(session.writes.at(-1)).toMatchObject({
      content: "one\r\nTWO\nthree\r",
    });
  });

  test("carries the save warning and clears it on edit", async () => {
    const { model, session } = makeModel({ content: "one\n" });
    session.nextWrite = {
      status: "written",
      modifiedAt: 101,
      size: 4,
      revision: "4:101",
      warning: "the file's group could not be preserved",
    };

    model.edit("two\n");
    await model.save();

    // The bytes landed, so the editor is clean — and the shortfall is
    // reported in the snapshot (and the bar) instead of silence.
    expect(model.getSnapshot()).toMatchObject({
      status: "clean",
      saveWarning: "the file's group could not be preserved",
    });

    model.edit("three\n");
    expect(model.getSnapshot().saveWarning).toBeNull();
  });

  test("keeps the joined form when lines are added or removed", async () => {
    const { model, session } = makeModel({ content: "one\r\ntwo\n" });

    model.edit("one\r\nextra\r\ntwo\r\n");
    await model.save();

    // No 1:1 line mapping exists, so the joined form stands (documented).
    expect(session.writes.at(-1)).toMatchObject({
      content: "one\r\nextra\r\ntwo\r\n",
    });
  });

  test("reloads a clean editor when the disk version changes", async () => {
    const { model, session } = makeModel();
    session.file = {
      content: "external",
      hasBom: false,
      version: ready(102, 8) as Extract<WorkspaceFileVersion, { status: "ready" }>,
    };

    observeFile(model, session.file);

    expect(model.getSnapshot()).toMatchObject({ status: "clean", content: "external" });
  });

  test("adopts the format from a clean remote refresh", async () => {
    const { model, session } = makeModel({ content: "local\r\n", hasBom: true });
    session.file = {
      content: "remote\n",
      hasBom: false,
      version: ready(102, 7) as Extract<WorkspaceFileVersion, { status: "ready" }>,
    };

    observeFile(model, session.file);
    expect(model.getSnapshot().lineSeparator).toBe("\n");
    model.edit("saved\n");
    await model.save();

    expect(session.writes.at(-1)).toEqual({
      content: "saved\n",
      expectedModifiedAt: 102,
      expectedRevision: null,
    });
  });

  test("applies consecutive clean disk observations", () => {
    const { model } = makeModel();
    const firstVersion = ready(102, 5);
    const latestVersion = ready(103, 6);

    observeFile(model, { content: "first", hasBom: false, version: firstVersion });
    observeFile(model, { content: "latest", hasBom: false, version: latestVersion });

    expect(model.getSnapshot()).toMatchObject({ status: "clean", content: "latest" });
  });

  test("preserves a dirty buffer and overwrites against the newest disk revision", async () => {
    const { model, session } = makeModel();
    model.edit("local");
    observeFile(model, { content: "disk", hasBom: false, version: ready(102, 4) });

    expect(model.getSnapshot()).toMatchObject({ status: "conflict", content: "local" });
    await model.overwrite();

    expect(session.writes).toEqual([
      { content: "local", expectedModifiedAt: 102, expectedRevision: null },
    ]);
    expect(model.getSnapshot().status).toBe("clean");
  });

  test("keeps the local CRLF and BOM when overwriting a conflict", async () => {
    const { model, session } = makeModel({ content: "one\r\n", hasBom: true });
    model.edit("local\r\n");
    observeFile(model, { content: "disk", hasBom: false, version: ready(102, 4) });

    await model.overwrite();

    expect(session.writes).toEqual([
      { content: "\uFEFFlocal\r\n", expectedModifiedAt: 102, expectedRevision: null },
    ]);
  });

  test("reload discards a conflicted local buffer for the disk contents", async () => {
    const { model, session } = makeModel();
    model.edit("local");
    const diskVersion = ready(102, 4) as Extract<WorkspaceFileVersion, { status: "ready" }>;
    session.file = { content: "disk", hasBom: false, version: diskVersion };
    observeFile(model, session.file);

    await model.reload();

    expect(model.getSnapshot()).toMatchObject({ status: "clean", content: "disk" });
  });

  test("adopts the remote format when reloading a conflict", async () => {
    const { model, session } = makeModel({ content: "one\r\n", hasBom: true });
    model.edit("local\r\n");
    const diskVersion = ready(102, 5) as Extract<WorkspaceFileVersion, { status: "ready" }>;
    session.file = { content: "disk\n", hasBom: false, version: diskVersion };
    observeFile(model, session.file);

    await model.reload();
    model.edit("saved\n");
    await model.save();

    expect(session.writes.at(-1)).toEqual({
      content: "saved\n",
      expectedModifiedAt: 102,
      expectedRevision: null,
    });
  });

  test("reports failed saves without losing the local buffer", async () => {
    const { model, session } = makeModel();
    session.nextWrite = new Error("disk full");
    model.edit("important local work");

    await model.save();

    expect(model.getSnapshot()).toMatchObject({
      status: "error",
      content: "important local work",
      error: "disk full",
    });
  });

  test("a deletion conflicts with local changes and stops autosave", () => {
    const { model, session, clock } = makeModel();
    model.edit("local");
    observeVersion(model, { status: "missing", workspaceId: "w", path: "file.ts" });

    clock.fire();

    expect(model.getSnapshot().status).toBe("conflict");
    expect(session.writes).toEqual([]);
  });

  test("clears a transient check error when the recovered file is unchanged", () => {
    const { model } = makeModel();
    observeVersion(model, {
      status: "error",
      workspaceId: "w",
      path: "file.ts",
      error: "Requested path is not a file",
    });

    observeFile(model, { content: "one", hasBom: false, version: ready() });

    expect(model.getSnapshot()).toMatchObject({
      status: "clean",
      modified: false,
      observedVersion: { status: "ready" },
      error: null,
    });
  });

  test("resumes autosave when a dirty file recovers unchanged from a check error", async () => {
    const { model, session, clock } = makeModel();
    model.edit("local");
    observeVersion(model, {
      status: "error",
      workspaceId: "w",
      path: "file.ts",
      error: "Requested path is not a file",
    });

    observeFile(model, { content: "one", hasBom: false, version: ready() });
    clock.fire();
    await Promise.resolve();

    expect(model.getSnapshot()).toMatchObject({ status: "clean", modified: false });
    expect(session.writes).toEqual([
      { content: "local", expectedModifiedAt: 100, expectedRevision: null },
    ]);
  });

  test("dispose cancels pending autosave", () => {
    const { model, session, clock } = makeModel();
    model.edit("local");

    model.dispose();
    clock.fire();

    expect(session.writes).toEqual([]);
  });

  test("suspends a pending autosave while close confirmation is active", async () => {
    const { model, session, clock } = makeModel();
    model.edit("local");

    const resume = model.suspendAutosave();
    clock.fire();
    expect(session.writes).toEqual([]);

    resume();
    clock.fire();
    await Promise.resolve();
    expect(session.writes).toHaveLength(1);
  });

  test("creates a missing file on the first save", async () => {
    const file = {
      content: "",
      hasBom: false,
      version: { status: "missing", workspaceId: "w", path: "new.ts" } as WorkspaceFileVersion,
    };
    const session = new FileSession(file);
    const clock = new TestClock();
    const model = new FileEditorModel({ file, session, clock, missing: true });

    model.edit("born\n");
    await model.save();

    expect(session.writes).toEqual([
      { content: "born\n", expectedModifiedAt: null, expectedRevision: null, create: true },
    ]);
    expect(model.getSnapshot()).toMatchObject({ status: "clean", modified: false });
    expect(model.getSnapshot().observedVersion.status).toBe("ready");
  });

  test("a create that loses the race conflicts and overwrites cleanly", async () => {
    const file = {
      content: "",
      hasBom: false,
      version: { status: "missing", workspaceId: "w", path: "new.ts" } as WorkspaceFileVersion,
    };
    const session = new FileSession(file);
    const clock = new TestClock();
    const model = new FileEditorModel({ file, session, clock, missing: true });
    session.nextWrite = { status: "conflict", version: ready(102, 4) };

    model.edit("local\n");
    await model.save();

    expect(model.getSnapshot()).toMatchObject({ status: "conflict", content: "local\n" });
    expect(getFileConflictCallout(model.getSnapshot())).toEqual({
      kind: "changed",
      canOverwrite: true,
    });
    await model.overwrite();
    expect(session.writes.at(-1)).toMatchObject({
      content: "local\n",
      expectedModifiedAt: 102,
    });
  });

  test("maps conflict versions to one exhaustive callout state", () => {
    const { model } = makeModel();
    const snapshot = model.getSnapshot();

    expect(getFileConflictCallout(snapshot)).toBeNull();
    expect(
      getFileConflictCallout({
        ...snapshot,
        status: "conflict",
        modified: true,
        observedVersion: ready(101),
      }),
    ).toEqual({ kind: "changed", canOverwrite: true });
    expect(
      getFileConflictCallout({
        ...snapshot,
        status: "conflict",
        observedVersion: { status: "missing", workspaceId: "w", path: "file.ts" },
      }),
    ).toEqual({ kind: "deleted" });
    expect(
      getFileConflictCallout({
        ...snapshot,
        status: "conflict",
        observedVersion: {
          status: "error",
          workspaceId: "w",
          path: "file.ts",
          error: "unreadable",
        },
      }),
    ).toEqual({ kind: "checkFailed" });
  });
});
