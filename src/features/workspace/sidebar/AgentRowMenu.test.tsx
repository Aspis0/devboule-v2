// @vitest-environment happy-dom

import { act, type ReactNode, type RefObject } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConfirmProvider } from "../../../components/ConfirmHost";
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
  onArchive: (sessionId: string) => Promise<string | null>,
  onDismiss = vi.fn(),
) {
  const anchor = document.createElement("button");
  await act(async () =>
    root.render(
      <ConfirmProvider>
        <AgentRowMenu
          sessionId="child-1"
          title="Website redesign"
          anchor={{ current: anchor }}
          onArchive={onArchive}
          onDismiss={onDismiss}
        />
      </ConfirmProvider>,
    ),
  );
  return onDismiss;
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

    expect(onArchive).toHaveBeenCalledWith("child-1");
  });

  it("closes once the archive went through", async () => {
    const onDismiss = await render(async () => null);
    await act(async () => menuItem()?.click());
    await act(async () => confirmButton()?.click());
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("keeps the menu open and says why when the daemon refuses", async () => {
    const onDismiss = await render(async () => "It is still running, so nothing was archived.");
    await act(async () => menuItem()?.click());
    await act(async () => confirmButton()?.click());

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
});
