// @vitest-environment happy-dom

// The command and the environment a permission request carries are the
// card's verbosity, not its decision: they open behind one disclosure while
// Allow/Deny, Submit/Dismiss and Clear stay on the card itself.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PermissionCard } from "./PermissionCard";
import type { PermissionRequest } from "../types/ipc";

const respond = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({ sessionPermissionRespond: respond }));

const shellRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "bash-1",
  title: "Shell",
  command: "rm -rf build",
  env: [{ name: "CI", value: "1" }],
  options: [
    { optionId: "deny", name: "Deny", kind: "reject_once" },
    { optionId: "allow", name: "Allow once", kind: "allow_once" },
  ],
};

let container: HTMLDivElement;
let root: Root;

async function render(request: PermissionRequest = shellRequest): Promise<void> {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(
      <PermissionCard
        sessionId="session-1"
        subscriptionId={1}
        request={request}
        capabilities={["typed_permissions"]}
      />,
    );
  });
}

function disclosure(): HTMLDetailsElement | null {
  return container.querySelector<HTMLDetailsElement>(".permission-card-verbose");
}

function action(name: string): HTMLButtonElement | undefined {
  return [...container.querySelectorAll("button")].find((button) => button.textContent === name);
}

describe("the permission card's verbose values", () => {
  afterEach(async () => {
    await act(async () => root?.unmount());
    container?.remove();
    respond.mockReset();
  });

  it("keeps the command and the environment closed behind one summary", async () => {
    await render();

    const details = disclosure();
    expect(details?.open).toBe(false);
    expect(details?.querySelector(".permission-card-command")).not.toBeNull();
    expect(details?.querySelector(".permission-card-env")).not.toBeNull();
  });

  it("opens from the keyboard, because the summary is the control", async () => {
    await render();

    const summary = disclosure()?.querySelector("summary");
    expect(summary?.tagName).toBe("SUMMARY");
    // The summary IS the disclosure in a browser: focusable, and Enter or
    // Space toggles it with no key handler here. happy-dom reports tabIndex
    // -1 for <summary>, so reach is asserted by focus().
    summary?.focus();
    expect(document.activeElement).toBe(summary);
    // The control names itself in words, so a reader can tell what it opens
    // before pressing it.
    expect(summary?.textContent).toBe("Command and environment");
  });

  it("leaves the decision itself outside the disclosure", async () => {
    await render();

    const allow = action("Allow once");
    const deny = action("Deny");
    expect(allow).not.toBeNull();
    expect(deny).not.toBeNull();
    expect(disclosure()?.contains(allow ?? null)).toBe(false);
    expect(disclosure()?.contains(deny ?? null)).toBe(false);

    respond.mockResolvedValue(undefined);
    await act(async () => deny?.click());
    expect(respond).toHaveBeenCalledWith("session-1", 1, "bash-1", "deny", undefined, undefined);
  });

  it("renders no disclosure for a request that carries neither", async () => {
    const { command: _command, env: _env, ...bare } = shellRequest;
    await render(bare as PermissionRequest);

    expect(disclosure()).toBeNull();
    expect(action("Allow once")).not.toBeNull();
  });
});
