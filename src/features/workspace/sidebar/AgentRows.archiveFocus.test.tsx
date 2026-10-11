// @vitest-environment happy-dom

import { act, type ReactNode, type RefObject } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConfirmProvider } from "../../../components/ConfirmHost";
import { AgentRows, type AgentRowsProps } from "./AgentRows";
import type { RailAgent } from "./railAgents";

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

const child = (id: string): RailAgent => ({
  id,
  kind: "claude",
  provider: "Claude",
  title: id,
  state: "finished",
  child: true,
  parentId: "parent",
  parentTitle: "cia",
  hasChildren: false,
  generation: 1,
  archivable: true,
});

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

function props(
  rows: RailAgent[],
  onArchiveAgent: AgentRowsProps["onArchiveAgent"],
): AgentRowsProps {
  return {
    agents: { rows, hidden: 0 },
    activeId: null,
    onOpenAgent: vi.fn(),
    onOpenHistory: vi.fn(),
    onArchiveAgent,
  };
}

async function renderRows(rows: RailAgent[], onArchiveAgent: AgentRowsProps["onArchiveAgent"]) {
  await act(async () =>
    root.render(
      <ConfirmProvider>
        <AgentRows {...props(rows, onArchiveAgent)} />
      </ConfirmProvider>,
    ),
  );
}

const items = () => [...container.querySelectorAll<HTMLElement>('[role="treeitem"]')];

/** Archives the row at `index` through its menu; the archive stays in flight until `finish` is called. */
async function archiveRowAt(index: number, rows: RailAgent[]) {
  const finish: { resolve: (sentence: string | null) => void } = { resolve: () => undefined };
  const onArchiveAgent = vi.fn(
    () =>
      new Promise<string | null>((resolve) => {
        finish.resolve = resolve;
      }),
  );
  await renderRows(rows, onArchiveAgent);
  const row = items()[index];
  if (row === undefined) throw new Error("the row did not render");
  row.focus();
  await act(async () => {
    row.dispatchEvent(new KeyboardEvent("keydown", { key: "F10", shiftKey: true, bubbles: true }));
  });
  await act(async () => container.querySelector<HTMLButtonElement>('[role="menuitem"]')?.click());
  await act(async () =>
    document.querySelector<HTMLButtonElement>(".confirm-dialog-confirm")?.click(),
  );
  return { finish, onArchiveAgent };
}

describe("focus after an archive from the row's menu", () => {
  it("moves focus to the next row when the archived row has gone", async () => {
    const rows = [child("a"), child("b"), child("c")];
    const { finish } = await archiveRowAt(1, rows);

    await renderRows([child("a"), child("c")], vi.fn());
    await act(async () => finish.resolve(null));

    expect(document.activeElement).toBe(items()[1]);
    expect(items()[1]?.getAttribute("aria-label")).toContain("c");
  });

  it("moves focus to the previous row when the last row was archived", async () => {
    const rows = [child("a"), child("b"), child("c")];
    const { finish } = await archiveRowAt(2, rows);

    await renderRows([child("a"), child("b")], vi.fn());
    await act(async () => finish.resolve(null));

    expect(document.activeElement).toBe(items()[1]);
    expect(items()[1]?.getAttribute("aria-label")).toContain("b");
  });
});
