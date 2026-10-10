// @vitest-environment happy-dom

// The close-flow contract: mounted editors register, dirty ones flush on
// demand, and the flush event answers with exactly what is still unsaved
// (conflicted-with-edits and saves that did not land included).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { FileEditorModel } from "./model";
import {
  flushEditors,
  installEditorFlush,
  registerEditor,
  unsavedEditorLabels,
} from "./editorRegistry";
import type { WorkspaceFileWriteResult } from "../../../types/ipc";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => {
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  return {
    listen: (event: string, handler: (event: { payload: unknown }) => void) => {
      listeners.set(event, handler);
      return Promise.resolve(() => {
        listeners.delete(event);
      });
    },
    __listeners: listeners,
  };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function ready(version = {}) {
  return {
    status: "ready" as const,
    workspaceId: "w",
    path: "a.txt",
    size: 4,
    modifiedAt: 100,
    revision: "4:100",
    ...version,
  };
}

function modelWith(content: string, writes: string[]) {
  const file = { content, hasBom: false, version: ready() };
  const session = {
    write: async (input: { content: string }): Promise<WorkspaceFileWriteResult> => {
      writes.push(input.content);
      return {
        status: "written" as const,
        modifiedAt: 101,
        size: input.content.length,
        revision: "x",
      };
    },
  };
  return new FileEditorModel({ file, session });
}

describe("editorRegistry", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockResolvedValue(undefined as never);
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("reports nothing unsaved when every editor is clean", () => {
    const writes: string[] = [];
    const release = registerEditor(modelWith("one\n", writes), "a.txt");
    try {
      expect(unsavedEditorLabels()).toEqual([]);
    } finally {
      release();
    }
    expect(unsavedEditorLabels()).toEqual([]);
  });

  it("flushes dirty editors and reports what is still unsaved", async () => {
    const writes: string[] = [];
    const clean = modelWith("one\n", writes);
    const dirty = modelWith("one\n", writes);
    dirty.edit("two\n");
    const conflicted = modelWith("one\n", writes);
    conflicted.edit("local\n");
    // A disk move under local edits: conflict with modifications.
    conflicted.receiveFileObservation({
      status: "ready",
      file: { content: "disk\n", hasBom: false, version: ready({ modifiedAt: 102 }) },
    });
    const releases = [
      registerEditor(clean, "clean.txt"),
      registerEditor(dirty, "dirty.txt"),
      registerEditor(conflicted, "conflict.txt"),
    ];
    try {
      expect(unsavedEditorLabels()).toEqual(["dirty.txt", "conflict.txt"]);

      const unsaved = await flushEditors(3000);
      expect(writes).toEqual(["two\n"]);
      // The dirty one landed; the conflicted one has no safe automatic
      // write, so the close question must name it.
      expect(unsaved).toEqual(["conflict.txt"]);
    } finally {
      releases.forEach((release) => release());
    }
  });

  it("answers the close flow's flush request with the unsaved labels", async () => {
    const writes: string[] = [];
    const dirty = modelWith("one\n", writes);
    dirty.edit("two\n");
    const release = registerEditor(dirty, "dirty.txt");
    try {
      const unlisten = await installEditorFlush();
      const { __listeners } = (await import("@tauri-apps/api/event")) as unknown as {
        __listeners: Map<string, (event: { payload: unknown }) => void>;
      };
      const flush = __listeners.get("devboule:flush-editors");
      if (!flush) throw new Error("the flush listener did not install");
      flush({ payload: { nonce: 7 } });
      // The handler flushes (dirty saves) then answers; poll for the
      // answer instead of counting microtasks.
      const deadline = Date.now() + 1000;
      while (
        !vi.mocked(invoke).mock.calls.some(([command]) => command === "editors_flushed") &&
        Date.now() < deadline
      ) {
        await new Promise((resolve) => setTimeout(resolve, 5));
      }

      expect(invoke).toHaveBeenCalledWith("editors_flushed", { nonce: 7, unsaved: [] });
      expect(writes).toEqual(["two\n"]);
      unlisten();
    } finally {
      release();
    }
  });
});
