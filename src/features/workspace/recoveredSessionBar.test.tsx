// Human-path proofs for the recovered reopen bar: the daemon's verdict,
// never a re-derivation, and never an auto-resume on mount.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  sessionResume: vi.fn(),
  reasonFromCause: vi.fn((cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
  ),
}));

import { reasonFromCause, sessionResume } from "../../lib/tauri";
import { RecoveredSessionBar } from "./recoveredSessionBar";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function recoveredSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "rec-1",
    workspaceId: null,
    kind: "claude",
    title: "old chat",
    state: {
      type: "recovered",
      generation: 2,
      integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
    },
    elapsedMs: null,
    resumable: true,
    ...overrides,
  };
}

async function renderBar(session: Session | null, onReopened = vi.fn()) {
  root = createRoot(container);
  await act(async () => {
    root.render(<RecoveredSessionBar session={session} onReopened={onReopened} />);
  });
  return onReopened;
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.mocked(sessionResume).mockResolvedValue({
    type: "resumed",
    session: recoveredSession({ id: "rec-1" }),
  });
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("RecoveredSessionBar", () => {
  it("offers Reopen exactly when the daemon says resumable, and never resumes on mount", async () => {
    const onReopened = await renderBar(recoveredSession({ resumable: true }));

    expect(container.querySelector('[data-testid="recovered-reopen-bar"]')).not.toBeNull();
    expect(container.textContent).toContain("Reopen");
    expect(sessionResume).not.toHaveBeenCalled();
    expect(onReopened).not.toHaveBeenCalled();
  });

  it("reopens on one click and hands the resumed session back", async () => {
    const resumed = recoveredSession({ id: "rec-1" });
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "resumed", session: resumed });
    const onReopened = await renderBar(recoveredSession({ resumable: true }));

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());

    expect(sessionResume).toHaveBeenCalledTimes(1);
    expect(sessionResume).toHaveBeenCalledWith("rec-1");
    expect(onReopened).toHaveBeenCalledTimes(1);
    expect(onReopened).toHaveBeenCalledWith(resumed);
  });

  it("shows no button for a recovered row the daemon says cannot resume, with honest copy", async () => {
    await renderBar(recoveredSession({ resumable: false }));

    expect(container.querySelector('[data-testid="recovered-reopen-bar"]')).toBeNull();
    expect(container.querySelector('[data-testid="recovered-unresumable"]')).not.toBeNull();
    expect(container.textContent).toContain("Resume is not available for this session");
    expect(container.querySelector("button")).toBeNull();
    expect(sessionResume).not.toHaveBeenCalled();
  });

  it("shows no button when the daemon predates the verdict, and still says it cannot come back", async () => {
    const { resumable: _dropped, ...withoutVerdict } = recoveredSession();
    await renderBar(withoutVerdict as Session);

    expect(container.querySelector('[data-testid="recovered-reopen-bar"]')).toBeNull();
    expect(container.querySelector('[data-testid="recovered-unresumable"]')).not.toBeNull();
    expect(sessionResume).not.toHaveBeenCalled();
  });

  it("renders nothing for a live session", async () => {
    await renderBar(
      recoveredSession({
        id: "live-1",
        state: { type: "live", generation: 1 },
        resumable: false,
      }),
    );

    expect(container.textContent).toBe("");
    expect(sessionResume).not.toHaveBeenCalled();
  });

  it("reports a refused resume instead of failing silently", async () => {
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    await renderBar(recoveredSession({ resumable: true }));

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());

    expect(container.querySelector('[role="alert"]')?.textContent).toContain("gone");
  });

  it("reports a rejected resume with the daemon's own reason", async () => {
    vi.mocked(sessionResume).mockRejectedValueOnce(new Error("pipe broke"));
    vi.mocked(reasonFromCause).mockReturnValueOnce("pipe broke");
    await renderBar(recoveredSession({ resumable: true }));

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());

    expect(container.querySelector('[role="alert"]')?.textContent).toContain("pipe broke");
  });
});
