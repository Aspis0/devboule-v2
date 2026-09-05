// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  oracleAsk: vi.fn(),
  oracleStatus: vi.fn(),
  pluginsList: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  oracleAsk: mocks.oracleAsk,
  oracleStatus: mocks.oracleStatus,
  pluginsList: mocks.pluginsList,
}));

import { App } from "../../app/App";
import { useAppStore } from "../../store/appStore";
import { DesignSurface, type DesignHost } from "./DesignSurface";
import { createOracleHost } from "./oracleHost";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const RESULT = {
  path: "src/features/oracle/OraclePanel.tsx",
  line_start: 42,
  line_end: 58,
  snippet: "export function OraclePanel() {",
  score: 0.98,
};

function createRootContainer(): { container: HTMLDivElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  return { container, root: createRoot(container) };
}

async function renderDesign(host: DesignHost): Promise<{
  container: HTMLDivElement;
  root: Root;
}> {
  const { container, root } = createRootContainer();
  await act(async () => root.render(<DesignSurface host={host} />));
  return { container, root };
}

async function fillDraft(container: HTMLDivElement, prompt: string): Promise<void> {
  const draft = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Describe a design change"]',
  );
  if (draft === null) throw new Error("Design composer missing");
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("textarea value setter did not exist");

  await act(async () => {
    setValue.call(draft, prompt);
    draft.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  useAppStore.setState({
    activeSurface: "design",
    plugins: null,
    installing: null,
    installError: null,
  });
  mocks.oracleAsk.mockReset();
  mocks.oracleStatus.mockReset();
  mocks.pluginsList.mockResolvedValue({ root: "", plugins: [], problem: null });
});

afterEach(async () => {
  document.body.replaceChildren();
});

describe("Oracle design host", () => {
  it("loads the document skeleton with an empty transcript", async () => {
    const document = await createOracleHost().loadDocument();

    expect(document.messages).toEqual([]);
  });

  it("maps Oracle result paths into generation sources", async () => {
    mocks.oracleAsk.mockResolvedValue({
      query: "where is the Oracle panel?",
      results: [RESULT, { ...RESULT, path: "src/lib/tauri.ts" }],
    });
    const host = createOracleHost();

    const generation = await host.generate?.(
      "where is the Oracle panel?",
      new AbortController().signal,
    );

    expect(generation?.prompt).toBe("where is the Oracle panel?");
    expect(mocks.oracleAsk).toHaveBeenCalledWith("where is the Oracle panel?");
    expect(generation?.sources).toEqual([
      "src/features/oracle/OraclePanel.tsx",
      "src/lib/tauri.ts",
    ]);
    expect(generation?.desc).toContain("2");
    expect(generation?.desc).toContain("repository index");
  });

  it("describes an empty Oracle result without inventing a match", async () => {
    mocks.oracleAsk.mockResolvedValue({ query: "not present", results: [] });
    const host = createOracleHost();

    const generation = await host.generate?.("not present", new AbortController().signal);

    expect(generation?.sources).toEqual([]);
    expect(generation?.desc.toLowerCase()).toContain("nothing found");
    expect(generation?.desc).not.toContain("1 hit");
  });

  it("surfaces an Oracle query failure in the assistant message", async () => {
    mocks.oracleAsk.mockRejectedValue(new Error("Oracle daemon unavailable"));
    const { container, root } = await renderDesign(createOracleHost());
    await fillDraft(container, "Find the workspace resolver.");

    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(container.textContent).toContain("Generation failed");
    expect(container.textContent).toContain("Oracle daemon unavailable");
    expect(container.textContent).not.toContain("Oracle found");
    await act(async () => root.unmount());
  });

  it("rejects before asking when the signal is already aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    const host = createOracleHost();

    await expect(host.generate?.("ignored", controller.signal)).rejects.toMatchObject({
      name: "AbortError",
    });
    expect(mocks.oracleAsk).not.toHaveBeenCalled();
  });

  it("rejects after Oracle resolves when the signal was aborted", async () => {
    let resolveAsk: ((value: { query: string; results: (typeof RESULT)[] }) => void) | undefined;
    mocks.oracleAsk.mockReturnValue(
      new Promise((resolve) => {
        resolveAsk = resolve;
      }),
    );
    const controller = new AbortController();
    const host = createOracleHost();
    const generation = host.generate?.("wait for Oracle", controller.signal);
    controller.abort();
    resolveAsk?.({ query: "wait for Oracle", results: [RESULT] });

    await expect(generation).rejects.toMatchObject({ name: "AbortError" });
  });

  it("does not expose a save capability or save affordance", async () => {
    const host = createOracleHost();
    expect(host.saveDocument).toBeUndefined();
    const { container, root } = await renderDesign(host);

    expect(container.querySelector(".design-save-primary")).toBeNull();
    expect(container.querySelector(".design-save-menu")).toBeNull();
    await act(async () => root.unmount());
  });

  it("discloses when App falls back to the demo host", async () => {
    mocks.oracleStatus.mockRejectedValue(new Error("Oracle daemon unavailable"));
    const { container, root } = createRootContainer();

    await act(async () => root.render(<App />));
    await act(async () => undefined);
    await act(async () => undefined);

    expect(container.textContent).toContain("Demo design — fixtures, not a live store.");
    await act(async () => root.unmount());
  });
});
