// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { bannerText, TerminalSurface } from "./TerminalSurface";

const coreMocks = vi.hoisted(() => {
  const snapshotEvent = {
    type: "snapshot",
    asOfSeq: 0,
    cols: 80,
    rows: 24,
    data: "",
    cursor: { row: 0, col: 0, visible: true, shape: "block", blinking: false },
    alternateScreen: false,
    bracketedPaste: false,
    lineWrap: true,
  };
  const invoke = vi.fn(async (command: string, args?: Record<string, unknown>) => {
    if (command === "sessions_list") return [];
    if (command === "session_create") {
      return {
        id: "session-1",
        workspaceId: "w1",
        kind: "terminal",
        title: "Terminal",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
      };
    }
    if (command === "session_attach") {
      // The daemon already streams over the channel before the attach confirms.
      const channel = args?.ch as { onmessage?: (event: unknown) => void } | undefined;
      channel?.onmessage?.(snapshotEvent);
      return 17;
    }
    return undefined;
  });
  return { invoke };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: coreMocks.invoke,
  Channel: class {
    onmessage: (event: unknown) => void;
    constructor(onmessage: (event: unknown) => void) {
      this.onmessage = onmessage;
    }
  },
}));

vi.mock("./createTerminalView", () => ({
  createTerminalView: async () => ({
    write: (_data: string, callback?: () => void) => callback?.(),
    applySnapshot: (_snapshot: unknown, callback: () => void) => callback(),
    fit: () => true,
    dispose: () => undefined,
    cols: () => 80,
    rows: () => 24,
  }),
}));

class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  private readonly callback: (entries: unknown[]) => void;
  constructor(callback: (entries: unknown[]) => void) {
    this.callback = callback;
    ResizeObserverStub.instances.push(this);
  }
  observe(): void {}
  disconnect(): void {}
  fire(): void {
    this.callback([]);
  }
}

const flush = (milliseconds: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, milliseconds));

describe("terminal integrity banner copy", () => {
  it("renders the zero-byte recovered warning verbatim", () => {
    expect(
      bannerText({
        kind: "recovered",
        integrity: {
          kind: "unverifiable",
          droppedFrames: 0,
          droppedBytes: 0,
          trimmedBytes: 0,
        },
      }),
    ).toBe(
      "The previous terminal process is gone. The end of the saved transcript could not be verified.",
    );
  });

  it("renders the measured recovered warning verbatim", () => {
    expect(
      bannerText({
        kind: "recovered",
        integrity: {
          kind: "unverifiable",
          droppedFrames: 2,
          droppedBytes: 12 * 1024,
          trimmedBytes: 0,
        },
      }),
    ).toBe(
      "The previous terminal process is gone. At least 12 KB of output was not saved, and the end of the transcript could not be verified either.",
    );
  });

  it("renders the measured exited warning verbatim", () => {
    expect(
      bannerText({
        kind: "exited",
        code: 7,
        lost: { frames: 2, bytes: 12 * 1024 },
        trimmedBytes: 0,
      }),
    ).toBe("The terminal process exited with code 7. At least 12 KB of output was not saved.");
  });

  it("renders the unknown-amount exited warning verbatim", () => {
    expect(
      bannerText({
        kind: "exited",
        code: 7,
        lost: { frames: 2, bytes: 0 },
        trimmedBytes: 0,
      }),
    ).toBe("The terminal process exited with code 7. Some output was not saved.");
  });

  it("renders the live journal degradation warning verbatim", () => {
    expect(bannerText({ kind: "journal_degraded", lost: { frames: 2, bytes: 12 * 1024 } })).toBe(
      "Scrollback history is incomplete: at least 12 KB of output could not be saved.",
    );
  });

  it("keeps the exited copy shape when no exit code was observed", () => {
    expect(
      bannerText({
        kind: "exited",
        code: null,
        lost: { frames: 2, bytes: 12 * 1024 },
        trimmedBytes: 0,
      }),
    ).toBe("The terminal process exited. At least 12 KB of output was not saved.");
  });

  it("renders the trimmed-only warning verbatim", () => {
    expect(bannerText({ kind: "exited", code: 0, lost: null, trimmedBytes: 12 * 1024 })).toBe(
      "The oldest 12 KB of this transcript was removed by the history limit.",
    );
  });

  it("renders the trimmed-and-lost warning verbatim", () => {
    expect(
      bannerText({
        kind: "exited",
        code: 0,
        lost: { frames: 2, bytes: 8 * 1024 },
        trimmedBytes: 12 * 1024,
      }),
    ).toBe(
      "The oldest 12 KB was removed by the history limit, and at least 8.2 KB of output was not saved.",
    );
  });
});

describe("TerminalSurface observer wiring", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot> | null = null;

  beforeEach(() => {
    globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
    container = document.createElement("div");
    document.body.appendChild(container);
    ResizeObserverStub.instances = [];
    vi.mocked(invoke).mockClear();
  });

  afterEach(async () => {
    if (root !== null) {
      await act(async () => root?.unmount());
      root = null;
    }
    document.body.replaceChildren();
  });

  it("resizes through the replacement session after the controller is recreated", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface workspaceId="w1" sessionId="session-1" onPermissionRequest={vi.fn()} />,
      );
    });
    // Let the first controller attach, consume its snapshot, and finish its
    // own debounced resize before the replacement.
    await act(async () => flush(350));

    // Same workspace/session props: only the controller effect re-runs, so a
    // stale observer closure would still hold the disposed first session.
    await act(async () => {
      root!.render(
        <TerminalSurface workspaceId="w1" sessionId="session-1" onPermissionRequest={vi.fn()} />,
      );
    });
    await act(async () => flush(350));

    const callsBeforeFire = vi.mocked(invoke).mock.calls.length;
    ResizeObserverStub.instances.at(-1)?.fire();
    await act(async () => flush(250));

    expect(vi.mocked(invoke).mock.calls.slice(callsBeforeFire)).toContainEqual([
      "session_resize",
      expect.objectContaining({ id: "session-1", subscriptionId: 17, cols: 80, rows: 24 }),
    ]);
  });
});
