// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";
import {
  closingDaemon,
  ended,
  failed,
  live,
  surface,
  openMenu,
  archiveAction,
  rowTitles,
  pressArchiveAction,
  answerAsk,
} from "./subagentArchiveHarness";
import { sessionClose } from "../../lib/tauri";
import type { SessionState } from "../../types/ipc";

vi.mock("../../lib/tauri", async () => ({
  ...(await import("./sessionChannelHarness")).tauriMock,
  sessionClose: vi.fn(async () => undefined),
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  vi.mocked(sessionClose).mockReset();
  vi.mocked(sessionClose).mockImplementation(async () => undefined);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
});

describe("the subagent menu's archive outcome", () => {
  it("keeps a child whose close failed, with a mapped sentence and no raw daemon text", async () => {
    const refresh = vi.fn(async () => undefined);
    vi.mocked(sessionClose).mockImplementation(async (id: string) => {
      if (id === "child-bad") {
        throw { code: "journal", message: "raw journal words" };
      }
      await daemon.close(id);
    });
    const daemon = closingDaemon(
      (node) => root.render(node),
      [ended("child-ok", 1, "Good child"), ended("child-bad", 1, "Bad child")],
      refresh,
    );
    await act(async () => root.render(daemon.node()));
    await openMenu();

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(rowTitles()).toEqual(["Bad child"]);
    const failure = document.querySelector<HTMLElement>(".workspace-subagent-row-failure");
    expect(failure?.textContent).toBe("Saved history could not be read or written.");
    // The daemon's own words never reach the DOM, in any form.
    expect(document.body.textContent).not.toContain("raw journal words");
    expect(document.querySelector('[title*="raw journal"]')).toBeNull();
    expect(document.querySelector(".error-detail-sr-only")).toBeNull();
    // The child stayed in the roster, so its row still offers the action.
    expect(archiveAction()?.textContent).toBe("Archive 1 finished subagent");
  });

  it("keeps the act's answers when the roster read fails", async () => {
    const refresh = vi.fn(async () => Promise.reject(new Error("roster read failed")));
    vi.mocked(sessionClose).mockImplementation(async (id: string) => {
      if (id === "child-bad") {
        throw { code: "journal", message: "raw journal words" };
      }
      await daemon.close(id);
    });
    const daemon = closingDaemon(
      (node) => root.render(node),
      [ended("child-ok", 1, "Good child"), ended("child-bad", 1, "Bad child")],
      refresh,
    );
    await act(async () => root.render(daemon.node()));
    await openMenu();

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(refresh).toHaveBeenCalledTimes(1);
    expect(rowTitles()).toEqual(["Bad child"]);
    const failure = document.querySelector<HTMLElement>(".workspace-subagent-row-failure");
    expect(failure?.textContent).toBe("Saved history could not be read or written.");
  });

  it("leaves focus on the first remaining row", async () => {
    const refresh = vi.fn(async () => undefined);
    const daemon = closingDaemon(
      (node) => root.render(node),
      [
        ended("child-a", 1, "First child"),
        ended("child-b", 1, "Second child"),
        live("child-run", 1, "Working child"),
      ],
      refresh,
    );
    vi.mocked(sessionClose).mockImplementation(daemon.close);
    await act(async () => root.render(daemon.node()));
    await openMenu();

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(document.querySelector(".workspace-subagent-list")).not.toBeNull();
    expect(document.activeElement?.classList.contains("workspace-subagent-row")).toBe(true);
    expect(document.activeElement?.textContent).toBe("Working child");
  });

  it("sends focus to the composer when the last child was archived", async () => {
    const refresh = vi.fn(async () => undefined);
    const daemon = closingDaemon(
      (node) => root.render(node),
      [ended("child-only", 1, "Only child")],
      refresh,
    );
    vi.mocked(sessionClose).mockImplementation(daemon.close);
    await act(async () => root.render(daemon.node()));
    await act(async () => {
      channelHarness.active?.({ type: "agent_finished", stopReason: "end_turn" });
    });
    await openMenu();
    const composer = document.querySelector<HTMLTextAreaElement>(".workspace-composer textarea");
    if (composer === null) throw new Error("composer did not render");

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-only");
    expect(document.querySelector(".workspace-subagent-list")).toBeNull();
    expect(container.querySelector('[data-testid="subagent-pill"]')).toBeNull();
    expect(document.activeElement).toBe(composer);
    expect(document.activeElement).not.toBe(document.body);
  });

  it("takes the archived child out of the pill's counts", async () => {
    const refresh = vi.fn(async () => undefined);
    const daemon = closingDaemon(
      (node) => root.render(node),
      [failed("child-fail", 1, "Failing child"), live("child-run", 1, "Working child")],
      refresh,
    );
    vi.mocked(sessionClose).mockImplementation(daemon.close);
    await act(async () => root.render(daemon.node()));
    await openMenu();

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    expect(pill?.getAttribute("aria-label")).toBe("Subagents: 1 failed, 1 working");

    await pressArchiveAction();
    await answerAsk("Archive");

    const after = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    expect(after?.getAttribute("aria-label")).toBe("Subagents: 1 working");
    expect(rowTitles()).toEqual(["Working child"]);
  });

  it("keeps focus off the body when the composer cannot take it", async () => {
    const refresh = vi.fn(async () => undefined);
    const observedState: SessionState = {
      type: "ended",
      generation: 1,
      code: 0,
      integrity: { kind: "complete" },
    };
    const daemon = closingDaemon(
      (node) => root.render(node),
      [ended("child-only", 1, "Only child")],
      refresh,
      observedState,
    );
    vi.mocked(sessionClose).mockImplementation(daemon.close);
    await act(async () => root.render(daemon.node()));
    await openMenu();
    const composer = document.querySelector<HTMLTextAreaElement>(".workspace-composer textarea");
    if (composer === null) throw new Error("composer did not render");
    expect(composer.disabled).toBe(true);

    await pressArchiveAction();
    await answerAsk("Archive");

    const transcript = document.querySelector<HTMLElement>(".workspace-conversation");
    expect(document.activeElement).toBe(transcript);
    expect(document.activeElement).not.toBe(document.body);
  });

  it("names the conversation, where focus falls back, as a region", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([], refresh)));
    const transcript = document.querySelector<HTMLElement>(".workspace-conversation");
    expect(transcript?.getAttribute("role")).toBe("region");
    expect(transcript?.getAttribute("aria-label")).toBe("Conversation");
  });

  it("puts focus on the pill when the ask is cancelled after its sole target left the roster", async () => {
    const refresh = vi.fn(async () => undefined);
    const daemon = closingDaemon(
      (node) => root.render(node),
      [ended("child-only", 1, "Only child"), live("child-run", 1, "Working child")],
      refresh,
    );
    await act(async () => root.render(daemon.node()));
    await openMenu();

    await pressArchiveAction();
    // The roster drops the child while the ask is up: the action the ask
    // came from unmounts with it.
    await act(async () => daemon.push([live("child-run", 1, "Working child")]));
    expect(archiveAction()).toBeNull();

    await answerAsk("Cancel");

    expect(sessionClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(container.querySelector('[data-testid="subagent-pill"]'));
  });
});
