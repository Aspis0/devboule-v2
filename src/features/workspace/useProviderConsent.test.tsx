// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderInfo } from "../../types/ipc";
import { useProviderConsent } from "./useProviderConsent";

const provider: ProviderInfo = {
  id: "codex-acp",
  executable: "@agentclientprotocol/codex-acp@1.10.0",
  acpAvailable: true,
  authentication: "unknown",
  protocol: "acp",
  origin: "npx-wrapper",
  launchArgs: ["--name=Alice Bob", '--output="x"'],
};

function ConsentHarness({ onConfirmed }: { onConfirmed: (value: ProviderInfo) => void }) {
  const consent = useProviderConsent({ onConfirmed });
  return (
    <>
      <button type="button" data-testid="request" onClick={() => consent.request(provider)} />
      <button type="button" data-testid="confirm" onClick={consent.confirm} />
      <output data-testid="command">{consent.commandLine}</output>
      <output data-testid="pending">{consent.pending?.id ?? "none"}</output>
    </>
  );
}

describe("useProviderConsent", () => {
  let container: HTMLDivElement;
  let root: Root;

  afterEach(() => {
    root.unmount();
    container.remove();
  });

  it("formats the npx consent command with unambiguous argument quoting", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => root.render(<ConsentHarness onConfirmed={vi.fn()} />));

    await act(async () =>
      container.querySelector<HTMLButtonElement>("[data-testid=request]")?.click(),
    );

    expect(container.querySelector("[data-testid=command]")?.textContent).toBe(
      'npx -y @agentclientprotocol/codex-acp@1.10.0 "--name=Alice Bob" "--output=\\"x\\""',
    );
  });

  it("ignores a second synchronous confirm while the pending provider is stale", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    const onConfirmed = vi.fn();
    root = createRoot(container);
    await act(async () => root.render(<ConsentHarness onConfirmed={onConfirmed} />));
    await act(async () =>
      container.querySelector<HTMLButtonElement>("[data-testid=request]")?.click(),
    );

    await act(async () => {
      const confirm = container.querySelector<HTMLButtonElement>("[data-testid=confirm]");
      confirm?.click();
      confirm?.click();
    });

    expect(onConfirmed).toHaveBeenCalledTimes(1);
    expect(onConfirmed).toHaveBeenCalledWith(provider);
  });

  it("resets inFlight after a confirmed provider is handed off", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    const onConfirmed = vi.fn();
    function StateHarness() {
      const consent = useProviderConsent({ onConfirmed });
      return (
        <>
          <button type="button" data-testid="request" onClick={() => consent.request(provider)} />
          <button type="button" data-testid="confirm" onClick={consent.confirm} />
          <output data-testid="in-flight">{String(consent.inFlight)}</output>
        </>
      );
    }
    root = createRoot(container);
    await act(async () => root.render(<StateHarness />));
    await act(async () =>
      container.querySelector<HTMLButtonElement>("[data-testid=request]")?.click(),
    );
    await act(async () =>
      container.querySelector<HTMLButtonElement>("[data-testid=confirm]")?.click(),
    );

    expect(container.querySelector("[data-testid=in-flight]")?.textContent).toBe("false");
  });
});
