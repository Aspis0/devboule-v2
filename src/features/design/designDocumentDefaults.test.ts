import { describe, expect, it } from "vitest";
import { createAgentHost } from "./agentHost";
import { createDesignDocumentDefaults } from "./designDocumentDefaults";

describe("design document defaults", () => {
  it("carries the application defaults the surface reads", () => {
    expect(createDesignDocumentDefaults()).toEqual({
      contextPrefix: "Editing",
      draftPlaceholder: "Describe the change to Index header…",
      noContextPlaceholder: "Describe what to generate…",
      initialState: {
        zoom: 1,
        saved: false,
        draft: "",
        hiddenLayerIds: [],
      },
      grounded: true,
    });
  });

  it("hands out fresh copies so hosts never share array identity", () => {
    const first = createDesignDocumentDefaults();
    const second = createDesignDocumentDefaults();

    expect(first.initialState).not.toBe(second.initialState);
    expect(first.initialState.hiddenLayerIds).not.toBe(second.initialState.hiddenLayerIds);
  });

  it("flows the same values into every agent host document", async () => {
    const defaults = createDesignDocumentDefaults();
    const document = await createAgentHost().loadDocument();

    expect(document.contextPrefix).toBe(defaults.contextPrefix);
    expect(document.draftPlaceholder).toBe(defaults.draftPlaceholder);
    expect(document.noContextPlaceholder).toBe(defaults.noContextPlaceholder);
    expect(document.initialState).toEqual(defaults.initialState);
    expect(document.grounded).toBe(defaults.grounded);
  });

  it("carries no inspector-era ghost fields", async () => {
    // tokenFooter, radiusOptions, and the radius/flat snapshot state lost
    // their only reader when the inspector went away. Exact matching keeps
    // them from quietly reigniting as dead document weight.
    expect(createDesignDocumentDefaults()).toEqual({
      contextPrefix: expect.any(String),
      draftPlaceholder: expect.any(String),
      noContextPlaceholder: expect.any(String),
      initialState: {
        zoom: expect.any(Number),
        saved: expect.any(Boolean),
        draft: expect.any(String),
        hiddenLayerIds: [],
      },
      grounded: expect.any(Boolean),
    });
    const document = await createAgentHost().loadDocument();
    expect(document).not.toHaveProperty("tokenFooter");
    expect(document).not.toHaveProperty("radiusOptions");
    expect(document.initialState).not.toHaveProperty("radius");
    expect(document.initialState).not.toHaveProperty("flat");
  });
});
