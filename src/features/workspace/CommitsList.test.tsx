// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ErrorSentence } from "../../lib/errorSentence";
import type { WorkspaceGitCommitEntry, WorkspaceGitLog } from "../../types/ipc";
import { CommitsList } from "./CommitsList";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function commitEntry(overrides: Partial<WorkspaceGitCommitEntry> = {}): WorkspaceGitCommitEntry {
  return {
    sha: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
    shortSha: "a1b2c3d",
    subject: "Add the thing",
    authorName: "gualt",
    authorDate: "2026-09-01T10:00:00+00:00",
    isOnRemote: true,
    isOnBase: false,
    ...overrides,
  };
}

function logReply(overrides: Partial<WorkspaceGitLog> = {}): WorkspaceGitLog {
  return {
    baseRef: "origin/main",
    commits: [commitEntry()],
    error: null,
    ...overrides,
  };
}

function failure(sentence: string): ErrorSentence {
  return { sentence, detail: null };
}

describe("CommitsList", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
    });
  }

  it("renders the reply's commits with sha, subject, author and a relative time", async () => {
    await render(
      <CommitsList
        log={logReply({
          commits: [
            commitEntry({ shortSha: "a1b2c3d", subject: "Add the thing", authorName: "gualt" }),
            commitEntry({
              sha: "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3",
              shortSha: "b2c3d4e",
              subject: "Fix the other",
              authorName: "someone-else",
            }),
          ],
        })}
        failure={null}
      />,
    );

    const rows = container.querySelectorAll(".workspace-commits-row");
    expect(rows).toHaveLength(2);
    expect(container.querySelector(".workspace-commits-sha")?.textContent).toBe("a1b2c3d");
    expect(container.querySelector(".workspace-commits-subject")?.textContent).toBe(
      "Add the thing",
    );
    expect(container.querySelector(".workspace-commits-author")?.textContent).toBe("gualt");
    // A relative time, not a raw ISO stamp: the helper's own words.
    expect(container.querySelector(".workspace-commits-time")?.textContent).toContain("ago");
    expect(container.textContent).not.toContain("2026-09-01");
    // The full subject rides the hover of the truncated one.
    expect(container.querySelector(".workspace-commits-subject")?.getAttribute("title")).toBe(
      "Add the thing",
    );
  });

  // Paseo's function: the section shows the branch's own commits
  // (commits-section.tsx:61-70 filters the base half out). The order is
  // the author dates' — the wire's only date — so the time column cannot
  // jump backwards; equal dates keep the wire's order (stable sort).
  it("shows only the branch's own commits, newest first by their author dates", async () => {
    await render(
      <CommitsList
        log={logReply({
          commits: [
            // The wire's order: newest by commit date. The middle row's
            // author date is the oldest of the three — a rebased or
            // amended commit — so the author-date sort moves it last.
            commitEntry({ shortSha: "a1b2c3d", subject: "Newest work" }),
            commitEntry({
              shortSha: "b2c3d4e",
              subject: "Rebased work",
              authorDate: "2026-01-01T10:00:00+00:00",
            }),
            commitEntry({
              shortSha: "c3d4e5f",
              subject: "Base history",
              isOnBase: true,
            }),
          ],
        })}
        failure={null}
      />,
    );

    const rows = Array.from(container.querySelectorAll(".workspace-commits-row"));
    expect(rows).toHaveLength(2);
    expect(rows[0].querySelector(".workspace-commits-subject")?.textContent).toBe("Newest work");
    expect(rows[1].querySelector(".workspace-commits-subject")?.textContent).toBe("Rebased work");
    // The base branch's history is filtered out, not dimmed.
    expect(container.textContent).not.toContain("Base history");
    // The time column is monotonic down the list.
    const times = rows.map(
      (row) => row.querySelector(".workspace-commits-time")?.textContent ?? "",
    );
    expect(times[0]).not.toBe("");
    expect(times[1]).not.toBe("");
  });

  it("holds a loading state until the first answer arrives", async () => {
    await render(<CommitsList log={null} failure={null} />);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Loading commits…");
    expect(container.querySelector(".workspace-commits-row")).toBeNull();
  });

  it("says the branch sits at its base when nothing is ahead of it", async () => {
    await render(
      <CommitsList
        log={logReply({
          commits: [commitEntry({ shortSha: "b2c3d4e", isOnBase: true })],
        })}
        failure={null}
      />,
    );

    expect(container.querySelector(".workspace-commits-state")?.textContent).toBe(
      "No commits ahead of origin/main",
    );
    expect(container.querySelector(".workspace-commits-row")).toBeNull();
  });

  // The wire's two causes for an empty, baseRef-less answer — a
  // repository with no commit yet, and a detached HEAD outside a rebase
  // (`workspace_git_log.rs:90-95`) — share one sentence that is true for
  // both: there is nothing to show either way.
  it("says there is nothing to show when the answer names no base and no commits", async () => {
    await render(<CommitsList log={logReply({ baseRef: null, commits: [] })} failure={null} />);

    expect(container.querySelector(".workspace-commits-state")?.textContent).toBe(
      "No commits to show",
    );
  });

  // A live failure must win over an older refusal kept in the cell: the
  // 30 s poll can leave the refusal as the reply while a newer read
  // throws, and the present-tense problem must show first.
  it("shows a live failure ahead of an older refusal kept in the cell", async () => {
    const refusal = "the base branch main is not local and not on origin";
    await render(
      <CommitsList
        log={logReply({ commits: [], error: refusal })}
        failure={failure("The connection to the agent daemon was lost. Devboule is reconnecting.")}
      />,
    );

    const alerts = Array.from(container.querySelectorAll('[role="alert"]'));
    expect(alerts).toHaveLength(2);
    expect(alerts[0]?.textContent).toBe(
      "The connection to the agent daemon was lost. Devboule is reconnecting.",
    );
    expect(alerts[1]?.textContent).toBe(refusal);
    expect(container.querySelector(".workspace-commits-state")).toBeNull();
  });

  // The wire's rule is that the panel shows the daemon's words: a
  // baseRef-less answer gets a sentence that is true without a ref,
  // never the word "base" standing in for one that does not exist.
  it("never prints the word 'base' as a ref when the answer names none", async () => {
    await render(
      <CommitsList
        log={logReply({ baseRef: null, commits: [commitEntry({ isOnBase: true })] })}
        failure={null}
      />,
    );

    expect(container.querySelector(".workspace-commits-state")?.textContent).toBe(
      "No commits to show",
    );
    expect(container.textContent).not.toContain("ahead of base");
  });

  // The wire's refusal arm: an empty list WITH a sentence — the daemon's
  // own words, alone. Not the empty-tree claim with the reason discarded.
  it("shows a wire refusal as the daemon's sentence, alone", async () => {
    const refusal = "the base branch main is not local and not on origin";
    await render(<CommitsList log={logReply({ commits: [], error: refusal })} failure={null} />);

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(refusal);
    expect(container.querySelector(".workspace-commits-row")).toBeNull();
    expect(container.querySelector(".workspace-commits-state")).toBeNull();
    expect(container.textContent).not.toContain("No commits ahead");
    expect(container.textContent).not.toContain("No commits yet");
  });

  it("keeps the list beside the sentence that cut it short", async () => {
    const cut = "git log produced more than the reply cap; the oldest commits are missing";
    await render(
      <CommitsList
        log={logReply({ error: cut, commits: [commitEntry({ shortSha: "a1b2c3d" })] })}
        failure={null}
      />,
    );

    expect(container.querySelector(".workspace-commits-row")).not.toBeNull();
    expect(container.querySelector(".workspace-commits-note")?.textContent).toBe(cut);
  });

  it("keeps the last list beside a refusal that arrives later", async () => {
    await render(<CommitsList log={logReply()} failure={null} />);
    expect(container.querySelector(".workspace-commits-row")).not.toBeNull();

    await act(async () => {
      root.render(<CommitsList log={logReply()} failure={failure("the daemon is restarting")} />);
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toBe("the daemon is restarting");
    expect(container.querySelector(".workspace-commits-row")).not.toBeNull();
  });
});
