// @vitest-environment happy-dom

import { act, type ReactNode, type RefObject } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConfirmProvider } from "../../../components/ConfirmHost";
import type { Session } from "../../../types/ipc";
import { archiveTargets, type ArchiveTarget } from "../subagentArchive";
import { AgentRowMenu } from "./AgentRowMenu";

vi.mock("../popoverPlace", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../popoverPlace")>();
  return {
    ...actual,
    AnchoredPopover: (props: {
      containerRef?: RefObject<HTMLDivElement | null>;
      className?: string;
      children?: ReactNode;
    }) => (
      <div
        ref={(node: HTMLDivElement | null) => {
          if (props.containerRef) props.containerRef.current = node;
        }}
        className={props.className}
      >
        {props.children}
      </div>
    ),
  };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

async function render(
  onArchive: (target: ArchiveTarget) => Promise<string | null>,
  target: ArchiveTarget = { id: "child-1", generation: 2 },
) {
  const callbacks = { onArchived: vi.fn(), onDismiss: vi.fn() };
  const anchor = document.createElement("button");
  await act(async () =>
    root.render(
      <ConfirmProvider>
        <AgentRowMenu
          target={target}
          title="Website redesign"
          anchor={{ current: anchor }}
          onArchive={onArchive}
          onArchived={callbacks.onArchived}
          onDismiss={callbacks.onDismiss}
        />
      </ConfirmProvider>,
    ),
  );
  return callbacks;
}

const menuItem = () => container.querySelector<HTMLButtonElement>('[role="menuitem"]');
const confirmButton = () => document.querySelector<HTMLButtonElement>(".confirm-dialog-confirm");

describe("the agent row's menu", () => {
  it("asks first, and archives only on the answer", async () => {
    const onArchive = vi.fn(async () => null);
    await render(onArchive);

    await act(async () => menuItem()?.click());
    expect(onArchive).not.toHaveBeenCalled();
    await act(async () => confirmButton()?.click());

    expect(onArchive).toHaveBeenCalledWith({ id: "child-1", generation: 2 });
  });

  it("reports the archive as done, and not as dismissed", async () => {
    const { onArchived, onDismiss } = await render(async () => null);
    await act(async () => menuItem()?.click());
    await act(async () => confirmButton()?.click());
    expect(onArchived).toHaveBeenCalledTimes(1);
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("keeps the menu open and says why when the daemon refuses", async () => {
    const { onArchived, onDismiss } = await render(
      async () => "It is still running, so nothing was archived.",
    );
    await act(async () => menuItem()?.click());
    await act(async () => confirmButton()?.click());

    expect(onArchived).not.toHaveBeenCalled();
    expect(onDismiss).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "It is still running, so nothing was archived.",
    );
  });

  it("archives nothing when the person keeps it", async () => {
    const onArchive = vi.fn(async () => null);
    await render(onArchive);
    await act(async () => menuItem()?.click());
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".confirm-dialog-cancel")?.click(),
    );
    expect(onArchive).not.toHaveBeenCalled();
  });

  it("closes nothing when the roster moved on while the confirmation was open", async () => {
    const endedChild = (generation: number): Session => ({
      id: "child-1",
      workspaceId: "workspace-1",
      kind: "acp",
      title: "Website redesign",
      createdBy: "parent",
      state: { type: "ended", generation, code: 0, integrity: { kind: "complete" } },
      elapsedMs: null,
    });
    let roster: Session[] = [endedChild(1)];
    const close = vi.fn(async () => undefined);
    const { onArchived } = await render(
      (target) =>
        archiveTargets([target], () => roster, close).then((s) => s.get(target.id) ?? null),
      { id: "child-1", generation: 1 },
    );
    await act(async () => menuItem()?.click());
    // A new run of the same child starts while the person reads the prompt.
    roster = [endedChild(2)];
    await act(async () => confirmButton()?.click());

    expect(close).not.toHaveBeenCalled();
    expect(onArchived).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "It changed since you asked, so it was left open.",
    );
  });
});
