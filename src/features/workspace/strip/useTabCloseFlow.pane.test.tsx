// @vitest-environment happy-dom

// The pane header's kebab fires through the tab-close flow: the same close
// policy and the same confirmation the tab menu gets, anchored at the pane's
// own session instead of the tab menu's.

import { act, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../../../types/ipc";
import { agentSession, silentAgentSession, terminalSession } from "../bulkCloseHarness";
import type { CloseIntent } from "./closePolicy";
import { useTabCloseFlow } from "./useTabCloseFlow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type Flow = ReturnType<typeof useTabCloseFlow>;
type OnClose = (
  kind: CloseIntent,
  matched: readonly Session[],
  skipped: ReadonlyArray<{ id: string; title: string; generation: number }>,
  onFailed?: (sessionId: string) => void,
) => void;

const SESSIONS: Session[] = [
  agentSession("agent-one", "Agent one"),
  silentAgentSession("agent-two", "Agent two"),
  terminalSession("term-three", "Term three"),
];

function renderFlow(sessions: Session[], onClose: OnClose) {
  const store: { flow: Flow | null } = { flow: null };
  function Probe() {
    const flow = useTabCloseFlow({
      sessions,
      selectedSessionId: "agent-one",
      selection: new Set(),
      onClose,
      selectSession: () => undefined,
      clearSelection: () => undefined,
      addButtonRef: { current: null },
    });
    useEffect(() => {
      store.flow = flow;
    });
    const confirm = flow.confirm;
    return confirm === null ? null : (
      <div data-testid="close-confirm">
        <span data-testid="close-confirm-title">{confirm.title}</span>
        <span data-testid="close-confirm-label">{confirm.confirmLabel}</span>
      </div>
    );
  }
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  return {
    host,
    root,
    async mount() {
      await act(async () => {
        root.render(<Probe />);
      });
    },
    flow: () => {
      if (store.flow === null) throw new Error("close flow did not mount");
      return store.flow;
    },
  };
}

afterEach(async () => {
  document.body.replaceChildren();
});

async function mount(sessions: Session[], onClose: OnClose) {
  const rendered = renderFlow(sessions, onClose);
  await rendered.mount();
  return rendered;
}

describe("activatePaneEntry", () => {
  it("asks the bulk confirm for close-others, anchored at the pane session", async () => {
    const onClose = vi.fn();
    const { host, root, flow } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().activatePaneEntry("agent-two", "others");
    });
    expect(host.querySelector("[data-testid='close-confirm-title']")?.textContent).toBe(
      "Close other tabs?",
    );
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("asks before closing a running agent, like the tab menu", async () => {
    const onClose = vi.fn();
    const { host, root, flow } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().activatePaneEntry("agent-two", "close");
    });
    expect(host.querySelector("[data-testid='close-confirm-title']")?.textContent).toBe(
      "Archive running agent?",
    );
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("fires at once for a session with no process, with no ask", async () => {
    const onClose = vi.fn();
    const dead = agentSession("dead", "Dead", {
      type: "ended",
      generation: 1,
      code: 0,
      integrity: { kind: "complete" },
    });
    const { root, flow } = await mount([dead], onClose);
    await act(async () => {
      flow().activatePaneEntry("dead", "close");
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onClose.mock.calls[0]?.[0]).toBe("archive");
    expect(onClose.mock.calls[0]?.[1]).toHaveLength(1);
    await act(async () => root.unmount());
  });

  it("refuses the keys the header never offers", async () => {
    const onClose = vi.fn();
    const { host, root, flow } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().activatePaneEntry("agent-two", "delete");
    });
    await act(async () => {
      flow().activatePaneEntry("agent-two", "close-selection");
    });
    expect(host.querySelector("[data-testid='close-confirm']")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("leaves the tab menu's own path working", async () => {
    const onClose = vi.fn();
    const { host, root, flow } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().openMenu("agent-one");
    });
    await act(async () => {
      flow().activateEntry("close");
    });
    expect(host.querySelector("[data-testid='close-confirm-title']")?.textContent).toBe(
      "Archive running agent?",
    );
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });
});
