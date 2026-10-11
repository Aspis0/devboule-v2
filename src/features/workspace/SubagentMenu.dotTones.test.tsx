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

// Keeps the line up: a settled child alone draws no line, so no list would open.
const runningChild = {
  id: "child-running",
  kind: "acp" as const,
  title: "Still working",
  createdBy: "parent",
  state: { type: "live" as const, generation: 1 },
  activity: "working" as const,
};

type Child = typeof failedChild | typeof waitingChild | typeof runningChild;

async function renderParent(
  sessionRoster: readonly Child[],
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
    container.querySelector<HTMLButtonElement>('[data-testid="who-is-working"]')?.click(),
  );
}

function rowDot(title: string): Element | null {
  const row = [
    ...document.querySelectorAll<HTMLButtonElement>("button.workspace-subagent-row"),
  ].find((button) => button.getAttribute("aria-label")?.startsWith(title));
  return row?.querySelector(".workspace-subagent-status-dot") ?? null;
}

describe("subagent dots: a child waiting on a person is ochre, an exit code is not a failure", () => {
  it("paints a child that ended on an exit code as stopped, never failed", async () => {
    await renderParent([failedChild, runningChild]);
    const dot = rowDot("Crashed child");
    expect(dot?.classList.contains("workspace-subagent-status-stopped")).toBe(true);
    expect(dot?.classList.contains("workspace-subagent-status-failed")).toBe(false);
    expect(dot?.classList.contains("workspace-subagent-status-attention")).toBe(false);
  });

  it("paints a child waiting on a permission card with the attention class, not the running one", async () => {
    await renderParent([waitingChild], new Map([["child-waiting", "Needs your approval"]]));
    const dot = rowDot("Asks for a command");
    expect(dot?.classList.contains("workspace-subagent-status-attention")).toBe(true);
    expect(dot?.classList.contains("workspace-subagent-status-running")).toBe(false);
    expect(dot?.classList.contains("workspace-subagent-status-failed")).toBe(false);
  });

  it("paints the line's approval dot with the attention tone, never the danger one", async () => {
    await renderParent([waitingChild], new Map([["child-waiting", "Needs your approval"]]));
    const line = container.querySelector<HTMLButtonElement>('[data-testid="who-is-working"]');
    expect(line?.querySelector(".workspace-who-dot-attention")).not.toBeNull();
    expect(line?.querySelector(".workspace-who-dot-danger")).toBeNull();
  });

  it("keeps a failed child off the line: its failure shows in the list the line opens", async () => {
    await renderParent([failedChild, runningChild]);
    const line = container.querySelector<HTMLButtonElement>('[data-testid="who-is-working"]');
    expect(line?.getAttribute("aria-label")).not.toContain("failed");
    expect(line?.querySelector(".workspace-who-dot-danger")).toBeNull();
    expect(line?.textContent).not.toContain("failed");
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
