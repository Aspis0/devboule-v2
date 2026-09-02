// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { PALETTE } from "./palette";
import { renderAgentRoster } from "./roster";

describe("host session roster", () => {
  it("shows unknown and known provider chips without placing file-less agents", () => {
    const element = document.createElement("div");
    renderAgentRoster(
      element,
      [
        { id: "unknown-1", provider: null, state: "working", fileId: null, title: "Shell" },
        {
          id: "open-1",
          provider: "opencode",
          state: "finished",
          fileId: null,
          title: "OpenCode task",
        },
      ],
      new Set(["src/main.ts"]),
    );

    expect(element.textContent).toContain("Shell");
    expect(element.textContent).toContain("Unknown provider");
    expect(element.textContent).toContain("OpenCode");
    expect(element.textContent).toContain("finished");
    const chips = [...element.querySelectorAll(".polis-roster-provider")];
    expect(chips).toHaveLength(2);
    expect(chips[1]?.getAttribute("data-color")).toBe(PALETTE.providerOpenCode.toString(16));
    expect(element.textContent).not.toContain("src/main.ts");
  });
});
