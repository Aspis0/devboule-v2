// @vitest-environment happy-dom

// The rename half of the tab menu, in the hook that owns it
// (useSessionRename.ts) and in the close flow that splices its entry ahead
// of the close group and routes the menu's and the pane header kebab's
// rename key to its open — with focus already on the anchor the dialog
// returns to.

import { act, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import {
  agentSession,
  endedAgentSession,
  recoveredAgentSession,
  terminalSession,
} from "../bulkCloseHarness";
import { useSessionRename } from "./useSessionRename";
import { composeStripTabs } from "./toolTabs";
import { useTabCloseFlow } from "./useTabCloseFlow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type Flow = ReturnType<typeof useTabCloseFlow>;
type Rename = ReturnType<typeof useSessionRename>;

function renderFlow(
  sessions: Session[],
  renameSupported: boolean,
  selection: ReadonlySet<string> = new Set<string>(),
) {
  const store: { flow: Flow | null; rename: Rename | null } = { flow: null, rename: null };
  function Probe({ sessions }: { sessions: Session[] }) {
    const rename = useSessionRename({ sessions, renameSupported });
    const flow = useTabCloseFlow({
      sessions,
      tabs: composeStripTabs(sessions, []),
      activeTabId: "agent-one",
      selection,
      onClose: () => undefined,
      onCloseTabs: () => undefined,
      onCloseTools: () => undefined,
      selectTab: () => undefined,
      clearSelection: () => undefined,
      addButtonRef: { current: null },
      renameMenu: { entriesFor: rename.renameEntriesFor, open: rename.openRename },
    });
    useEffect(() => {
      store.flow = flow;
      store.rename = rename;
    });
    const confirm = flow.confirm;
    return confirm === null ? null : <div data-testid="close-confirm" />;
  }
  const host = document.createElement("div");
  document.body.appendChild(host);
  // The tab element the flow's focus restore looks up by id, outside the
  // React root so the render does not wipe it.
  host.innerHTML = `<button id="workspace-session-tab-agent-one" type="button">agent one</button><div id="probe"></div>`;
  const probeHost = host.querySelector<HTMLDivElement>("#probe");
  if (probeHost === null) throw new Error("probe host did not render");
  const root = createRoot(probeHost);
  return {
    host,
    root,
    flow: () => {
      if (store.flow === null) throw new Error("close flow did not mount");
      return store.flow;
    },
    rename: () => {
      if (store.rename === null) throw new Error("rename hook did not mount");
      return store.rename;
    },
    // Moves the world under an open surface: the caller wraps this in act.
    setSessions: (next: Session[]) => {
      root.render(<Probe sessions={next} />);
    },
    async mount() {
      await act(async () => {
        root.render(<Probe sessions={sessions} />);
      });
    },
    async unmount() {
      await act(async () => {
        root.unmount();
      });
      host.remove();
    },
  };
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("the tab menu's rename entry", () => {
  it("offers Rename ahead of close actions on an agent with the capability", async () => {
    const flow = renderFlow([agentSession("agent-one", "Agent one")], true);
    await flow.mount();
    await act(async () => flow.flow().openMenu("agent-one"));
    const entries = flow.flow().menu?.entries ?? [];
    expect(entries.map((entry) => entry.label)).toEqual([
      "Copy session ID",
      "Copy branch name",
      "Rename",
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
      "Archive",
      "Delete",
    ]);
    const rename = entries.find((entry) => entry.key === "rename");
    expect(rename?.separatorAfter).toBe(true);
    expect(rename?.disabled).toBe(false);
    await flow.unmount();
  });

  it("hides Rename on a terminal even with the capability", async () => {
    // Every negative case carries its own control first: a supported agent
    // tab DOES get the row, so the absence below is the builder's answer and
    // not a menu that never had a rename half.
    const sessions = [
      agentSession("agent-one", "Agent one"),
      terminalSession("term-three", "shell three"),
    ];
    const flow = renderFlow(sessions, true);
    await flow.mount();
    flow.flow().openMenu("agent-one");
    await act(async () => {});
    expect(flow.flow().menu?.entries.map((entry) => entry.label)).toContain("Rename");
    flow.flow().openMenu("term-three");
    await act(async () => {});
    expect(flow.flow().menu?.entries.map((entry) => entry.label)).not.toContain("Rename");
    await flow.unmount();
  });

  it("hides Rename on an agent when the daemon does not advertise the capability", async () => {
    // The control: the same agent with the capability advertised gets the row.
    const supported = renderFlow([agentSession("agent-one", "Agent one")], true);
    await supported.mount();
    supported.flow().openMenu("agent-one");
    await act(async () => {});
    expect(supported.flow().menu?.entries.map((entry) => entry.label)).toContain("Rename");
    await supported.unmount();

    const flow = renderFlow([agentSession("agent-one", "Agent one")], false);
    await flow.mount();
    flow.flow().openMenu("agent-one");
    await act(async () => {});
    expect(flow.flow().menu?.entries.map((entry) => entry.label)).not.toContain("Rename");
    await flow.unmount();
  });

  it("hides Rename on a recovered agent — the daemon's road needs a live process", async () => {
    // The daemon reaches the session record only through a live registry
    // entry; a journal-replayed (recovered) row is a Transcript entry and the
    // rename is refused with process_gone. The control: the live agent in the
    // same menu gets the row, so the recovered one's absence is the answer.
    const sessions = [
      agentSession("agent-one", "Agent one"),
      recoveredAgentSession("agent-two", "Agent two"),
    ];
    const flow = renderFlow(sessions, true);
    await flow.mount();
    flow.flow().openMenu("agent-one");
    await act(async () => {});
    expect(flow.flow().menu?.entries.map((entry) => entry.label)).toContain("Rename");
    flow.flow().openMenu("agent-two");
    await act(async () => {});
    expect(flow.flow().menu?.entries.map((entry) => entry.label)).not.toContain("Rename");
    await flow.unmount();
  });

  it("offers Rename on an ended agent — EOF keeps the entry live", async () => {
    // The non-obvious half: an agent whose process exited while this daemon
    // was alive is still a Live registry entry, so the rename reaches it.
    const flow = renderFlow([endedAgentSession("agent-one", "Agent one")], true);
    await flow.mount();
    flow.flow().openMenu("agent-one");
    await act(async () => {});
    const entries = flow.flow().menu?.entries ?? [];
    expect(entries.map((entry) => entry.label)).toContain("Rename");
    await flow.unmount();
  });

  it("never offers Rename on a selection menu — rename is per-tab", async () => {
    // The control: the same agent's tab menu carries the row.
    const tab = renderFlow([agentSession("agent-one", "Agent one")], true);
    await tab.mount();
    tab.flow().openMenu("agent-one");
    await act(async () => {});
    expect(tab.flow().menu?.entries.map((entry) => entry.label)).toContain("Rename");
    await tab.unmount();

    // A selection menu is close-only.
    const flow = renderFlow([agentSession("agent-one", "Agent one")], true, new Set(["agent-one"]));
    await flow.mount();
    flow.flow().openMenu("agent-one");
    await act(async () => {});
    expect(flow.flow().menu?.entries.map((entry) => entry.label)).toEqual(["Close"]);
    await flow.unmount();
  });
});

describe("the rename dispatch roads", () => {
  it("the menu's Rename opens the dialog with focus on the tab", async () => {
    const flow = renderFlow([agentSession("agent-one", "Agent one")], true);
    await flow.mount();
    const tab = flow.host.querySelector<HTMLButtonElement>("#workspace-session-tab-agent-one");
    if (tab === null) throw new Error("tab did not render");
    flow.flow().openMenu("agent-one");
    await act(async () => {});
    flow.flow().activateEntry("rename");
    await act(async () => {});
    const rename = flow.rename().rename;
    expect(rename).toEqual({ sessionId: "agent-one", title: "Agent one" });
    expect(document.activeElement).toBe(tab);
    await flow.unmount();
  });

  it("a roster push for another session leaves the open rename alone", async () => {
    const sessions = [
      agentSession("agent-one", "Agent one"),
      agentSession("agent-two", "Agent two"),
    ];
    const flow = renderFlow(sessions, true);
    await flow.mount();
    flow.rename().openRename("agent-one");
    await act(async () => {});
    expect(flow.rename().rename).toEqual({ sessionId: "agent-one", title: "Agent one" });

    await act(async () => {
      flow.setSessions([...sessions, agentSession("agent-three", "Agent three")]);
    });
    expect(flow.rename().rename).toEqual({ sessionId: "agent-one", title: "Agent one" });
    await flow.unmount();
  });

  it("the auto-title landing moves the open dialog's pre-fill", async () => {
    // The daemon names an untitled session from its first prompt while the
    // dialog is open: the roster push carries the new name, and the field
    // follows the push rather than keeping what it opened with.
    const sessions = [agentSession("agent-one", "Agent one")];
    const flow = renderFlow(sessions, true);
    await flow.mount();
    flow.rename().openRename("agent-one");
    await act(async () => {});

    const titled = { ...sessions[0]!, displayName: "run the flaky test suite" };
    await act(async () => {
      flow.setSessions([titled]);
    });

    expect(flow.rename().rename).toEqual({
      sessionId: "agent-one",
      title: "run the flaky test suite",
    });
    await flow.unmount();
  });

  it("a session that leaves the roster closes the open dialog", async () => {
    // The same invalidation the menu and the ask already honour: a
    // MEANINGFUL roster change is dead, not dormant.
    const sessions = [agentSession("agent-one", "Agent one")];
    const flow = renderFlow(sessions, true);
    await flow.mount();
    flow.rename().openRename("agent-one");
    await act(async () => {});
    expect(flow.rename().rename).not.toBeNull();

    await act(async () => {
      flow.setSessions([]);
    });

    expect(flow.rename().rename).toBeNull();
    await flow.unmount();
  });

  it("the pane header kebab's Rename opens the same dialog", async () => {
    const flow = renderFlow([agentSession("agent-one", "Agent one")], true);
    await flow.mount();
    flow.flow().activatePaneEntry("agent-one", "rename");
    await act(async () => {});
    expect(flow.rename().rename).toEqual({ sessionId: "agent-one", title: "Agent one" });
    await flow.unmount();
  });

  it("refuses the keys the pane header never offers", async () => {
    const flow = renderFlow([agentSession("agent-one", "Agent one")], true);
    await flow.mount();
    flow.flow().activatePaneEntry("agent-one", "delete");
    flow.flow().activatePaneEntry("agent-one", "close-selection");
    await act(async () => {});
    expect(flow.rename().rename).toBeNull();
    expect(flow.flow().confirm).toBeNull();
    await flow.unmount();
  });
});
