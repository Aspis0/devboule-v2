// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ saveArtifactHtml: vi.fn() }));

vi.mock("./artifactSave", () => ({ saveArtifactHtml: mocks.saveArtifactHtml }));

import { ArtifactSaveControl } from "./ArtifactSaveControl";

const FRAGMENT = "<main><h1>Our menu</h1></main>";
const RUN_TITLE = "Agent did not report written files";

describe("ArtifactSaveControl", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    mocks.saveArtifactHtml.mockReset();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
  });

  async function render(): Promise<void> {
    await act(async () => {
      root.render(<ArtifactSaveControl html={FRAGMENT} title={RUN_TITLE} />);
    });
  }

  function saveButton(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>('button[aria-label="Save HTML"]');
    if (button === null) throw new Error("the save button is missing");
    return button;
  }

  async function click(): Promise<void> {
    await act(async () => {
      saveButton().click();
    });
  }

  it("offers one labelled button and says nothing before it is used", async () => {
    await render();

    expect(container.querySelectorAll("button")).toHaveLength(1);
    expect(saveButton().textContent).toBe("Save HTML");
    expect(saveButton().title).toBe("Save the generated page as a standalone HTML file");
    expect(container.textContent).toBe("Save HTML");
    expect(mocks.saveArtifactHtml).not.toHaveBeenCalled();
  });

  it("passes the live fragment and title to the flow", async () => {
    mocks.saveArtifactHtml.mockResolvedValue({ status: "cancelled" });
    await render();

    await click();

    expect(mocks.saveArtifactHtml).toHaveBeenCalledWith(FRAGMENT, RUN_TITLE);
  });

  it("shows the saved path as a status message", async () => {
    mocks.saveArtifactHtml.mockResolvedValue({ status: "saved", path: "C:/tmp/Our menu.html" });
    await render();

    await click();

    expect(container.textContent).toBe("Save HTMLSaved to C:/tmp/Our menu.html.");
    const status = container.querySelector('[role="status"]');
    expect(status?.textContent).toBe("Saved to C:/tmp/Our menu.html.");
    expect(status?.getAttribute("title")).toBe("Saved to C:/tmp/Our menu.html");
  });

  it("says nothing at all when the save is cancelled", async () => {
    mocks.saveArtifactHtml.mockResolvedValue({ status: "cancelled" });
    await render();

    await click();

    // A cancel is not a failure: no failure text, no status region, no claim
    // that anything was written.
    expect(container.textContent).toBe("Save HTML");
    expect(container.querySelector('[role="status"]')).toBeNull();
  });

  it("shows a failure the user can diagnose, with the cause in the tooltip", async () => {
    mocks.saveArtifactHtml.mockResolvedValue({
      status: "failed",
      message: "writing `C:/tmp/Our menu.html` failed: Access is denied. (os error 5)",
    });
    await render();

    await click();

    expect(container.textContent).toBe("Save HTMLSave failed.");
    const status = container.querySelector('[role="status"]');
    expect(status?.textContent).toBe("Save failed.");
    expect(status?.getAttribute("title")).toBe(
      "writing `C:/tmp/Our menu.html` failed: Access is denied. (os error 5)",
    );
  });

  it("clears a failure when the next attempt is cancelled", async () => {
    mocks.saveArtifactHtml.mockResolvedValueOnce({ status: "failed", message: "disk full" });
    await render();
    await click();
    expect(container.textContent).toContain("Save failed.");

    mocks.saveArtifactHtml.mockResolvedValueOnce({ status: "cancelled" });
    await click();

    expect(container.textContent).not.toContain("Save failed.");
    expect(container.textContent).toBe("Save HTML");
  });

  it("retires the saved path after its reading window", async () => {
    vi.useFakeTimers();
    mocks.saveArtifactHtml.mockResolvedValue({ status: "saved", path: "C:/tmp/Our menu.html" });
    await render();
    await click();
    expect(container.textContent).toContain("Saved to C:/tmp/Our menu.html.");

    await act(async () => {
      vi.advanceTimersByTime(6_000);
    });

    expect(container.textContent).toBe("Save HTML");
  });
});
