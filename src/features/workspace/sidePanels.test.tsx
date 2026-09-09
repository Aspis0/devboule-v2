// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AppSurface, ChangesSurface, DesignPanel, FilesSurface } from "./sidePanels";
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
      },
    });
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
    });
  }

  describe("ChangesSurface", () => {
    it("labels the hardcoded rows as a mockup, not a real diff", async () => {
      await render(<ChangesSurface />);

      const note = container.querySelector('[role="note"]');
      if (note === null) throw new Error("mockup notice did not render");
      expect(note.textContent).toBe(
        "Mockup — these rows are hardcoded examples. Real git integration is not built yet.",
      );
    });

    it("renders the file rows as non-interactive elements, not buttons", async () => {
      await render(<ChangesSurface />);

      expect(container.querySelectorAll("button")).toHaveLength(0);
      expect(container.querySelectorAll(".workspace-file-change")).toHaveLength(3);
    });

    it("no longer renders the fake git operations Stage and Discard", async () => {
      await render(<ChangesSurface />);

      // Anchor on the panel's real content first: if ChangesSurface ever stops
      // rendering its rows, this test must fail rather than pass vacuously on
      // an empty button list.
      expect(container.querySelectorAll(".workspace-file-change")).toHaveLength(3);
      expect(container.textContent).toContain("index_writer.rs");
      const controls = Array.from(container.querySelectorAll("button, [role='button']")).map(
        (control) => control.textContent,
      );
      expect(controls).not.toContain("Stage");
      expect(controls).not.toContain("Discard");
    });
  });

  describe("FilesSurface", () => {
    it("renders the tree entries as non-interactive elements, not buttons", async () => {
      await render(<FilesSurface />);

      expect(container.querySelectorAll("button")).toHaveLength(0);
      expect(container.querySelectorAll(".workspace-tree-file")).toHaveLength(4);
    });

    it("labels the hardcoded tree as a mockup", async () => {
      await render(<FilesSurface />);

      const note = container.querySelector('[role="note"]');
      if (note === null) throw new Error("mockup notice did not render");
      expect(note.textContent).toBe(
        "Mockup — these files are hardcoded examples. No workspace file tree is read yet.",
      );
    });
  });

  describe("AppSurface", () => {
    it("no longer renders the dead Reindex and Export buttons", async () => {
      await render(<AppSurface appBuild={41} onReload={() => undefined} />);

      // Anchor on the panel's real content first so the label assertions below
      // cannot pass on an unrendered panel.
      expect(container.querySelector(".workspace-browser-reload")).not.toBeNull();
      expect(container.textContent).toContain("web.rust-core.devboule.localhost");
      const labels = Array.from(container.querySelectorAll("button")).map(
        (button) => button.textContent,
      );
      expect(labels).not.toContain("Reindex");
      expect(labels).not.toContain("Export");
    });

    it("keeps the wired reload button", async () => {
      let reloads = 0;
      await render(
        <AppSurface
          appBuild={41}
          onReload={() => {
            reloads += 1;
          }}
        />,
      );

      const reload = container.querySelector<HTMLButtonElement>(".workspace-browser-reload");
      if (reload === null) throw new Error("reload button did not render");
      await act(async () => {
        reload.click();
      });
      expect(reloads).toBe(1);
    });

    it("labels the static page as a mockup", async () => {
      await render(<AppSurface appBuild={41} onReload={() => undefined} />);

      const note = container.querySelector('[role="note"]');
      if (note === null) throw new Error("mockup notice did not render");
      expect(note.textContent).toBe(
        "Mockup — this browser page is a static example. The dev-server preview is not built yet.",
      );
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
