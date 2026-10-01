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
import { composeStripTabs } from "./toolTabs";
import { useTabCloseFlow } from "./useTabCloseFlow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type Flow = ReturnType<typeof useTabCloseFlow>;
type OnClose = (
  kind: CloseIntent,
  matched: readonly Session[],
  onFailed?: (sessionId: string) => void,
) => void;

const SESSIONS: Session[] = [
  agentSession("agent-one", "Agent one"),
  silentAgentSession("agent-two", "Agent two"),
  terminalSession("term-three", "Term three"),
];

function renderFlow(
  sessions: Session[],
  onClose: OnClose,
  selection: ReadonlySet<string> = new Set<string>(),
) {
  const onCloseTabs = vi.fn();
  const store: { flow: Flow | null } = { flow: null };
  function Probe() {
    const flow = useTabCloseFlow({
      sessions,
      tabs: composeStripTabs(sessions, []),
      activeTabId: "agent-one",
      selection,
      onClose,
      onCloseTabs: onCloseTabs,
      onCloseTools: () => undefined,
      selectTab: () => undefined,
      clearSelection: () => undefined,
      addButtonRef: { current: null },
      // The explicit no-op: this test has no rename half to wire, and the
      // required arg makes that a decision rather than a default.
      renameMenu: { entriesFor: () => [], open: () => undefined },
    });
    useEffect(() => {
      store.flow = flow;
    });
    const confirm = flow.confirm;
    return confirm === null ? null : (
      <div data-testid="close-confirm">
        <span data-testid="close-confirm-title">{confirm.title}</span>
        <span data-testid="close-confirm-label">{confirm.confirmLabel}</span>
        <span data-testid="close-confirm-tone">{confirm.tone}</span>
        <span data-testid="close-confirm-targets">
          {confirm.targets.map((target) => target.id).join(" ")}
        </span>
      </div>
    );
  }
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  return {
    host,
    root,
    onCloseTabs,
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

async function mount(sessions: Session[], onClose: OnClose, selection?: ReadonlySet<string>) {
  const rendered = renderFlow(sessions, onClose, selection);
  await rendered.mount();
  return rendered;
}

describe("activatePaneEntry", () => {
  it("removes other tabs locally, anchored at the pane session", async () => {
    const onClose = vi.fn();
    const { host, root, flow, onCloseTabs } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().activatePaneEntry("agent-two", "others");
    });
    expect(host.querySelector("[data-testid='close-confirm-title']")).toBeNull();
    expect(onCloseTabs).toHaveBeenCalledWith(["agent-one", "term-three"]);
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
    // Every ask the flow raises is a destructive act: the sole affirmative is
    // the filled danger.
    expect(host.querySelector("[data-testid='close-confirm-tone']")?.textContent).toBe("danger");
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => {
      root.unmount();
    });
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

  it("ignores delete, selection and copy keys in the pane close dispatcher", async () => {
    const onClose = vi.fn();
    const { host, root, flow } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().activatePaneEntry("agent-two", "delete");
    });
    await act(async () => {
      flow().activatePaneEntry("agent-two", "close-selection");
      flow().activatePaneEntry("agent-two", "copy-session-id");
      flow().activatePaneEntry("agent-two", "copy-path");
    });
    expect(host.querySelector("[data-testid='close-confirm']")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(flow().confirm).toBeNull();
    await act(async () => root.unmount());
  });

  it("removes a tab locally through its tab menu", async () => {
    const onClose = vi.fn();
    const { host, root, flow, onCloseTabs } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().openMenu("agent-one");
    });
    await act(async () => {
      flow().activateEntry("close");
    });
    expect(host.querySelector("[data-testid='close-confirm-title']")).toBeNull();
    expect(onCloseTabs).toHaveBeenCalledWith(["agent-one"]);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("keeps copy keys out of the close dispatcher", async () => {
    const onClose = vi.fn();
    const { root, flow, onCloseTabs } = await mount(SESSIONS, onClose);
    await act(async () => flow().openMenu("agent-one"));
    await act(async () => {
      flow().activateEntry("copy-session-id");
      flow().activateEntry("copy-path");
    });
    expect(flow().copyEntryValue("copy-session-id")).toBe("agent-one");
    expect(flow().menu).not.toBeNull();
    expect(flow().confirm).toBeNull();
    expect(onCloseTabs).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("opens the delete confirm from the tab menu's Delete", async () => {
    const onClose = vi.fn();
    const { host, root, flow } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().openMenu("agent-one");
    });
    await act(async () => {
      flow().activateEntry("delete");
    });
    expect(host.querySelector("[data-testid='close-confirm-title']")?.textContent).toContain(
      "Delete",
    );
    expect(host.querySelector("[data-testid='close-confirm-tone']")?.textContent).toBe("danger");
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => {
      root.unmount();
    });
  });

  it("removes the selection locally from its menu", async () => {
    const onClose = vi.fn();
    const { host, root, flow, onCloseTabs } = await mount(
      SESSIONS,
      onClose,
      new Set(["agent-one", "agent-two"]),
    );
    await act(async () => {
      flow().openMenu("agent-one");
    });
    await act(async () => {
      flow().activateEntry("close-selection");
    });
    expect(host.querySelector("[data-testid='close-confirm-title']")).toBeNull();
    expect(onCloseTabs).toHaveBeenCalledWith(["agent-one", "agent-two"]);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("targets the tabs after the anchor for close-to-the-right", async () => {
    const onClose = vi.fn();
    const { host, root, flow, onCloseTabs } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().activatePaneEntry("agent-two", "right");
    });
    expect(host.querySelector("[data-testid='close-confirm-title']")).toBeNull();
    expect(onCloseTabs).toHaveBeenCalledWith(["term-three"]);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("does nothing for an anchor the roster does not know", async () => {
    const onClose = vi.fn();
    const { host, root, flow } = await mount(SESSIONS, onClose);
    await act(async () => {
      flow().activatePaneEntry("gone", "right");
    });
    await act(async () => {
      flow().activatePaneEntry("gone", "close");
    });
    expect(host.querySelector("[data-testid='close-confirm']")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });
});
