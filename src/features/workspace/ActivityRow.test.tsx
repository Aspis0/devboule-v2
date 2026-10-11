// @vitest-environment happy-dom

// Every agent line shares one shape: the same icon slot ahead of the words, so an
// agent message and a Devboule agent call read as the same kind of line.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ActivityRow } from "./ActivityRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root | null = null;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
});

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  container.remove();
});

describe("ActivityRow", () => {
  it("leads its line with the agent icon, in the slot every agent line uses", async () => {
    root = createRoot(container);
    await act(async () => {
      root?.render(<ActivityRow summary="Sent a message to Finish Kalsa website" detail="words" />);
    });
    const line = container.querySelector(".workspace-chat-activity-line");
    expect(line?.firstElementChild?.classList.contains("workspace-chat-agent-icon")).toBe(true);
    expect(line?.textContent).toBe("Sent a message to Finish Kalsa website");
  });
});
