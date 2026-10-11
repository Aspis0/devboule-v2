// @vitest-environment happy-dom

// The first-message composer of a new workspace: the provider it starts the
// agent with is picked in its controls, and the message goes to the start.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderInfo } from "../../types/ipc";
import { DraftAgentComposer } from "./DraftAgentComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const provider = (id: string): ProviderInfo =>
  ({
    id,
    executable: id,
    acpAvailable: false,
    authentication: "ok",
    protocol: "acp",
  }) as ProviderInfo;

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
});

async function render(
  props: Partial<Parameters<typeof DraftAgentComposer>[0]> = {},
): Promise<{ onSend: ReturnType<typeof vi.fn>; onProviderChange: ReturnType<typeof vi.fn> }> {
  const onSend = vi.fn(async () => true);
  const onProviderChange = vi.fn();
  await act(async () =>
    root.render(
      <DraftAgentComposer
        providers={[provider("claude"), provider("codex")]}
        providerId={null}
        onProviderChange={onProviderChange}
        onSend={onSend}
        disabledReason={null}
        {...props}
      />,
    ),
  );
  return { onSend, onProviderChange };
}

function chip(): HTMLButtonElement {
  const found = container.querySelector<HTMLButtonElement>('[data-testid="draft-provider-chip"]');
  if (found === null) throw new Error("the provider chip did not render");
  return found;
}

describe("the first-message composer", () => {
  it("starts on the first provider and names it in the chip", async () => {
    await render();

    expect(chip().textContent).toContain("Claude");
  });

  it("starts on the picked provider when one is given", async () => {
    await render({ providerId: "codex" });

    expect(chip().textContent).toContain("Codex");
  });

  it("answers a pick with that provider's id, so the next message starts it", async () => {
    const { onProviderChange } = await render();

    await act(async () => chip().click());
    const option = container.ownerDocument.querySelector<HTMLElement>(
      '[data-testid="draft-provider-option-codex"]',
    );
    if (option === null) throw new Error("the Codex option did not open");
    await act(async () => option.click());

    expect(onProviderChange).toHaveBeenCalledWith("codex");
  });

  it("offers no provider chip when no agent can start", async () => {
    await render({ providers: [], disabledReason: "No agent is installed to start." });

    expect(container.querySelector('[data-testid="draft-provider-chip"]')).toBeNull();
    expect(container.textContent).toContain("No agent is installed to start.");
  });
});
