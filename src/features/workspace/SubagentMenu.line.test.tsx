// @vitest-environment happy-dom

import { act, type ReactNode, type RefObject } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionTask } from "../../types/ipc";
import { markSessionOpened, resetOpenedSessionsForTests } from "./openedSessions";
import { SubagentMenu, type SubagentMenuProps } from "./SubagentMenu";
import type { SubagentRow } from "./subagentRows";

vi.mock("./popoverPlace", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./popoverPlace")>();
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

const child = (id: string, title: string, status: SubagentRow["status"]): SubagentRow => ({
  kind: "child",
  id,
  title,
  status,
  generation: 1,
});

const commandTask = (id: string): SessionTask => ({
  id,
  kind: "command",
  title: "npm test",
  state: "running",
  sessionId: "parent",
  startedAtMs: Date.now() - 9_000,
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
  resetOpenedSessionsForTests();
});

async function render(props: Partial<SubagentMenuProps>) {
  const full: SubagentMenuProps = { rows: [], tasks: [], ...props };
  await act(async () => root.render(<SubagentMenu {...full} />));
}

const line = () => container.querySelector<HTMLButtonElement>('[data-testid="who-is-working"]');

describe("the line's focus", () => {
  it("hands focus to the composer when the last working child settles while the list is open", async () => {
    const onEmptied = vi.fn();
    await render({ rows: [child("a", "Kalsa", "running")], onEmptied });
    await act(async () => line()?.click());
    const row = container.querySelector<HTMLButtonElement>(".workspace-subagent-row");
    await act(async () => row?.focus());

    await render({ rows: [child("a", "Kalsa", "finished")], onEmptied });

    expect(onEmptied).toHaveBeenCalledTimes(1);
  });

  it("leaves focus alone when the line empties while focus sits elsewhere", async () => {
    const onEmptied = vi.fn();
    await render({ rows: [child("a", "Kalsa", "running")], onEmptied });
    const other = document.createElement("textarea");
    document.body.append(other);
    await act(async () => other.focus());

    await render({ rows: [child("a", "Kalsa", "finished")], onEmptied });

    expect(onEmptied).not.toHaveBeenCalled();
    other.remove();
  });
});

describe("the line's commands", () => {
  it("opens the Tasks tab when the line names commands only", async () => {
    const onOpenTasks = vi.fn();
    await render({ tasks: [commandTask("c1")], onOpenTasks });

    await act(async () => line()?.click());

    expect(onOpenTasks).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".workspace-subagent-list")).toBeNull();
  });
});

describe("the failed child on the line", () => {
  it("clears once its tab opened, and stays cleared when the line is drawn again", async () => {
    await render({ rows: [child("web", "Website redesign", "failed")] });
    expect(line()?.getAttribute("aria-label")).toBe("Website redesign failed");

    await act(async () => markSessionOpened("web"));

    expect(line()).toBeNull();
  });
});

describe("the line's grammar", () => {
  it("uses the plural verb for several children waiting on the person", async () => {
    await render({
      rows: [child("w1", "Kalsa", "running"), child("w2", "Website", "running")],
      attentionById: new Map([
        ["w1", "Needs your approval"],
        ["w2", "Needs your approval"],
      ]),
    });
    expect(line()?.getAttribute("aria-label")).toBe("2 agents need you");
  });
});
