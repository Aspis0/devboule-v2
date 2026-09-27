// @vitest-environment happy-dom

// The lazy model count: no vocabulary call until an expanded row mounts this
// component, one call per provider, and no number for absent/none/empty.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { providerVocabularyGet } from "../../../lib/tauri";
import type { ProviderVocabulary } from "../../../types/ipc";
import { ProviderModelCount, type ModelCountCache } from "./ProviderModelCount";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return { ...actual, providerVocabularyGet: vi.fn() };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function vocabularyWith(
  state: ProviderVocabulary["models"]["state"],
  count: number,
): ProviderVocabulary {
  return {
    provider: "grok",
    models: {
      state,
      items: Array.from({ length: count }, (_, index) => ({
        modelId: `model-${index}`,
        name: `Model ${index}`,
      })),
    },
    modes: { state: "none", items: [] },
    source: "cache",
  };
}

describe("ProviderModelCount", () => {
  let container: HTMLDivElement;
  let root: Root;
  let cache: ModelCountCache;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    cache = new Map();
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function renderCount(supported = true, epoch = 0) {
    await act(async () =>
      root.render(
        <ProviderModelCount providerId="grok" supported={supported} cache={cache} epoch={epoch} />,
      ),
    );
    await act(async () => undefined);
  }

  it("never asks when the daemon did not advertise provider_vocabulary", async () => {
    await renderCount(false);
    expect(providerVocabularyGet).not.toHaveBeenCalled();
    expect(container.textContent).toBe("");
  });

  it("asks once on mount with a cache-friendly read, not a forced probe", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(vocabularyWith("present", 12));
    await renderCount();
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    expect(providerVocabularyGet).toHaveBeenCalledWith("grok", "", false);
    expect(container.textContent).toContain("12 models");
  });

  it("singularises one model", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(vocabularyWith("present", 1));
    await renderCount();
    expect(container.textContent).toContain("1 model");
    expect(container.textContent).not.toContain("1 models");
  });

  it("shows no number for absent, none, or an empty present list", async () => {
    for (const [state, count] of [
      ["absent", 0],
      ["none", 0],
      ["present", 0],
    ] as const) {
      vi.mocked(providerVocabularyGet).mockResolvedValueOnce(vocabularyWith(state, count));
      const other = document.createElement("div");
      document.body.appendChild(other);
      const otherRoot = createRoot(other);
      const otherCache = new Map();
      await act(async () =>
        otherRoot.render(
          <ProviderModelCount providerId="grok" supported cache={otherCache} epoch={0} />,
        ),
      );
      await act(async () => undefined);
      expect(other.textContent ?? "").not.toMatch(/\d+ models?/);
      await act(async () => otherRoot.unmount());
      other.remove();
    }
  });

  it("shows no number when the read fails", async () => {
    vi.mocked(providerVocabularyGet).mockRejectedValueOnce({
      code: "internal",
      message: "probe died",
    });
    await renderCount();
    expect(container.textContent).toBe("");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("serves a cached reply without asking again", async () => {
    cache.set("grok", { epoch: 0, value: 7 });
    await renderCount();
    expect(providerVocabularyGet).not.toHaveBeenCalled();
    expect(container.textContent).toContain("7 models");
  });

  it("serves a cached absence without asking again", async () => {
    cache.set("grok", { epoch: 0, value: null });
    await renderCount();
    expect(providerVocabularyGet).not.toHaveBeenCalled();
    expect(container.textContent).toBe("");
  });

  it("caches the first reply, so a remount asks nothing more", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(vocabularyWith("present", 5));
    await renderCount();
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    root = createRoot(container);
    await renderCount();
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("5 models");
  });

  it("dedupes a remount inside the read window instead of probing twice", async () => {
    // Expand → collapse → expand while the read is open: the second mount
    // subscribes to the in-flight promise, and one call serves both.
    let resolveRead!: (reply: ProviderVocabulary) => void;
    vi.mocked(providerVocabularyGet).mockReturnValueOnce(
      new Promise<ProviderVocabulary>((resolve) => {
        resolveRead = resolve;
      }),
    );
    await renderCount();
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    root = createRoot(container);
    await renderCount();
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);

    resolveRead(vocabularyWith("present", 9));
    await act(async () => undefined);
    await act(async () => undefined);
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("9 models");
  });

  it("ignores entries from a previous epoch", async () => {
    cache.set("grok", { epoch: 0, value: 3 });
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(vocabularyWith("present", 4));
    await renderCount(true, 1);
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("4 models");
  });
});
