// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AppSurface, DesignPanel, PullRequestSurface } from "./sidePanels";
import { useAppStore } from "../../store/appStore";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("side panel dead controls", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    useAppStore.setState({ activeSurface: "workspace" });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    useAppStore.setState({ activeSurface: "workspace" });
    // DesignPreviewPanel reads the live session; leave it empty for the next test.
    useAppStore.setState({
      designSession: {
        host: null,
        document: null,
        messages: [],
        latestArtifact: null,
        generation: null,
        sectionNotes: [],
      },
    });
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
    });
  }

  // The Changes panel's tests moved to `ChangesSurface.test.tsx` (and, for
  // its row acts, to `ChangesSurface.actions.test.tsx`) and the Files
  // panel's to `FilesSurface.test.tsx` (and, for the Files panel's row
  // acts, to `FilesFileActions.test.tsx`): both panels render real data
  // now, so their guarantees — which controls each panel may draw (and
  // that the one act that loses data asks first), and no mockup notice —
  // are anchored there against real content instead of the mock's.
  describe("AppSurface", () => {
    it("names the panel and says what will live here", async () => {
      await render(<AppSurface />);

      expect(container.textContent).toContain("Interactive app");
      expect(container.textContent).toContain("not available yet");
    });

    it("carries no controls and no mock browser", async () => {
      await render(<AppSurface />);

      expect(container.querySelectorAll("button, input, select, textarea, a")).toHaveLength(0);
      expect(container.textContent).not.toContain("Mockup");
      expect(container.querySelector('[role="note"]')).toBeNull();
    });
  });

  describe("PullRequestSurface", () => {
    it("names the panel and says what will live here", async () => {
      await render(<PullRequestSurface />);

      expect(container.textContent).toContain("Pull request");
      expect(container.textContent).toContain("not available yet");
    });

    it("carries no controls and no mock ship card", async () => {
      await render(<PullRequestSurface />);

      expect(container.querySelectorAll("button, input, select, textarea, a")).toHaveLength(0);
      expect(container.textContent).not.toContain("#412");
      expect(container.querySelector('[role="note"]')).toBeNull();
    });
  });

  describe("DesignPanel", () => {
    it("Open Design selects the design surface in the app store", async () => {
      await render(<DesignPanel />);

      const openDesign = container.querySelector<HTMLButtonElement>(".workspace-open-design");
      if (openDesign === null) throw new Error("Open Design button did not render");
      await act(async () => {
        openDesign.click();
      });

      expect(useAppStore.getState().activeSurface).toBe("design");
    });

    it("no longer renders the dead composer textarea or Generate button", async () => {
      useAppStore.setState({
        designSession: {
          host: {
            loadDocument: async () => {
              throw new Error("not used in this test");
            },
          },
          document: null,
          messages: [],
          latestArtifact: { html: "<p>latest artifact</p>" },
          generation: null,
          sectionNotes: [],
        },
      });
      await render(<DesignPanel />);

      // Anchor on the panel's real content first so the absence assertions
      // below cannot pass on an unrendered panel.
      expect(container.querySelector(".workspace-generation-card")).not.toBeNull();
      expect(container.querySelector(".workspace-open-design")).not.toBeNull();
      expect(container.querySelector("textarea")).toBeNull();
      const labels = Array.from(container.querySelectorAll("button")).map(
        (button) => button.textContent,
      );
      expect(labels).not.toContain("Generate");
    });

    it("no longer claims to be a mockup and carries no hardcoded generation", async () => {
      await render(<DesignPanel />);

      // Positive anchor: the panel must render its live-session row, the Open Design
      // control and the not-opened note, so the absence assertions below cannot pass on
      // a panel that renders nothing at all.
      expect(container.querySelector(".workspace-grounding-row")).not.toBeNull();
      expect(container.querySelector(".workspace-open-design")).not.toBeNull();
      expect(container.textContent).toContain("Design has not been opened in this session yet");

      expect(container.textContent).not.toContain("Mockup");
      // The invented generation card from the mockup must be gone for good.
      expect(container.textContent).not.toContain("Edited Index header");
      expect(container.querySelector('[role="note"]')).toBeNull();
    });
  });
});
