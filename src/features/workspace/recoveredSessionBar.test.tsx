// Human-path proofs for the recovered reopen bar: the daemon's verdict,
// never a re-derivation, and never an auto-resume on mount. The quiet
// note's own tokens are proved through the real sheet.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../../types/ipc";

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
  removeCssProof();
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

  it("renders nothing for a recovered terminal: the pane carries that failure's one sentence", async () => {
    await renderBar(recoveredSession({ kind: "terminal", resumable: false }));

    expect(container.textContent).toBe("");
    expect(sessionResume).not.toHaveBeenCalled();
  });
});

describe("the recovered bar's quiet state", () => {
  it("is a neutral status note with a reachable Reopen, not the error block", async () => {
    await renderBar(recoveredSession({ resumable: true }));

    const bar = container.querySelector('[data-testid="recovered-reopen-bar"]');
    if (bar === null) throw new Error("reopen bar did not render");
    expect(bar.getAttribute("role")).toBe("status");
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
