// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentChatSurface } from "./AgentChatSurface";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const css = readFileSync(resolve(import.meta.dirname, "SubagentMenu.css"), "utf8");

/** The body of the top-level rule whose selector is exactly `selector`. */
function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = css.match(new RegExp(`^${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`, "m"))?.[1];
  if (body === undefined) throw new Error(`no rule in SubagentMenu.css for ${selector}`);
  return body;
}

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
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
});

const failedChild = {
  id: "child-failed",
  kind: "acp" as const,
  title: "Crashed child",
  createdBy: "parent",
  state: {
    type: "ended" as const,
    generation: 1,
    code: 1,
    integrity: { kind: "complete" as const },
  },
};

const waitingChild = {
  id: "child-waiting",
  kind: "acp" as const,
  title: "Asks for a command",
  createdBy: "parent",
  state: { type: "live" as const, generation: 1 },
  activity: "blocked" as const,
};

async function renderParent(
  sessionRoster: readonly (typeof failedChild | typeof waitingChild)[],
  subagentAttention?: ReadonlyMap<string, string>,
) {
  await act(async () =>
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="parent"
        sessionRoster={sessionRoster}
        subagentAttention={subagentAttention}
      />,
    ),
  );
  await act(async () =>
    container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]')?.click(),
  );
}

function rowDot(title: string): Element | null {
  const row = [
    ...document.querySelectorAll<HTMLButtonElement>("button.workspace-subagent-row"),
  ].find((button) => button.getAttribute("aria-label")?.startsWith(title));
  return row?.querySelector(".workspace-subagent-status-dot") ?? null;
}

describe("subagent dots: a failed child is red, a child waiting on a person is ochre", () => {
  it("paints a failed child's row dot with the failed class and no other state class", async () => {
    await renderParent([failedChild]);
    const dot = rowDot("Crashed child");
    expect(dot?.classList.contains("workspace-subagent-status-failed")).toBe(true);
    expect(dot?.classList.contains("workspace-subagent-status-attention")).toBe(false);
  });

  it("paints a child waiting on a permission card with the attention class, not the running one", async () => {
    await renderParent([waitingChild], new Map([["child-waiting", "Needs your approval"]]));
    const dot = rowDot("Asks for a command");
    expect(dot?.classList.contains("workspace-subagent-status-attention")).toBe(true);
    expect(dot?.classList.contains("workspace-subagent-status-running")).toBe(false);
    expect(dot?.classList.contains("workspace-subagent-status-failed")).toBe(false);
  });

  it("paints the pill's approval dot with the attention class, never the failed one", async () => {
    await renderParent([waitingChild], new Map([["child-waiting", "Needs your approval"]]));
    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    const dot = pill?.querySelector(".workspace-subagent-attention .workspace-subagent-status-dot");
    expect(dot?.classList.contains("workspace-subagent-status-attention")).toBe(true);
    expect(dot?.classList.contains("workspace-subagent-status-failed")).toBe(false);
  });

  it("paints the pill's failed count with the failed class", async () => {
    await renderParent([failedChild]);
    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    const dot = pill?.querySelector(".workspace-subagent-status-dot");
    expect(dot?.classList.contains("workspace-subagent-status-failed")).toBe(true);
  });

  it("gives the failed class the failed token, and the attention class the approval token", () => {
    expect(ruleBody(".workspace-subagent-status-failed")).toContain(
      "background: var(--tone-failed);",
    );
    expect(ruleBody(".workspace-subagent-status-attention")).toContain(
      "background: var(--tone-attention);",
    );
  });

  it("writes a failed archive sentence in the danger tone, not the attention one", () => {
    expect(ruleBody(".workspace-subagent-row-failure")).toContain("color: var(--danger);");
  });
});
