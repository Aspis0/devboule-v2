// @vitest-environment happy-dom

// The terminal header reads its title from the session, never from a literal:
// `sessionTitle` already prefers a display name, so naming a session renames
// this header with zero further changes. Workspace does not pass the title
// yet (it is untouched by this slice), so the literal stays the default.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TerminalSurface } from "../../terminal/TerminalSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const coreMocks = vi.hoisted(() => {
  const invoke = vi.fn(async (command: string) => {
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

vi.mock("../../terminal/createTerminalView", () => ({
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
  observe(): void {}
  disconnect(): void {}
}

describe("terminal pane header title", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot> | null = null;

  beforeEach(() => {
    globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(invoke).mockClear();
  });

  afterEach(async () => {
    if (root !== null) {
      await act(async () => root?.unmount());
      root = null;
    }
    document.body.replaceChildren();
  });

  it("shows the session title instead of the literal", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          title="zsh · api"
          observedState={{ type: "live", generation: 1 }}
        />,
      );
    });
    await act(async () => undefined);
    expect(container.querySelector(".workspace-terminal-title")?.textContent).toBe("zsh · api");
  });

  it("falls back to the literal until the workspace passes a title", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceId="w1"
          sessionId="session-1"
          observedState={{ type: "live", generation: 1 }}
        />,
      );
    });
    await act(async () => undefined);
    expect(container.querySelector(".workspace-terminal-title")?.textContent).toBe("Terminal");
  });
});
