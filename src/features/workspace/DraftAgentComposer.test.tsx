// @vitest-environment happy-dom

// The first-message composer of a new workspace: the provider it starts the
// agent with is picked in its controls, and the message goes to the start.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderInfo } from "../../types/ipc";
import { DraftAgentComposer, resolveDraftSelection } from "./DraftAgentComposer";

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

describe("the first-message composer's model, effort and mode", () => {
  const CHOICES = {
    models: [
      {
        modelId: "sonnet",
        name: "Sonnet",
        providerId: "claude",
        efforts: [
          { id: "low", label: "Low" },
          { id: "medium", label: "Medium", default: true },
        ],
      },
      { modelId: "haiku", name: "Haiku", providerId: "claude" },
    ],
    modes: [{ id: "plan", name: "Plan" }],
    defaultModelId: "sonnet",
    defaultModeId: "plan",
  } as never;

  it("shows the catalog's default model, its default effort and its default mode when nothing is picked", async () => {
    await act(async () =>
      root.render(
        <DraftAgentComposer
          providers={[provider("claude")]}
          providerId={null}
          onProviderChange={vi.fn()}
          choices={CHOICES}
          onSend={vi.fn(async () => true)}
          disabledReason={null}
        />,
      ),
    );

    expect(container.querySelector('[data-testid="draft-model-chip"]')?.textContent).toContain(
      "Sonnet",
    );
    expect(container.querySelector('[data-testid="draft-effort-chip"]')?.textContent).toContain(
      "Medium",
    );
    expect(container.querySelector('[data-testid="draft-mode-chip"]')?.textContent).toContain(
      "Plan",
    );
  });

  it("reports an effort pick with the model it belongs to, and keeps the other picks", async () => {
    const onSelectionChange = vi.fn();
    await act(async () =>
      root.render(
        <DraftAgentComposer
          providers={[provider("claude")]}
          providerId={null}
          onProviderChange={vi.fn()}
          choices={CHOICES}
          selection={{ modelId: "sonnet", effort: null, modeId: null }}
          onSelectionChange={onSelectionChange}
          onSend={vi.fn(async () => true)}
          disabledReason={null}
        />,
      ),
    );
    const chip = container.querySelector<HTMLButtonElement>('[data-testid="draft-effort-chip"]');
    if (chip === null) throw new Error("the effort chip did not render");
    await act(async () => chip.click());
    const low = document.querySelector<HTMLElement>('[data-testid="draft-effort-option-low"]');
    if (low === null) throw new Error("the Low option did not open");
    await act(async () => low.click());

    expect(onSelectionChange).toHaveBeenCalledWith({
      modelId: "sonnet",
      effort: "low",
      modeId: null,
    });
  });

  it("offers no model chip while the catalog is still loading, and no effort chip for a model without efforts", async () => {
    await act(async () =>
      root.render(
        <DraftAgentComposer
          providers={[provider("claude")]}
          providerId={null}
          onProviderChange={vi.fn()}
          choices={null}
          onSend={vi.fn(async () => true)}
          disabledReason={null}
        />,
      ),
    );
    expect(container.querySelector('[data-testid="draft-model-chip"]')).toBeNull();
    expect(container.querySelector('[data-testid="draft-effort-chip"]')).toBeNull();
    expect(container.querySelector('[data-testid="draft-mode-chip"]')).toBeNull();
  });
});

describe("resolveDraftSelection", () => {
  it("resolves to the catalog's default model, its default effort and its default mode", () => {
    expect(
      resolveDraftSelection(
        {
          models: [
            { modelId: "haiku", name: "Haiku" },
            {
              modelId: "sonnet",
              name: "Sonnet",
              efforts: [
                { id: "low", label: "Low" },
                { id: "high", label: "High", default: true },
              ],
            },
          ],
          modes: [
            { id: "ask", name: "Ask" },
            { id: "plan", name: "Plan" },
          ],
          defaultModelId: "sonnet",
          defaultModeId: "plan",
        } as never,
        { modelId: null, effort: null, modeId: null },
      ),
    ).toEqual({ modelId: "sonnet", effort: "high", modeId: "plan" });
  });

  it("leaves the model and mode unset when the catalog names no default and nothing was picked", () => {
    const choices = {
      models: [
        { modelId: "a", name: "A" },
        { modelId: "b", name: "B" },
      ],
      modes: [{ id: "m1", name: "M1" }],
      defaultModelId: null,
      defaultModeId: null,
    } as never;

    expect(resolveDraftSelection(choices, { modelId: null, effort: null, modeId: null })).toEqual({
      modelId: null,
      effort: null,
      modeId: null,
    });
  });

  it("keeps a pick that exists and falls back to the default, not the first row, when a pick is gone", () => {
    const choices = {
      models: [
        {
          modelId: "a",
          name: "A",
          efforts: [
            { id: "x", label: "X" },
            { id: "y", label: "Y", default: true },
          ],
        },
        { modelId: "b", name: "B" },
      ],
      modes: [],
      defaultModelId: "b",
    } as never;

    expect(resolveDraftSelection(choices, { modelId: "a", effort: "x", modeId: null })).toEqual({
      modelId: "a",
      effort: "x",
      modeId: null,
    });
    expect(resolveDraftSelection(choices, { modelId: "gone", effort: "y", modeId: null })).toEqual({
      modelId: "b",
      effort: null,
      modeId: null,
    });
  });

  it("leaves an effort unset when the model names no default effort", () => {
    const choices = {
      models: [
        {
          modelId: "a",
          name: "A",
          efforts: [
            { id: "x", label: "X" },
            { id: "y", label: "Y" },
          ],
        },
      ],
      modes: [],
      defaultModelId: "a",
    } as never;

    expect(resolveDraftSelection(choices, { modelId: null, effort: null, modeId: null })).toEqual({
      modelId: "a",
      effort: null,
      modeId: null,
    });
  });
});
