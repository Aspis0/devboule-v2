// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ChangesSurface, type DiffState } from "./sidePanels";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("ChangesSurface mockup notice", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.clearAllMocks();
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it("labels the hardcoded rows as a mockup, not a real diff", async () => {
    const onDiffStateChange = vi.fn((_state: DiffState) => undefined);
    root = createRoot(container);
    await act(async () => {
      root.render(<ChangesSurface diffState="unstaged" onDiffStateChange={onDiffStateChange} />);
    });

    const note = container.querySelector('[role="note"]');
    if (note === null) throw new Error("mockup notice did not render");
    expect(note.textContent).toBe(
      "Mockup — these rows are hardcoded examples. Real git integration is not built yet.",
    );
  });
});
