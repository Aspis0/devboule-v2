// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { DiffCard } from "./DiffCard";
import type { WorkspaceGitFileDiff } from "../../types/ipc";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("DiffCard", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it("names added and removed rows for screen readers", async () => {
    const reply: WorkspaceGitFileDiff = {
      path: "a.ts",
      isNew: false,
      isDeleted: false,
      additions: 1,
      deletions: 1,
      status: "ok",
      error: null,
      lines: [
        { kind: "remove", text: "old" },
        { kind: "add", text: "new" },
      ],
    };
    await act(async () => {
      root.render(<DiffCard path="a.ts" diff={{ reply, failure: null }} />);
    });
    const rows = [...container.querySelectorAll(".workspace-diff-line")];
    expect(rows).toHaveLength(2);
    expect(rows[0]?.querySelector(".sr-only")?.textContent).toBe("removed");
    expect(rows[1]?.querySelector(".sr-only")?.textContent).toBe("added");
    const markers = [...container.querySelectorAll(".workspace-diff-line > span:first-child")];
    expect(markers).toHaveLength(2);
    for (const marker of markers) expect(marker.getAttribute("aria-hidden")).toBe("true");
  });
});
