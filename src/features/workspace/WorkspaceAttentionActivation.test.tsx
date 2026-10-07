// @vitest-environment happy-dom
// A click on an attention toast. The target the OS hands back has to open the
// session's own tab through the roads a row click already uses, and a session
// the roster no longer holds has to leave its workspace in front instead of
// opening anything new.
//
// The click arrives as one app event from the Rust side, so the listener is
// driven through the mocked event module: the tests exercise the same road the
// app registers at startup, never the handler function alone.
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(event, handler);
    return () => {
      listeners.delete(event);
    };
  }),
}));

import { listen } from "@tauri-apps/api/event";
import {
  afterEachHarness,
  agentSession,
  beforeEachHarness,
  pushSnapshots,
  renderWorkspace,
  tabElement,
  unmountWorkspace,
  workspace,
} from "./bulkCloseHarness";
import { sessionCreate, sessionsList, workspacesList } from "../../lib/tauri";
import type { Session, SessionStateSnapshot } from "../../types/ipc";
import { disposeAttentionActivation, startAttentionActivation } from "./attentionActivation";
import { localWorkspaceKey } from "./hosts/hostIdentity";
import { getLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
import { useAppStore } from "../../store/appStore";
import { sharedSessionController } from "./workspaceSessions";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

/** The wire name the Rust half publishes; a literal so a rename breaks here. */
const ATTENTION_ACTIVATED_EVENT = "attention:activated";

const secondWorkspace = { ...workspace, id: "workspace-2", title: "second", path: "C:\\side" };

const inWorkspace = (id: string, title: string, workspaceId: string): Session => ({
  ...agentSession(id, title),
  workspaceId,
});

const rosterOf = (members: Array<{ id: string; workspaceId: string }>): SessionStateSnapshot[] =>
  members.map(({ id, workspaceId }) => ({
    id,
    workspaceId,
    kind: "acp" as const,
    title: id,
    state: { type: "live" as const, generation: 1 },
    elapsedMs: 0,
  }));

/** One click, delivered where the Rust event lands: through the listener the
 *  app registers at startup. A file that lost its listener fails here. */
async function clickToast(payload: unknown): Promise<void> {
  await startAttentionActivation();
  const handler = listeners.get(ATTENTION_ACTIVATED_EVENT);
  if (handler === undefined) throw new Error("the activation listener is not registered");
  await act(async () => handler({ payload }));
}

function selectedWorkspaceTitle(): string | null {
  const row = document.querySelector<HTMLButtonElement>('.workspace-row[aria-pressed="true"]');
  return row?.textContent?.trim() ?? null;
}

const tabId = (sessionId: string): string => `workspace-session-tab-${sessionId}`;

beforeEach(() => {
  beforeEachHarness();
  disposeAttentionActivation();
  listeners.clear();
  vi.mocked(workspacesList).mockResolvedValue([workspace, secondWorkspace]);
  vi.mocked(sessionsList).mockResolvedValue([
    inWorkspace("agent-one", "Agent one", "workspace-1"),
    inWorkspace("agent-two", "Agent two", "workspace-2"),
  ]);
});

afterEach(async () => {
  await afterEachHarness();
  listeners.clear();
  useAppStore.getState().selectSurface("workspace");
});

describe("a click on an attention toast", () => {
  it("activation_opens_the_target_session_tab", async () => {
    await renderWorkspace(false);
    await pushSnapshots(rosterOf([{ id: "agent-two", workspaceId: "workspace-2" }]));

    await clickToast({ sessionId: "agent-two", workspaceId: "workspace-2" });

    expect(tabElement("agent-two").getAttribute("aria-selected")).toBe("true");
    expect(selectedWorkspaceTitle()).toContain("second");
  });

  it("activation_for_a_gone_session_opens_its_workspace", async () => {
    await renderWorkspace(false);
    await pushSnapshots(rosterOf([{ id: "agent-one", workspaceId: "workspace-1" }]));
    vi.mocked(sessionCreate).mockClear();

    await clickToast({ sessionId: "agent-gone", workspaceId: "workspace-2" });

    expect(getLastSelectedWorkspaceKey()).toBe(localWorkspaceKey("workspace-2"));
    expect(selectedWorkspaceTitle()).toContain("second");
    expect(document.getElementById(tabId("agent-gone"))).toBeNull();
    expect(vi.mocked(sessionCreate)).not.toHaveBeenCalled();
  });

  it("activation_handler_registers_once", async () => {
    await renderWorkspace(true);
    const opened = vi.spyOn(sharedSessionController(), "open");
    await pushSnapshots(rosterOf([{ id: "agent-two", workspaceId: "workspace-2" }]));

    await clickToast({ sessionId: "agent-two", workspaceId: "workspace-2" });
    await startAttentionActivation();

    const registrations = vi
      .mocked(listen)
      .mock.calls.filter(([event]) => event === ATTENTION_ACTIVATED_EVENT);
    expect(registrations).toHaveLength(1);
    expect(opened).toHaveBeenCalledTimes(1);
  });

  it("drops a payload that is not a target", async () => {
    await renderWorkspace(false);
    await pushSnapshots(rosterOf([{ id: "agent-two", workspaceId: "workspace-2" }]));

    await clickToast({ sessionId: 7, workspaceId: "workspace-2" });
    await clickToast({ sessionId: "agent-two" });

    expect(document.getElementById(tabId("agent-two"))).toBeNull();
  });

  it("releases the listener when the module is replaced", async () => {
    await renderWorkspace(false);
    await startAttentionActivation();
    expect(listeners.has(ATTENTION_ACTIVATED_EVENT)).toBe(true);

    disposeAttentionActivation();
    expect(listeners.has(ATTENTION_ACTIVATED_EVENT)).toBe(false);

    // The replaced module starts over: one registration, one listener.
    await startAttentionActivation();
    expect(listeners.has(ATTENTION_ACTIVATED_EVENT)).toBe(true);
  });

  it("releases a registration that resolves after the module was replaced", async () => {
    // The bridge answers late: the dispose lands while listen() is pending, so
    // nobody is left holding the unlisten the promise is about to hand over.
    let settle: (unlisten: () => void) => void = () => undefined;
    vi.mocked(listen).mockImplementationOnce(
      () =>
        new Promise<() => void>((resolve) => {
          settle = resolve;
        }),
    );
    const unlisten = vi.fn();

    const pending = startAttentionActivation();
    disposeAttentionActivation();
    await act(async () => {
      settle(unlisten);
      await pending;
    });

    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("a click with no strip mounted opens the tab through the roster", async () => {
    // Settings or Design on screen: the surface's opener is withdrawn with it,
    // and the app-lifetime roads have to carry the click on their own.
    await renderWorkspace(false);
    await pushSnapshots(rosterOf([{ id: "agent-two", workspaceId: "workspace-2" }]));
    await unmountWorkspace();
    useAppStore.getState().selectSurface("settings");

    await clickToast({ sessionId: "agent-two", workspaceId: "workspace-2" });

    const state = sharedSessionController().getState();
    expect(state.selectedSessionId).toBe("agent-two");
    expect(state.openSessions.map((row) => row.id)).toContain("agent-two");
    expect(getLastSelectedWorkspaceKey()).toBe(localWorkspaceKey("workspace-2"));
    expect(useAppStore.getState().activeSurface).toBe("workspace");
  });

  it("a click with no strip mounted and no roster row lands on the workspace", async () => {
    await renderWorkspace(false);
    await pushSnapshots(rosterOf([{ id: "agent-one", workspaceId: "workspace-1" }]));
    await unmountWorkspace();
    const refresh = vi.spyOn(sharedSessionController(), "refresh");

    await clickToast({ sessionId: "agent-gone", workspaceId: "workspace-2" });

    expect(refresh).toHaveBeenCalled();
    expect(getLastSelectedWorkspaceKey()).toBe(localWorkspaceKey("workspace-2"));
    expect(document.getElementById(tabId("agent-gone"))).toBeNull();
  });
});
