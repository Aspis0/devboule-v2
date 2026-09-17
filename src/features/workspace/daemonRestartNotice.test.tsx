// One honest line for a daemon restart, from the `instanceId` the app
// already polls. No second detector, no scary banner.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { DaemonRestartNotice } from "./daemonRestartNotice";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

async function renderNotice(instanceId: string | null, hasRecovered: boolean) {
  root = createRoot(container);
  await act(async () => {
    root.render(<DaemonRestartNotice instanceId={instanceId} hasRecovered={hasRecovered} />);
  });
}

async function rerenderNotice(instanceId: string | null, hasRecovered: boolean) {
  await act(async () => {
    root.render(<DaemonRestartNotice instanceId={instanceId} hasRecovered={hasRecovered} />);
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("DaemonRestartNotice", () => {
  it("stays silent on first connect, even with recovered rows present", async () => {
    await renderNotice("daemon-a", true);

    expect(container.querySelector('[data-testid="daemon-restart-notice"]')).toBeNull();
  });

  it("says what happened when the instance id changes under recovered rows", async () => {
    await renderNotice("daemon-a", true);
    await rerenderNotice("daemon-b", true);

    const notice = container.querySelector('[data-testid="daemon-restart-notice"]');
    expect(notice).not.toBeNull();
    expect(notice?.textContent).toContain("daemon restarted");
    expect(notice?.textContent).toContain("read-only until reopened");
  });

  it("stays silent when nothing recovered needs explaining", async () => {
    await renderNotice("daemon-a", false);
    await rerenderNotice("daemon-b", false);

    expect(container.querySelector('[data-testid="daemon-restart-notice"]')).toBeNull();
  });

  it("does not treat a disconnect (null) as a restart", async () => {
    await renderNotice("daemon-a", true);
    await rerenderNotice(null, true);
    await rerenderNotice("daemon-a", true);

    expect(container.querySelector('[data-testid="daemon-restart-notice"]')).toBeNull();
  });
});
