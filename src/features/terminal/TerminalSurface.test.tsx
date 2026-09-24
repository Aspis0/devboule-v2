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
  let activeChannel: { onmessage?: (event: unknown) => void } | null = null;
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
      activeChannel = channel ?? null;
      channel?.onmessage?.(snapshotEvent);
      return 17;
    }
    return undefined;
  });
  const emit = (event: unknown): void => activeChannel?.onmessage?.(event);
  return { invoke, emit };
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
  createTerminalView: async (host: HTMLElement) => {
    // What xterm's terminal.open() leaves in the host: the helper textarea
    // the surface's autoFocus prop is expected to focus.
    const helper = document.createElement("textarea");
    helper.className = "xterm-helper-textarea";
    host.appendChild(helper);
    return {
      write: (_data: string, callback?: () => void) => callback?.(),
      applySnapshot: (_snapshot: unknown, callback: () => void) => callback(),
      fit: () => true,
      dispose: () => undefined,
      cols: () => 80,
      rows: () => 24,
    };
  },
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

  it("takes the + menu's focus request: autoFocus focuses the xterm helper once the view is open", async () => {
    const onAutoFocusTaken = vi.fn();
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          autoFocus
          onAutoFocusTaken={onAutoFocusTaken}
        />,
      );
    });
    // start() opens the view (helper textarea appears) and settles on its own.
    await act(async () => flush(400));

    const helper = container.querySelector(".xterm-helper-textarea");
    expect(helper).not.toBeNull();
    expect(document.activeElement).toBe(helper);
    expect(onAutoFocusTaken).toHaveBeenCalledTimes(1);
  });

  it("takes focus when the request arrives after the view is already open (reverse order)", async () => {
    const onAutoFocusTaken = vi.fn();
    root = createRoot(container);
    // start() opens the view first: no request yet, so nothing is focused.
    await act(async () => {
      root!.render(<TerminalSurface workspaceId="w1" sessionId="session-1" />);
    });
    await act(async () => flush(400));
    const helper = container.querySelector(".xterm-helper-textarea");
    expect(helper).not.toBeNull();
    expect(document.activeElement).not.toBe(helper);

    // The + menu's request lands only now — the prop-change path must take it.
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          autoFocus
          onAutoFocusTaken={onAutoFocusTaken}
        />,
      );
    });

    expect(document.activeElement).toBe(helper);
    expect(onAutoFocusTaken).toHaveBeenCalledTimes(1);
  });

  it("drops the request when the user has moved focus before it is armed", async () => {
    const onAutoFocusTaken = vi.fn();
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    const guard = () => document.activeElement === null || document.activeElement === document.body;
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          autoFocusGuard={guard}
          onAutoFocusTaken={onAutoFocusTaken}
        />,
      );
    });
    await act(async () => flush(400));
    const helper = container.querySelector(".xterm-helper-textarea");
    expect(helper).not.toBeNull();

    // The user clicks elsewhere while startup is pending; THEN the request is
    // armed. Focus must stay where the user put it, and the request is spent.
    await act(async () => {
      outside.focus();
    });
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          autoFocus
          autoFocusGuard={guard}
          onAutoFocusTaken={onAutoFocusTaken}
        />,
      );
    });

    expect(document.activeElement).toBe(outside);
    expect(document.activeElement).not.toBe(helper);
    expect(onAutoFocusTaken).toHaveBeenCalledTimes(1);
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

  it("reattaches when a recovered terminal is reopened into a new generation", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          observedState={{
            type: "recovered",
            generation: 2,
            integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
          }}
        />,
      );
    });
    await act(async () => undefined);
    await act(async () => {
      coreMocks.emit({
        type: "recovered",
        integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
      });
    });

    expect(container.querySelector(".workspace-terminal-banner")?.textContent).toContain(
      "The previous terminal process is gone.",
    );
    expect(
      container.querySelector<HTMLButtonElement>(".workspace-terminal-interrupt")?.disabled,
    ).toBe(true);

    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          observedState={{ type: "live", generation: 3 }}
        />,
      );
    });
    await act(async () => undefined);

    expect(
      vi.mocked(invoke).mock.calls.filter(([command]) => command === "session_attach"),
    ).toHaveLength(2);
    expect(container.querySelector(".workspace-terminal-banner")).toBeNull();
    expect(container.querySelector(".workspace-terminal-status")?.textContent).toBe(
      "Connected to the local shell",
    );
    expect(
      container.querySelector<HTMLButtonElement>(".workspace-terminal-interrupt")?.disabled,
    ).toBe(false);
  });
});

describe("a recovered terminal states its ended state once", () => {
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

  it("says the sentence once in the pane banner, with the close-tab action and no Ctrl+C", async () => {
    // A recovered row's attach is refused: the controller produces the ended
    // banner (terminalSession.ts), which must be the pane's only telling.
    vi.mocked(invoke).mockImplementationOnce(async (command: string) => {
      if (command === "session_attach") {
        throw { code: "internal", message: "session attachment is not registered" };
      }
      return undefined;
    });
    const onCloseTab = vi.fn();
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          observedState={{
            type: "recovered",
            generation: 2,
            integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
          }}
          onCloseTab={onCloseTab}
        />,
      );
    });
    await act(async () => flush(100));

    const sentence =
      "This terminal ended with the previous daemon and cannot be reopened — close the tab or open a new one.";
    const banner = container.querySelector(".workspace-terminal-banner");
    expect(banner?.textContent).toContain(sentence);
    // Once in the whole pane: the header status carries no second copy.
    const pane = container.querySelector(".workspace-terminal-shell");
    expect((pane?.textContent?.split(sentence).length ?? 0) - 1).toBe(1);
    expect(container.querySelector(".workspace-terminal-status")?.textContent).toBe("");

    // The dot takes the tab chip's tone for a recovered row — never green.
    const dot = container.querySelector(".workspace-status-dot");
    expect(dot?.className).toContain("workspace-dot-border");
    expect(dot?.className).not.toContain("workspace-dot-green");

    // No interrupt control on a terminal that has ended.
    expect(container.querySelector(".workspace-terminal-interrupt")).toBeNull();

    // The sentence's action, and the raw attach text as reachable detail.
    expect(banner?.getAttribute("title")).toBe("session attachment is not registered");
    const closeTab = container.querySelector<HTMLButtonElement>(
      ".workspace-terminal-banner-action",
    );
    expect(closeTab?.textContent).toBe("Close tab");
    if (closeTab === null) throw new Error("the close-tab action did not render");
    await act(async () => closeTab.click());
    expect(onCloseTab).toHaveBeenCalledTimes(1);
  });
});
