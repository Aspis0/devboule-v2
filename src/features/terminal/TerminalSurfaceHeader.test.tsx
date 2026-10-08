// @vitest-environment happy-dom

// The terminal header reads its title from the session, never from a literal:
// `sessionTitle` already prefers a display name, so naming a session renames
// this header with zero further changes. Workspace passes the close seam but
// not the title yet, so the literal stays the default.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TerminalSurface } from "./TerminalSurface";
import { localWorkspaceKey, type WorkspaceKey } from "../workspace/hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

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
  observe(): void {}
  disconnect(): void {}
}

describe("terminal pane header", () => {
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
          workspaceKey={keyFor("w1")}
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
          workspaceKey={keyFor("w1")}
          sessionId="session-1"
          observedState={{ type: "live", generation: 1 }}
        />,
      );
    });
    await act(async () => undefined);
    expect(container.querySelector(".workspace-terminal-title")?.textContent).toBe("Terminal");
  });

  it("renders the surface's session ID copy row without cwd or close actions", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceKey={keyFor("w1")}
          sessionId="session-1"
          title="Human title"
          observedState={{ type: "live", generation: 1 }}
        />,
      );
    });
    const kebab = container.querySelector<HTMLButtonElement>(".pane-header-kebab");
    expect(kebab).not.toBeNull();
    await act(async () => kebab!.click());
    expect(
      [...document.querySelectorAll('.pane-header-menu [role="menuitem"]')].map(
        (row) => row.textContent,
      ),
    ).toEqual(["Copy session ID"]);
  });

  it("reads Failed when the terminal's own start fails over a live row", async () => {
    vi.mocked(invoke).mockImplementationOnce(async (command: string) => {
      if (command === "session_attach") {
        throw { code: "internal", message: "boom" };
      }
      return undefined;
    });
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceKey={keyFor("w1")}
          sessionId="session-1"
          observedState={{ type: "live", generation: 1 }}
        />,
      );
    });
    await act(async () => undefined);
    expect(container.querySelector(".workspace-terminal-status")?.childNodes[0]?.textContent).toBe(
      "Failed",
    );
    expect(container.querySelector(".workspace-status-dot")?.className).toContain(
      "workspace-dot-failed",
    );
  });

  it("reads Quiet with the sentence in the tooltip and the accessible name when silent", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <TerminalSurface
          workspaceKey={keyFor("w1")}
          sessionId="session-1"
          observedState={{ type: "silent", generation: 1 }}
        />,
      );
    });
    await act(async () => undefined);
    const status = container.querySelector(".workspace-terminal-status");
    expect(status?.childNodes[0]?.textContent).toBe("Quiet");
    expect(status?.getAttribute("title")).toContain("may still be working");
    expect(status?.getAttribute("aria-label")).toBeNull();
    expect(status?.querySelector(".sr-only")?.textContent).toContain("may still be working");
  });
});
