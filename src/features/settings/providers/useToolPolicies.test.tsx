// @vitest-environment happy-dom

// The panel's single tool-policy store: one fetch for every row, boolean
// writes with an always-empty deny list, optimistic with revert, newest
// sequence wins, terminal load failure with Retry.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toolPolicyGet, toolPolicySet } from "../../../lib/tauri";
import type { ToolPolicyReply } from "../../../types/ipc";
import { useToolPolicies } from "./useToolPolicies";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return {
    ...actual,
    toolPolicyGet: vi.fn(async () => ({ policies: [] })),
    toolPolicySet: vi.fn(async () => undefined),
  };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function Harness({ supported = true, active = true }: { supported?: boolean; active?: boolean }) {
  const store = useToolPolicies(supported, active);
  return (
    <div>
      <span data-testid="policies">{JSON.stringify(store.policies)}</span>
      {store.loadFailed ? (
        <button type="button" onClick={store.retry}>
          Retry
        </button>
      ) : null}
      {store.loadError ? <span role="alert">{store.loadError.sentence}</span> : null}
      {Object.entries(store.writeErrors).map(([providerId, failure]) => (
        <span role="alert" data-testid={`write-error-${providerId}`} key={providerId}>
          {providerId}:{failure.sentence}
          <button type="button" onClick={() => store.dismissWriteError(providerId)}>
            dismiss-{providerId}
          </button>
        </span>
      ))}
      <button type="button" onClick={() => store.setEnabled("grok", false)}>
        off
      </button>
      <button type="button" onClick={() => store.setEnabled("grok", true)}>
        on
      </button>
      <button type="button" onClick={() => store.turnAllOn("grok")}>
        all-on
      </button>
    </div>
  );
}

describe("useToolPolicies", () => {
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
    vi.clearAllMocks();
    vi.mocked(toolPolicySet).mockReset();
    vi.mocked(toolPolicySet).mockImplementation(async () => undefined);
    vi.mocked(toolPolicyGet).mockReset();
    vi.mocked(toolPolicyGet).mockImplementation(async () => ({ policies: [] }));
  });

  async function renderHarness(props?: { supported?: boolean; active?: boolean }) {
    await act(async () => root.render(<Harness {...props} />));
    await act(async () => undefined);
  }

  function buttonNamed(name: string): HTMLButtonElement {
    const button = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === name,
    );
    if (!button) throw new Error(`button ${name} did not render`);
    return button as HTMLButtonElement;
  }

  it("fetches once on mount and never when unsupported or inactive", async () => {
    await renderHarness();
    expect(toolPolicyGet).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[data-testid="policies"]')?.textContent).toBe("[]");
  });

  it("never fetches when the daemon lacks tool_policy", async () => {
    await renderHarness({ supported: false });
    expect(toolPolicyGet).not.toHaveBeenCalled();
  });

  it("never fetches when no provider serves tools", async () => {
    await renderHarness({ active: false });
    expect(toolPolicyGet).not.toHaveBeenCalled();
  });

  it("writes enabled:false with an empty deny list, optimistically", async () => {
    await renderHarness();
    await act(async () => buttonNamed("off").click());
    expect(toolPolicySet).toHaveBeenCalledTimes(1);
    expect(toolPolicySet).toHaveBeenCalledWith("grok", false, []);
    expect(container.querySelector('[data-testid="policies"]')?.textContent).toContain(
      '"enabled":false',
    );
  });

  it("writes enabled:null with an empty deny list for on and for turn-all-on", async () => {
    await renderHarness();
    await act(async () => buttonNamed("on").click());
    expect(toolPolicySet).toHaveBeenCalledWith("grok", null, []);
    await act(async () => buttonNamed("all-on").click());
    expect(toolPolicySet).toHaveBeenCalledWith("grok", null, []);
    expect(toolPolicySet).toHaveBeenCalledTimes(2);
  });

  it("reverts the optimistic row and names the row on rejection", async () => {
    vi.mocked(toolPolicySet).mockRejectedValueOnce({
      code: "io",
      message: "policy file unwritable",
    });
    await renderHarness();
    await act(async () => buttonNamed("off").click());
    await act(async () => undefined);
    expect(container.querySelector('[data-testid="policies"]')?.textContent).toBe("[]");
    expect(container.querySelector('[data-testid="write-error-grok"]')?.textContent).toContain(
      "grok:",
    );
  });

  it("keeps the second write when two rapid toggles race and the first is rejected", async () => {
    let rejectFirst!: (cause: unknown) => void;
    let resolveSecond!: () => void;
    vi.mocked(toolPolicySet)
      .mockImplementationOnce(
        () =>
          new Promise<void>((_resolve, reject) => {
            rejectFirst = reject;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            resolveSecond = resolve;
          }),
      );
    await renderHarness();
    await act(async () => buttonNamed("off").click());
    await act(async () => buttonNamed("on").click());
    expect(toolPolicySet).toHaveBeenCalledTimes(2);
    expect(toolPolicySet).toHaveBeenNthCalledWith(1, "grok", false, []);
    expect(toolPolicySet).toHaveBeenNthCalledWith(2, "grok", null, []);
    await act(async () => rejectFirst({ code: "io", message: "first write lost" }));
    await act(async () => resolveSecond());
    await act(async () => undefined);
    expect(container.querySelector('[data-testid="policies"]')?.textContent).toContain(
      '"enabled":null',
    );
    expect(container.querySelector('[data-testid="write-error-grok"]')).toBeNull();
  });

  it("ends a failed load in Retry and recovers on success", async () => {
    vi.mocked(toolPolicyGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    await renderHarness();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const retry = buttonNamed("Retry");
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: [] }],
    });
    await act(async () => retry.click());
    await act(async () => undefined);
    expect(toolPolicyGet).toHaveBeenCalledTimes(2);
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.querySelector('[data-testid="policies"]')?.textContent).toContain("grok");
  });

  it("keeps one provider's failure while another provider writes", async () => {
    function TwoProviderHarness() {
      const store = useToolPolicies(true, true);
      return (
        <div>
          <button type="button" onClick={() => store.setEnabled("grok", false)}>
            grok-off
          </button>
          <button type="button" onClick={() => store.setEnabled("claude", false)}>
            claude-off
          </button>
          {Object.entries(store.writeErrors).map(([providerId, failure]) => (
            <span role="alert" data-testid={`write-error-${providerId}`} key={providerId}>
              {providerId}:{failure.sentence}
              <button type="button" onClick={() => store.dismissWriteError(providerId)}>
                dismiss-{providerId}
              </button>
            </span>
          ))}
        </div>
      );
    }
    await act(async () => root.render(<TwoProviderHarness />));
    await act(async () => undefined);
    vi.mocked(toolPolicySet).mockRejectedValueOnce({ code: "io", message: "grok unwritable" });
    await act(async () => buttonNamed("grok-off").click());
    await act(async () => undefined);
    expect(container.querySelector('[data-testid="write-error-grok"]')).not.toBeNull();

    await act(async () => buttonNamed("claude-off").click());
    await act(async () => undefined);
    // grok's unacknowledged report stands; only its own dismiss clears it.
    expect(container.querySelector('[data-testid="write-error-grok"]')).not.toBeNull();
    await act(async () => buttonNamed("dismiss-grok").click());
    expect(container.querySelector('[data-testid="write-error-grok"]')).toBeNull();
  });

  it("merges a refetch around an in-flight write instead of dropping it", async () => {
    // The reconnect self-heal: a fetch that overlaps a write adopts the
    // daemon's rows for idle providers and keeps the optimistic row for
    // the pinned one — the reply is never thrown away wholesale.
    let resolveWrite!: () => void;
    vi.mocked(toolPolicySet).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveWrite = resolve;
        }),
    );
    let resolveFetch!: (reply: ToolPolicyReply) => void;
    vi.mocked(toolPolicyGet).mockImplementationOnce(
      () =>
        new Promise<ToolPolicyReply>((resolve) => {
          resolveFetch = resolve;
        }),
    );
    await renderHarness();
    await act(async () => buttonNamed("off").click());
    await act(async () => undefined);
    expect(toolPolicySet).toHaveBeenCalledTimes(1);

    resolveFetch({ policies: [{ providerId: "claude", enabled: null, disabledTools: [] }] });
    await act(async () => undefined);
    await act(async () => undefined);
    const merged = container.querySelector('[data-testid="policies"]')?.textContent ?? "";
    // claude adopted from the daemon, grok's optimistic row kept.
    expect(merged).toContain("claude");
    expect(merged).toContain('"enabled":false');

    await act(async () => resolveWrite());
    await act(async () => undefined);
    expect(container.querySelector('[data-testid="policies"]')?.textContent).toContain("grok");
  });
});
