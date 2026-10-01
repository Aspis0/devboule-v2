// Human-path proofs for the recovered reopen bar: the daemon's verdict,
// never a re-derivation, and never an auto-resume on mount. The quiet
// note's own tokens are proved through the real sheet.
// @vitest-environment happy-dom
import { act, useLayoutEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ResumeResult, Session } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  sessionResume: vi.fn(),
}));

import { sessionResume } from "../../lib/tauri";
import { RecoveredSessionBar } from "./recoveredSessionBar";
import { assembleCssProof, removeCssProof, specificity } from "./cssProof";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../..");
const sheets = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/styles/global.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/Workspace.css"), "utf8"),
];

// Any ancestor wearing one of these would read the note or the button aloud;
// the checks walk the whole chain, bar-external ancestors included.
const LIVE_ANCESTOR =
  '[role="status"],[role="alert"],[role="log"],[role="marquee"],[role="timer"],[aria-live]';

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
    root.render(
      <RecoveredSessionBar
        key={session === null ? "no-session" : session.id}
        session={session}
        onReopened={onReopened}
      />,
    );
  });
  return onReopened;
}

async function mountBar(session: Session, onResumeFailed?: () => void): Promise<void> {
  root = createRoot(container);
  await rerenderBar(session, onResumeFailed);
}

async function rerenderBar(session: Session | null, onResumeFailed?: () => void): Promise<void> {
  await act(async () => {
    root.render(
      <RecoveredSessionBar
        key={session === null ? "no-session" : session.id}
        session={session}
        onReopened={vi.fn()}
        onResumeFailed={onResumeFailed}
      />,
    );
  });
}

// Fires the test's settle from the commit itself: the window between a commit
// and the effects that commit schedules can only be entered from inside it.
function CommitProbe({ resumable, onCommit }: { resumable: boolean; onCommit: () => void }) {
  useLayoutEffect(() => {
    onCommit();
  });
  return (
    <RecoveredSessionBar
      key="rec-1"
      session={recoveredSession({ resumable })}
      onReopened={vi.fn()}
    />
  );
}

function verdictStatus(): Element {
  const status = container.querySelector('[data-testid="recovered-verdict-status"]');
  if (status === null) throw new Error("verdict status did not render");
  return status;
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
  removeCssProof();
  vi.resetAllMocks();
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
    await renderBar(recoveredSession({ resumable: true }));

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());

    expect(container.querySelector('[role="alert"]')?.textContent).toContain("pipe broke");
  });

  it("asks for a roster refresh when the resume fails, and the offer yields to the read-only notice", async () => {
    vi.mocked(sessionResume).mockRejectedValueOnce(
      new Error("ACP request failed (-32002): Resource not found"),
    );
    const onResumeFailed = vi.fn();
    // The refresh contract arrives with this change; the cast keeps the red
    // runnable against the component that does not know the prop yet.
    const props = {
      session: recoveredSession(),
      onReopened: vi.fn(),
      onResumeFailed,
    } as Parameters<typeof RecoveredSessionBar>[0];
    root = createRoot(container);
    await act(async () => {
      root.render(<RecoveredSessionBar {...props} />);
    });
    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());

    expect(onResumeFailed).toHaveBeenCalledTimes(1);

    // The refresh lands: the daemon retracted the verdict, and the read-only
    // notice takes the button's place.
    await act(async () => {
      root.render(
        <RecoveredSessionBar
          session={recoveredSession({ resumable: false })}
          onReopened={vi.fn()}
        />,
      );
    });
    expect(container.querySelector('[data-testid="recovered-reopen-bar"]')).toBeNull();
    expect(container.querySelector('[data-testid="recovered-unresumable"]')).not.toBeNull();
  });

  it("leaves Reopen usable when the verdict dips and returns while the resume is in flight", async () => {
    let fail!: (result: ResumeResult) => void;
    vi.mocked(sessionResume).mockReturnValueOnce(
      new Promise<ResumeResult>((resolve) => {
        fail = resolve;
      }),
    );
    await mountBar(recoveredSession());

    const reopenButton = (): HTMLButtonElement | null =>
      container.querySelector<HTMLButtonElement>('[data-testid="recovered-reopen-bar"] button');
    const button = reopenButton();
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(button.textContent).toBe("Reopening…");
    expect(button.disabled).toBe(true);

    // The roster retracts the verdict and repairs it again before the attempt
    // settles: whatever comes back must belong to no attempt at all.
    await rerenderBar(recoveredSession({ resumable: false }));
    await rerenderBar(recoveredSession({ resumable: true }));

    const returned = reopenButton();
    if (returned === null) {
      throw new Error("Reopen button did not render after the verdict returned");
    }
    expect(returned.textContent).toBe("Reopen");
    expect(returned.disabled).toBe(false);

    // The attempt that outlived its verdict arrives late: no danger block,
    // and the bar says only that the click failed.
    await act(async () => {
      fail({ type: "failed", message: "session vanished" });
    });
    const settled = reopenButton();
    if (settled === null) throw new Error("Reopen button did not render after the stale settle");
    expect(settled.textContent).toBe("Reopen");
    expect(settled.disabled).toBe(false);
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(verdictStatus().textContent).toBe("Reopen failed. You can try again.");
  });

  it("renders nothing for a recovered terminal: the pane carries that failure's one sentence", async () => {
    await renderBar(recoveredSession({ kind: "terminal", resumable: false }));

    expect(container.textContent).toBe("");
    expect(sessionResume).not.toHaveBeenCalled();
  });
});

describe("the recovered bar's quiet state", () => {
  it("keeps the static note and the Reopen control outside every live region, in both branches", async () => {
    const onReopened = await renderBar(recoveredSession({ resumable: true }));

    const open = container.querySelector('[data-testid="recovered-reopen-bar"]');
    if (open === null) throw new Error("reopen bar did not render");
    expect(open.getAttribute("role")).toBeNull();
    expect(open.getAttribute("aria-live")).toBeNull();
    expect(open.textContent).toContain("Read-only transcript from the journal.");
    expect(open.classList.contains("workspace-session-notice")).toBe(true);
    const note = open.querySelector(".workspace-session-notice-text");
    if (note === null) throw new Error("note did not render");
    expect(note.closest(LIVE_ANCESTOR)).toBeNull();
    const button = open.querySelector<HTMLButtonElement>("button");
    if (button === null) throw new Error("Reopen did not render");
    expect(button.closest(LIVE_ANCESTOR)).toBeNull();
    button.focus();
    expect(document.activeElement).toBe(button);
    const status = container.querySelector('[data-testid="recovered-verdict-status"]');
    if (status === null) throw new Error("verdict status did not render");
    expect(status.getAttribute("role")).toBe("status");
    expect(status.classList.contains("sr-only")).toBe(true);
    expect(status.textContent).toBe("");

    const unresumable = recoveredSession({ resumable: false });
    await act(async () => {
      root.render(
        <RecoveredSessionBar key={unresumable.id} session={unresumable} onReopened={onReopened} />,
      );
    });

    const closed = container.querySelector('[data-testid="recovered-unresumable"]');
    if (closed === null) throw new Error("unresumable note did not render");
    expect(closed.getAttribute("role")).toBeNull();
    expect(closed.getAttribute("aria-live")).toBeNull();
    expect(closed.textContent).toContain("Resume is not available for this session");
    expect(closed.classList.contains("workspace-session-notice")).toBe(true);
    const closedNote = closed.querySelector(".workspace-session-notice-text");
    if (closedNote === null) throw new Error("unresumable note text did not render");
    expect(closedNote.closest(LIVE_ANCESTOR)).toBeNull();
    expect(container.querySelector('[data-testid="recovered-verdict-status"]')).toBe(status);
    expect(status.textContent).toBe("");
  });

  it("speaks the not-resumable verdict once, after the click that asked for it", async () => {
    const sentence = "This transcript is read-only. Resume is not available for this session.";
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    const onResumeFailed = vi.fn();
    root = createRoot(container);
    await act(async () => {
      root.render(
        <RecoveredSessionBar
          session={recoveredSession()}
          onReopened={vi.fn()}
          onResumeFailed={onResumeFailed}
        />,
      );
    });
    const status = container.querySelector('[data-testid="recovered-verdict-status"]');
    if (status === null) throw new Error("verdict status did not render");
    expect(status.getAttribute("role")).toBe("status");
    expect(status.textContent).toBe("");

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(onResumeFailed).toHaveBeenCalledTimes(1);
    expect(status.textContent).toBe(sentence);

    // The roster refresh lands with the retracted verdict.
    await act(async () => {
      root.render(
        <RecoveredSessionBar
          session={recoveredSession({ resumable: false })}
          onReopened={vi.fn()}
          onResumeFailed={onResumeFailed}
        />,
      );
    });

    expect(container.querySelector('[data-testid="recovered-verdict-status"]')).toBe(status);
    // The visible note carries the sentence now, so the region steps aside.
    expect(status.textContent).toBe("");
  });

  it("speaks the outcome when the verdict retracts while the resume is in flight", async () => {
    const sentence = "This transcript is read-only. Resume is not available for this session.";
    let fail!: (result: ResumeResult) => void;
    vi.mocked(sessionResume).mockReturnValueOnce(
      new Promise<ResumeResult>((resolve) => {
        fail = resolve;
      }),
    );
    const onResumeFailed = vi.fn();
    await mountBar(recoveredSession(), onResumeFailed);

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(verdictStatus().textContent).toBe("");

    // The roster retracts the verdict while the attempt is still in flight.
    await rerenderBar(recoveredSession({ resumable: false }), onResumeFailed);
    expect(container.querySelector('[data-testid="recovered-unresumable"]')).not.toBeNull();

    await act(async () => {
      fail({ type: "failed", message: "session vanished" });
    });

    // The settle is not erased: the region, mounted and waiting since the
    // flip, takes the outcome while the note keeps the visible verdict.
    expect(verdictStatus().textContent).toBe(sentence);
    expect(container.querySelector('[data-testid="recovered-unresumable"]')?.textContent).toContain(
      sentence,
    );
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(onResumeFailed).toHaveBeenCalledTimes(1);
  });

  it("says the retry note once when a failed reopen lands on a repaired verdict", async () => {
    const retry = "Reopen failed. You can try again.";
    let fail!: (result: ResumeResult) => void;
    vi.mocked(sessionResume).mockReturnValueOnce(
      new Promise<ResumeResult>((resolve) => {
        fail = resolve;
      }),
    );
    await mountBar(recoveredSession());

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(button.textContent).toBe("Reopening…");

    await rerenderBar(recoveredSession({ resumable: false }));
    await rerenderBar(recoveredSession({ resumable: true }));
    await act(async () => {
      fail({ type: "failed", message: "session vanished" });
    });

    // The user clicked, so the pane says it failed — politely, exactly once,
    // with no danger block and a control that stays usable.
    expect(verdictStatus().textContent).toBe(retry);
    expect((container.textContent ?? "").split(retry)).toHaveLength(2);
    expect(container.querySelector('[role="alert"]')).toBeNull();
    const settled = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (settled === null) throw new Error("Reopen button did not render after the settle");
    expect(settled.textContent).toBe("Reopen");
    expect(settled.disabled).toBe(false);

    // Another roster read with the same verdict: the text does not change.
    await rerenderBar(recoveredSession({ resumable: true }));
    expect(verdictStatus().textContent).toBe(retry);
  });

  it("answers a failure that settles between the flip's commit and its effects", async () => {
    let fail!: (result: ResumeResult) => void;
    vi.mocked(sessionResume).mockReturnValueOnce(
      new Promise<ResumeResult>((resolve) => {
        fail = resolve;
      }),
    );
    let armed = false;
    let committed!: () => void;
    const commitDone = new Promise<void>((resolve) => {
      committed = resolve;
    });
    const onCommit = (): void => {
      if (!armed) return;
      armed = false;
      fail({ type: "failed", message: "session vanished" });
      committed();
    };

    root = createRoot(container);
    const showProbe = async (resumable: boolean): Promise<void> => {
      await act(async () => {
        root.render(<CommitProbe resumable={resumable} onCommit={onCommit} />);
      });
    };
    await showProbe(true);

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    await showProbe(false);

    // The repair goes through React's normal scheduling, and the failure is
    // fired from its commit — before that commit's effects have run.
    armed = true;
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = false;
    root.render(<CommitProbe resumable onCommit={onCommit} />);
    await commitDone;
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    await act(async () => undefined);

    expect(verdictStatus().textContent).toBe("Reopen failed. You can try again.");
    expect(container.querySelector('[role="alert"]')).toBeNull();
    const settled = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (settled === null) throw new Error("Reopen button did not render");
    expect(settled.textContent).toBe("Reopen");
    expect(settled.disabled).toBe(false);
  });

  it("announces once: the region text asserted after each step of a failing click", async () => {
    const sentence = "This transcript is read-only. Resume is not available for this session.";
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    const onResumeFailed = vi.fn();

    await mountBar(recoveredSession(), onResumeFailed);
    const status = verdictStatus();
    expect(status.textContent).toBe("");
    expect(container.querySelectorAll('[data-testid="recovered-verdict-status"]')).toHaveLength(1);
    expect(container.querySelectorAll('[role="status"]')).toHaveLength(1);

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(verdictStatus()).toBe(status);
    expect(status.textContent).toBe(sentence);

    // The refreshed verdict lands: the visible note carries the sentence from
    // here, so the region steps aside — one copy in the tree, no re-speech.
    await rerenderBar(recoveredSession({ resumable: false }), onResumeFailed);
    expect(status.textContent).toBe("");

    // Another roster read with the same verdict: still empty.
    await rerenderBar(recoveredSession({ resumable: false }), onResumeFailed);
    expect(status.textContent).toBe("");
  });

  it("leaves a never-clicked non-resumable session's region empty when switching straight to it", async () => {
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    const onResumeFailed = vi.fn();
    await mountBar(recoveredSession({ id: "rec-1" }), onResumeFailed);

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(onResumeFailed).toHaveBeenCalledTimes(1);

    // Straight from the failed click to a session the daemon already calls
    // not-resumable and this pane never clicked: its region must be empty.
    await rerenderBar(recoveredSession({ id: "rec-2", resumable: false }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");
    expect(container.querySelector('[data-testid="recovered-unresumable"]')).not.toBeNull();
  });

  it("says nothing after a successful resume when the verdict later flips", async () => {
    vi.mocked(sessionResume).mockResolvedValueOnce({
      type: "resumed",
      session: recoveredSession({ id: "rec-1" }),
    });
    const onReopened = vi.fn();
    root = createRoot(container);
    await act(async () => {
      root.render(
        <RecoveredSessionBar session={recoveredSession({ id: "rec-1" })} onReopened={onReopened} />,
      );
    });
    expect(verdictStatus().textContent).toBe("");

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(onReopened).toHaveBeenCalledTimes(1);
    expect(verdictStatus().textContent).toBe("");

    // The daemon later re-recovers this session as not-resumable; with no
    // failed click behind it, the region must stay empty.
    await act(async () => {
      root.render(
        <RecoveredSessionBar
          session={recoveredSession({ id: "rec-1", resumable: false })}
          onReopened={onReopened}
        />,
      );
    });
    expect(verdictStatus().textContent).toBe("");
  });

  it("does not carry the error or the Reopening… state into another session", async () => {
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    await mountBar(recoveredSession({ id: "rec-1" }));

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(
      container.querySelector('[data-testid="recovered-reopen-bar"] [role="alert"]')?.textContent,
    ).toContain("gone");

    await rerenderBar(recoveredSession({ id: "rec-2" }));
    const onOther = container.querySelector('[data-testid="recovered-reopen-bar"]');
    if (onOther === null) throw new Error("reopen bar did not render for the other session");
    expect(onOther.classList.contains("workspace-session-error")).toBe(false);
    expect(onOther.querySelector('[role="alert"]')).toBeNull();
    const otherButton = onOther.querySelector<HTMLButtonElement>("button");
    if (otherButton === null) throw new Error("Reopen button did not render");
    expect(otherButton.textContent).toBe("Reopen");
    expect(otherButton.disabled).toBe(false);

    // A resume attempt still in flight must not follow the pane either.
    vi.mocked(sessionResume).mockReturnValue(new Promise<ResumeResult>(() => {}));
    await rerenderBar(recoveredSession({ id: "rec-1" }));
    const pendingButton = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (pendingButton === null) throw new Error("Reopen button did not render");
    await act(async () => pendingButton.click());
    expect(pendingButton.textContent).toBe("Reopening…");
    expect(pendingButton.disabled).toBe(true);

    await rerenderBar(recoveredSession({ id: "rec-2" }));
    const settledButton = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (settledButton === null) throw new Error("Reopen button did not render");
    expect(settledButton.textContent).toBe("Reopen");
    expect(settledButton.disabled).toBe(false);
  });

  it("empties the region between two identical failures so the second is a new announcement", async () => {
    const sentence = "This transcript is read-only. Resume is not available for this session.";
    let failFirst!: (result: ResumeResult) => void;
    let failSecond!: (result: ResumeResult) => void;
    vi.mocked(sessionResume)
      .mockReturnValueOnce(
        new Promise<ResumeResult>((resolve) => {
          failFirst = resolve;
        }),
      )
      .mockReturnValueOnce(
        new Promise<ResumeResult>((resolve) => {
          failSecond = resolve;
        }),
      );
    const onResumeFailed = vi.fn();
    await mountBar(recoveredSession(), onResumeFailed);

    const clickReopen = async (): Promise<void> => {
      const button = container.querySelector<HTMLButtonElement>(
        '[data-testid="recovered-reopen-bar"] button',
      );
      if (button === null) throw new Error("Reopen button did not render");
      await act(async () => button.click());
    };

    await clickReopen();
    await act(async () => {
      failFirst({ type: "failed", message: "gone" });
    });
    expect(verdictStatus().textContent).toBe(sentence);

    // The retry begins: the region empties so the next failure transitions.
    await clickReopen();
    expect(verdictStatus().textContent).toBe("");
    await act(async () => {
      failSecond({ type: "failed", message: "gone" });
    });
    expect(verdictStatus().textContent).toBe(sentence);
  });

  it("states the read-only sentence exactly once after the roster flips the verdict", async () => {
    const sentence = "This transcript is read-only. Resume is not available for this session.";
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    const onResumeFailed = vi.fn();
    await mountBar(recoveredSession(), onResumeFailed);
    const beforeFlip = verdictStatus();

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(beforeFlip.textContent).toBe(sentence);

    await rerenderBar(recoveredSession({ resumable: false }), onResumeFailed);
    const occurrences = (container.textContent ?? "").split(sentence).length - 1;
    expect(occurrences).toBe(1);
    expect(verdictStatus()).toBe(beforeFlip);
    expect(beforeFlip.textContent).toBe("");
  });

  it("keeps a resume that settles after a session switch from acting on the new pane", async () => {
    const onReopened = vi.fn();
    const onResumeFailed = vi.fn();
    let succeed!: (result: ResumeResult) => void;
    let fail!: (result: ResumeResult) => void;
    vi.mocked(sessionResume).mockReturnValueOnce(
      new Promise<ResumeResult>((resolve) => {
        succeed = resolve;
      }),
    );

    const showSession = async (id: string): Promise<void> => {
      await act(async () => {
        root.render(
          <RecoveredSessionBar
            key={id}
            session={recoveredSession({ id })}
            onReopened={onReopened}
            onResumeFailed={onResumeFailed}
          />,
        );
      });
    };
    const clickReopen = async (): Promise<void> => {
      const button = container.querySelector<HTMLButtonElement>(
        '[data-testid="recovered-reopen-bar"] button',
      );
      if (button === null) throw new Error("Reopen button did not render");
      await act(async () => button.click());
    };

    root = createRoot(container);
    await showSession("rec-1");
    await clickReopen();

    // The pane moves while the resume is in flight.
    await showSession("rec-2");
    await act(async () => {
      succeed({ type: "resumed", session: recoveredSession({ id: "rec-1" }) });
    });
    expect(onReopened).not.toHaveBeenCalled();

    // The same for a failure: the roster refresh still runs, nothing else does.
    vi.mocked(sessionResume).mockReturnValueOnce(
      new Promise<ResumeResult>((resolve) => {
        fail = resolve;
      }),
    );
    await clickReopen();
    await showSession("rec-3");
    await act(async () => {
      fail({ type: "failed", message: "gone" });
    });
    expect(onResumeFailed).toHaveBeenCalledTimes(1);
    expect(onReopened).not.toHaveBeenCalled();
    const bar = container.querySelector('[data-testid="recovered-reopen-bar"]');
    expect(bar?.querySelector('[role="alert"]')).toBeNull();
    expect(verdictStatus().textContent).toBe("");
  });

  it("does not re-speak a stale verdict after switching away and back", async () => {
    const sentence = "This transcript is read-only. Resume is not available for this session.";
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    const onResumeFailed = vi.fn();
    await mountBar(recoveredSession({ id: "rec-1" }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(onResumeFailed).toHaveBeenCalledTimes(1);
    expect(verdictStatus().textContent).toBe(sentence);

    await rerenderBar(recoveredSession({ id: "rec-2" }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");

    await rerenderBar(recoveredSession({ id: "rec-1", resumable: false }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");
  });

  it("adds no announcement when the verdict changes without a click: re-flip and second session", async () => {
    const sentence = "This transcript is read-only. Resume is not available for this session.";
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    const onResumeFailed = vi.fn();
    await mountBar(recoveredSession(), onResumeFailed);

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(verdictStatus().textContent).toBe(sentence);

    // The refresh retracts the verdict; the visible note carries the sentence
    // from here on, and the region stays empty through a repair and a re-flip
    // — nothing speaks without a click.
    await rerenderBar(recoveredSession({ resumable: false }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");
    await rerenderBar(recoveredSession({ resumable: true }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");
    await rerenderBar(recoveredSession({ resumable: false }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");

    // A second session, never clicked, whose verdict flips on its own: silent.
    await rerenderBar(recoveredSession({ id: "rec-2" }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");
    await rerenderBar(recoveredSession({ id: "rec-2", resumable: false }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");
  });

  it.each([true, false])(
    "drops the error and the announcement at every verdict flip, starting resumable=%s",
    async (start) => {
      const sentence = "This transcript is read-only. Resume is not available for this session.";
      vi.mocked(sessionResume).mockResolvedValue({ type: "failed", message: "gone" });
      const alerts = (): number => container.querySelectorAll('[role="alert"]').length;
      const failReopen = async (): Promise<void> => {
        const button = container.querySelector<HTMLButtonElement>(
          '[data-testid="recovered-reopen-bar"] button',
        );
        if (button === null) throw new Error("Reopen button did not render");
        await act(async () => button.click());
        expect(alerts()).toBe(1);
        expect(verdictStatus().textContent).toBe(sentence);
      };
      const expectOnlyTheVerdict = (resumable: boolean): void => {
        expect(alerts()).toBe(0);
        expect(verdictStatus().textContent).toBe("");
        if (resumable) {
          const bar = container.querySelector('[data-testid="recovered-reopen-bar"]');
          expect(bar?.className).toBe("workspace-session-notice workspace-session-recovered");
          expect(bar?.textContent).toBe("Read-only transcript from the journal.Reopen");
        } else {
          const note = container.querySelector('[data-testid="recovered-unresumable"]');
          expect(note?.textContent).toBe(sentence);
          expect((container.textContent ?? "").split(sentence)).toHaveLength(2);
        }
      };

      await mountBar(recoveredSession({ resumable: start }), vi.fn());
      expectOnlyTheVerdict(start);
      let resumable = start;
      for (let flip = 0; flip < 3; flip++) {
        if (resumable) await failReopen();
        resumable = !resumable;
        await rerenderBar(recoveredSession({ resumable }), vi.fn());
        expectOnlyTheVerdict(resumable);
      }
    },
  );

  it("drops the armed outcome while the bar shows no session, and stays silent on return", async () => {
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    const onResumeFailed = vi.fn();
    await mountBar(recoveredSession({ id: "rec-1" }), onResumeFailed);

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());
    expect(onResumeFailed).toHaveBeenCalledTimes(1);

    await rerenderBar(null, onResumeFailed);
    expect(container.querySelector('[data-testid="recovered-verdict-status"]')).toBeNull();

    await rerenderBar(recoveredSession({ id: "rec-1", resumable: false }), onResumeFailed);
    expect(verdictStatus().textContent).toBe("");
  });

  it("is a neutral note with a reachable Reopen, not the error block", async () => {
    await renderBar(recoveredSession({ resumable: true }));

    const bar = container.querySelector('[data-testid="recovered-reopen-bar"]');
    if (bar === null) throw new Error("reopen bar did not render");
    expect(bar.getAttribute("role")).toBeNull();
    expect(bar.getAttribute("aria-live")).toBeNull();
    expect(bar.classList.contains("workspace-session-recovered")).toBe(true);
    expect(bar.classList.contains("workspace-session-error")).toBe(false);
    expect(bar.textContent).toContain("Read-only transcript from the journal.");
    const button = bar.querySelector<HTMLButtonElement>("button");
    if (button === null) throw new Error("Reopen did not render");
    expect(button.disabled).toBe(false);
    button.focus();
    expect(document.activeElement).toBe(button);
  });

  it("arms the danger block and the alert only for a failed reopen", async () => {
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "gone" });
    await renderBar(recoveredSession({ resumable: true }));

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen did not render");
    await act(async () => button.click());

    const bar = container.querySelector('[data-testid="recovered-reopen-bar"]');
    if (bar === null) throw new Error("reopen bar disappeared");
    expect(bar.classList.contains("workspace-session-error")).toBe(true);
    expect(bar.classList.contains("workspace-session-recovered")).toBe(false);
    expect(bar.querySelector('[role="alert"]')?.textContent).toContain("gone");
  });

  it("paints the quiet note in the neutral tokens: muted text, no border, no fill", async () => {
    await renderBar(recoveredSession({ resumable: true }));

    const css = assembleCssProof(sheets);
    const targets = css.rules
      .flatMap((rule) => rule.selector.split(","))
      .map((part) => part.trim())
      .filter(
        (part) =>
          part.includes("workspace-session-recovered") ||
          part.includes("workspace-session-notice-text"),
      );
    css.inject(targets);
    const bar = container.querySelector('[data-testid="recovered-reopen-bar"]');
    if (bar === null) throw new Error("reopen bar did not render");
    const style = getComputedStyle(bar);
    expect(style.color).toBe(css.token("--muted"));
    expect(style.fontSize).toBe(css.token("--type-meta"));
    // The rule carries no danger, no border and no fill of its own; the danger
    // colours live on .workspace-session-error, which this bar does not wear.
    const own = css.rulesFor(".workspace-session-recovered");
    expect(own).not.toContain("danger");
    expect(own).not.toContain("background");
    expect(own).not.toContain("border");
  });

  it("keeps the Reopen control's hover and focus answers neutral inside the quiet note", () => {
    const css = assembleCssProof(sheets);
    const ink = css.token("--ink");
    const danger = css.token("--danger");
    expect(ink).toBeDefined();
    expect(danger).toBeDefined();
    for (const pseudo of [":hover", ":focus-visible"]) {
      const own = css.rulesFor(`.workspace-session-recovered .workspace-secondary-action${pseudo}`);
      expect(own).toContain(ink!);
      expect(own).not.toContain(danger!);
    }
    // The shared secondary-action answer is the one that flashes danger;
    // specificity, not sheet order, must decide which one wins.
    expect(css.rulesFor(".workspace-secondary-action:hover")).toContain(danger!);
    expect(specificity(".workspace-session-recovered .workspace-secondary-action:hover")).toEqual([
      0, 3, 0,
    ]);
    expect(specificity(".workspace-secondary-action:hover")).toEqual([0, 2, 0]);
  });
});
