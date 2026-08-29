import { describe, expect, it } from "vitest";
import type { OracleIndexStatus, OracleWorkspace } from "../../types/ipc";
import { getOracleErrorAction, getOracleStage, isIndexEmpty } from "./oracleUtils";

const workspace: OracleWorkspace = {
  path: "C:/code/project",
  source: "saved",
  exists: true,
  editable: true,
};

function makeStatus(overrides: Partial<OracleIndexStatus> = {}): OracleIndexStatus {
  return {
    state: "idle",
    indexed_files: 0,
    total_files: 12,
    indexed_chunks: 0,
    pending_files: 12,
    stale_files: 0,
    resource_budget: {
      max_cpu_percent: 50,
      max_memory_mb: 512,
      max_parallelism: 2,
    },
    model: {
      state: "ready",
      model_id: "embedding",
      directory: "models/embedding",
      file: null,
      file_index: 0,
      total_files: 1,
      bytes_done: 34_000_000,
      bytes_total: 34_000_000,
      approximate_bytes: 34_000_000,
      message: null,
    },
    reranker: {
      state: "ready",
      model_id: "reranker",
      directory: "models/reranker",
      file: null,
      file_index: 0,
      total_files: 1,
      bytes_done: 5_000_000,
      bytes_total: 5_000_000,
      approximate_bytes: 5_000_000,
      message: null,
    },
    ...overrides,
  };
}

describe("Oracle stage routing", () => {
  it("keeps setup requests out of the UI before a folder is chosen", () => {
    expect(
      getOracleStage({
        workspaceRequest: { status: "ready", value: { ...workspace, exists: false, path: null } },
        statusRequest: { status: "idle" },
      }),
    ).toBe("choose-workspace");
  });

  it("makes model download the dominant step", () => {
    expect(
      getOracleStage({
        workspaceRequest: { status: "ready", value: workspace },
        statusRequest: {
          status: "ready",
          value: makeStatus({
            model: { ...makeStatus().model, state: "downloading" },
          }),
        },
      }),
    ).toBe("models");
  });

  it("allows dense search but keeps a failed reranker out of the blocking path", () => {
    expect(
      getOracleStage({
        workspaceRequest: { status: "ready", value: workspace },
        statusRequest: {
          status: "ready",
          value: makeStatus({
            indexed_files: 4,
            indexed_chunks: 9,
            pending_files: 0,
            reranker: { ...makeStatus().reranker!, state: "failed", message: "offline" },
          }),
        },
      }),
    ).toBe("ready");
  });

  it("keeps a cancelled partial index useful without presenting it as complete", () => {
    expect(
      getOracleStage({
        workspaceRequest: { status: "ready", value: workspace },
        statusRequest: {
          status: "ready",
          value: makeStatus({
            indexed_files: 80,
            total_files: 200,
            indexed_chunks: 160,
            pending_files: 120,
            state: "idle",
          }),
        },
      }),
    ).toBe("incomplete");
  });

  it("shows a memory-paused worker on the partial-index surface", () => {
    expect(
      getOracleStage({
        workspaceRequest: { status: "ready", value: workspace },
        statusRequest: {
          status: "ready",
          value: makeStatus({
            state: "indexing",
            indexed_files: 4,
            total_files: 12,
            indexed_chunks: 9,
            pending_files: 8,
            pause_reason: "available memory is low",
          }),
        },
      }),
    ).toBe("incomplete");
  });

  it("sends a failed status request to folder recovery first", () => {
    expect(
      getOracleErrorAction({
        statusRequest: {
          status: "error",
          message: "Oracle workspace C:/code/project no longer exists",
        },
        status: null,
      }),
    ).toBe("choose-workspace");
    expect(
      getOracleErrorAction({
        statusRequest: { status: "error", message: "reading Oracle index status failed" },
        status: null,
      }),
    ).toBe("retry-status");
    expect(
      getOracleErrorAction({
        statusRequest: { status: "ready", value: makeStatus({ state: "error" }) },
        status: makeStatus({ state: "error" }),
      }),
    ).toBe("retry-index");
  });

  it("does not confuse an empty index with a populated one", () => {
    const status = makeStatus();
    expect(isIndexEmpty(null, status)).toBe(true);
    expect(
      isIndexEmpty(
        {
          indexed_files: 4,
          indexed_chunks: 9,
          pending_files: 0,
          stale_files: 0,
          backend: "sqlite",
        },
        status,
      ),
    ).toBe(false);
  });
});
