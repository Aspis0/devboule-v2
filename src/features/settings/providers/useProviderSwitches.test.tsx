// @vitest-environment happy-dom

import { act, useRef } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { providerSetEnabled } from "../../../lib/tauri";
import type { ProviderInfo } from "../../../types/ipc";
import { useProviderSwitches } from "./useProviderSwitches";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return { ...actual, providerSetEnabled: vi.fn(async () => undefined) };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const grok = { id: "grok", enabled: true } as ProviderInfo;

function Harness({ supported = true }: { supported?: boolean }) {
  const switches = useProviderSwitches(supported);
  const fetchSnapshot = useRef<ReturnType<typeof switches.beginFetch> | null>(null);
  return (
    <div>
      <output data-testid="enabled">{String(switches.isEnabled(grok))}</output>
      <button type="button" onClick={() => (fetchSnapshot.current = switches.beginFetch())}>
        begin-fetch
      </button>
      <button
        type="button"
        onClick={() => {
          if (fetchSnapshot.current) {
            switches.reconcile([{ ...grok, enabled: true }], fetchSnapshot.current);
          }
        }}
      >
        reconcile-stale-on
      </button>
      <button type="button" onClick={() => void switches.setEnabled(grok, true)}>
        local-on
      </button>
      <button
        type="button"
        onClick={() => switches.reconcile([{ ...grok, enabled: false }], switches.beginFetch())}
      >
        remote-off
      </button>
      <button type="button" onClick={() => void switches.setEnabled(grok, false)}>
        off
      </button>
    </div>
  );
}

describe("useProviderSwitches", () => {
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
  });

  function click(name: string) {
    const button = [...container.querySelectorAll("button")].find(
      (node) => node.textContent === name,
    );
    if (!button) throw new Error(`button ${name} did not render`);
    button.click();
  }

  it("adopts daemon state on a later refetch after a settled local write", async () => {
    await act(async () => root.render(<Harness />));
    await act(async () => click("local-on"));
    expect(container.querySelector("[data-testid=enabled]")?.textContent).toBe("true");
    await act(async () => click("remote-off"));
    expect(container.querySelector("[data-testid=enabled]")?.textContent).toBe("false");
  });

  it("does not send a switch frame when the daemon lacks its capability", async () => {
    await act(async () => root.render(<Harness supported={false} />));
    await act(async () => click("off"));
    expect(providerSetEnabled).not.toHaveBeenCalled();
  });

  it("preserves a toggle made after a fetch snapshot even if its write settles first", async () => {
    let finishWrite: (() => void) | undefined;
    vi.mocked(providerSetEnabled).mockImplementationOnce(
      () => new Promise<void>((resolve) => (finishWrite = resolve)),
    );
    await act(async () => root.render(<Harness />));
    await act(async () => click("begin-fetch"));
    await act(async () => click("off"));
    expect(container.querySelector("[data-testid=enabled]")?.textContent).toBe("false");

    await act(async () => {
      finishWrite?.();
      await Promise.resolve();
    });
    await act(async () => click("reconcile-stale-on"));

    expect(container.querySelector("[data-testid=enabled]")?.textContent).toBe("false");
  });
});
