// @vitest-environment happy-dom

// The provider picker's scroll contract: the "Choose agent" heading sits
// outside the options list, every option is inside it, and focusing an option
// scrolls that list — never an ancestor — so a keyboard user can reach the
// providers below the fold. Happy DOM has no layout, so the scroll cases stub
// the box the browser reports (clientHeight, scrollTop, offsetTop) on the real
// elements, the way the command-menu scroll test does.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import type { ProviderInfo } from "../../types/ipc";
import { DesignAgentPicker } from "./DesignAgentPicker";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const PROVIDER_IDS = [
  "claude",
  "codex",
  "grok",
  "gemini",
  "opencode",
  "crush",
  "droid",
  "amp",
  "aider",
  "cline",
  "roo",
  "kilo",
  "qwen",
  "copilot",
  "continue",
  "zed",
  "cursor-agent",
  "pi",
  "goose",
  "devin",
  "bolt",
  "windsurf",
];

function provider(id: string): ProviderInfo {
  return {
    id,
    executable: id,
    acpAvailable: true,
    authentication: "unknown",
    protocol: "acp",
    origin: "user-binary",
  };
}

let root: ReturnType<typeof createRoot> | null = null;
let container: HTMLDivElement | null = null;

afterEach(async () => {
  if (root !== null) {
    const mounted = root;
    root = null;
    await act(async () => mounted.unmount());
  }
  container?.remove();
  container = null;
});

async function openPicker(): Promise<HTMLElement> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  container = host;
  root = createRoot(host);
  await act(async () => {
    root!.render(
      <DesignAgentPicker
        providers={PROVIDER_IDS.map(provider)}
        providersLoading={false}
        selectedProviderId={null}
        unavailableProviderId={null}
        busy={false}
        agentSession={null}
        agentState={null}
        onProviderSelect={() => undefined}
        onModelSelect={() => undefined}
        onEffortSelect={() => undefined}
      />,
    );
  });
  const trigger = host.querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]');
  if (trigger === null) throw new Error("provider trigger did not render");
  await act(async () => trigger.click());
  const shell = host.querySelector<HTMLElement>("#design-provider-picker");
  if (shell === null) throw new Error("provider picker did not open");
  return shell;
}

function optionsList(shell: HTMLElement): HTMLElement {
  const list = shell.querySelector<HTMLElement>(".design-agent-picker-options");
  if (list === null) throw new Error("options list did not render");
  return list;
}

function optionRows(shell: HTMLElement): HTMLElement[] {
  return [...shell.querySelectorAll<HTMLElement>('[role="option"]')];
}

/** Gives a laid-out box to the two elements the scroll helper measures. */
function stubScrollport(
  list: HTMLElement,
  row: HTMLElement,
  top: number,
  height: number,
): {
  scrolledBy: () => number;
} {
  let scrollTop = 0;
  Object.defineProperty(list, "clientHeight", { value: 100, configurable: true });
  Object.defineProperty(list, "scrollTop", {
    get: () => scrollTop,
    set: (value: number) => {
      scrollTop = value;
    },
    configurable: true,
  });
  Object.defineProperty(row, "offsetTop", { value: top, configurable: true });
  Object.defineProperty(row, "offsetHeight", { value: height, configurable: true });
  return { scrolledBy: () => scrollTop };
}

describe("the provider picker's options list", () => {
  it("holds every option and stays outside the heading", async () => {
    const shell = await openPicker();
    const list = optionsList(shell);
    const label = shell.querySelector<HTMLElement>(".design-agent-picker-label");
    if (label === null) throw new Error("heading did not render");
    expect(label.textContent).toBe("Choose agent");
    expect(list.contains(label)).toBe(false);
    const rows = optionRows(shell);
    expect(rows).toHaveLength(PROVIDER_IDS.length);
    for (const row of rows) expect(list.contains(row)).toBe(true);
  });

  it("scrolls a focused option below the fold into its own view", async () => {
    const shell = await openPicker();
    const list = optionsList(shell);
    const rows = optionRows(shell);
    const last = rows[rows.length - 1];
    if (last === undefined) throw new Error("no option to focus");
    const { scrolledBy } = stubScrollport(list, last, 700, 30);
    await act(async () => last.focus());
    // The row's bottom (730) is past the 100px box: the box scrolls by 630.
    expect(scrolledBy()).toBe(630);
    expect(document.activeElement).toBe(last);
  });

  it("leaves the box alone when the focused option is already in view", async () => {
    const shell = await openPicker();
    const list = optionsList(shell);
    const first = optionRows(shell)[0];
    if (first === undefined) throw new Error("no option to focus");
    const { scrolledBy } = stubScrollport(list, first, 0, 30);
    await act(async () => first.focus());
    expect(scrolledBy()).toBe(0);
    expect(document.activeElement).toBe(first);
  });
});
