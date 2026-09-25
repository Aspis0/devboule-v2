// The "/" menu's keyboard, at the composer itself: the rows the arrows walk,
// what Enter and Tab put into the text, what Escape leaves behind, the line the
// menu shows when the daemon has no command (or no matching one) to show, and
// the send/queue keys that must not move. The surface-level command menu is
// asserted in `AgentChatSurface.test.tsx`.
// @vitest-environment happy-dom
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { WorkspaceComposer } from "./WorkspaceComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const COMMANDS = [
  { name: "build", description: "Build the app" },
  { name: "goal", description: "Set a goal" },
  { name: "workflow", description: "Run a workflow" },
];

let container: HTMLDivElement;
let root: Root;
let onSend: Mock<(text: string) => void>;
let onQueue: Mock<(text: string) => void>;

function composerProps(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): ComponentProps<typeof WorkspaceComposer> {
  return {
    streaming: false,
    turnActive: false,
    disabled: false,
    disabledReason: null,
    availableCommands: COMMANDS,
    onSend,
    ...overrides,
  };
}

async function renderComposer(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(overrides)} />);
  });
}

async function rerenderComposer(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): Promise<void> {
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(overrides)} />);
  });
}

function textarea(): HTMLTextAreaElement {
  const element = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  if (element === null) throw new Error("composer textarea did not render");
  return element;
}

async function type(text: string): Promise<void> {
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("textarea value setter did not exist");
  await act(async () => {
    setValue.call(textarea(), text);
    textarea().dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function press(key: string, modifiers: KeyboardEventInit = {}): Promise<void> {
  await act(async () => {
    textarea().dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...modifiers }),
    );
  });
}

function menu(): HTMLElement | null {
  return container.querySelector('[aria-label="Available commands"]');
}

function menuOrFail(): HTMLElement {
  const open = menu();
  if (open === null) throw new Error("command menu did not render");
  return open;
}

function rows(): HTMLButtonElement[] {
  return [...menuOrFail().querySelectorAll<HTMLButtonElement>('[role="option"]')];
}

/** The row aria-activedescendant names, looked up the way an assistive tech would. */
function activeRow(): HTMLElement | null {
  const id = textarea().getAttribute("aria-activedescendant");
  if (id === null) return null;
  return document.getElementById(id);
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  onSend = vi.fn<(text: string) => void>();
  onQueue = vi.fn<(text: string) => void>();
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("the open menu's keys", () => {
  it("starts on the first match and walks the rows with the arrows, wrapping at both ends", async () => {
    await renderComposer();
    await type("/");
    expect(activeRow()).toBe(rows()[0]);

    await press("ArrowDown");
    expect(activeRow()).toBe(rows()[1]);
    await press("ArrowDown");
    expect(activeRow()).toBe(rows()[2]);
    await press("ArrowDown");
    expect(activeRow()).toBe(rows()[0]);
    await press("ArrowUp");
    expect(activeRow()).toBe(rows()[2]);
  });

  it("inserts the highlighted command on Enter instead of sending", async () => {
    await renderComposer();
    await type("/");
    await press("ArrowDown");
    await press("Enter");

    expect(onSend).not.toHaveBeenCalled();
    expect(textarea().value).toBe("/goal ");
    expect(menu()).toBeNull();
  });

  it("inserts the highlighted command on Tab, too", async () => {
    await renderComposer();
    await type("/");
    await press("Tab");

    expect(onSend).not.toHaveBeenCalled();
    expect(textarea().value).toBe("/build ");
  });

  it("gives a queue chord to the menu as well, so nothing sends while it is open", async () => {
    // Paseo offers the event to the autocomplete before its own queue chord,
    // so an open menu takes Enter however it is modified.
    await renderComposer({ turnActive: true, enterQueues: true, onQueue });
    await type("/");
    await press("Enter", { ctrlKey: true, metaKey: true });

    expect(onSend).not.toHaveBeenCalled();
    expect(onQueue).not.toHaveBeenCalled();
    expect(textarea().value).toBe("/build ");
  });

  it("closes on Escape and keeps the text", async () => {
    await renderComposer();
    await type("/go");
    expect(rows()).toHaveLength(1);

    await press("Escape");

    expect(menu()).toBeNull();
    expect(textarea().value).toBe("/go");
    expect(onSend).not.toHaveBeenCalled();
  });

  it("opens again on the next keystroke after Escape", async () => {
    await renderComposer();
    await type("/w");
    await press("Escape");
    expect(menu()).toBeNull();

    await type("/wo");

    expect(rows()).toHaveLength(1);
    expect(activeRow()).toBe(rows()[0]);
  });

  it("refilters on typing and puts the highlight back on the first match", async () => {
    await renderComposer();
    await type("/");
    await press("ArrowDown");
    expect(activeRow()).toBe(rows()[1]);

    await type("/o");

    expect(rows().map((row) => row.querySelector(".workspace-command-name")?.textContent)).toEqual([
      "/goal",
      "/workflow",
    ]);
    expect(activeRow()).toBe(rows()[0]);
  });

  it("sends on Enter when the open menu has no row to give", async () => {
    // Paseo hands the key back untouched when it has no option, so a menu
    // with nothing to complete never traps Enter.
    await renderComposer();
    await type("/zzz");

    expect(rows()).toHaveLength(0);
    await press("Enter");

    expect(onSend).toHaveBeenCalledWith("/zzz");
    expect(textarea().value).toBe("");
  });

  it("sends on Enter with the menu closed, as it always has", async () => {
    await renderComposer();
    await type("hello");
    expect(menu()).toBeNull();

    await press("Enter");

    expect(onSend).toHaveBeenCalledWith("hello");
    expect(textarea().value).toBe("");
  });
});

describe("the send and queue keys with the menu closed", () => {
  it("queues on Enter while the turn runs when Enter is set to queue", async () => {
    await renderComposer({ turnActive: true, enterQueues: true, onQueue });
    await type("later");

    await press("Enter");

    expect(onQueue).toHaveBeenCalledWith("later");
    expect(onSend).not.toHaveBeenCalled();
  });

  it("queues on the alternate chord when Enter is set to steer", async () => {
    await renderComposer({ turnActive: true, enterQueues: false, onQueue });
    await type("later");

    await press("Enter", { ctrlKey: true, metaKey: true });

    expect(onQueue).toHaveBeenCalledWith("later");
    expect(onSend).not.toHaveBeenCalled();
  });

  it("sends on the alternate chord when Enter is set to queue", async () => {
    await renderComposer({ turnActive: true, enterQueues: true, onQueue });
    await type("later");

    await press("Enter", { ctrlKey: true, metaKey: true });

    expect(onSend).toHaveBeenCalledWith("later");
    expect(onQueue).not.toHaveBeenCalled();
  });
});

describe("the menu with nothing to show", () => {
  it("shows a line instead of an empty box when no command was published", async () => {
    await renderComposer({ availableCommands: [] });
    await type("/");

    expect(menuOrFail().textContent).toContain("No commands found");
    expect(rows()).toHaveLength(0);
    expect(textarea().getAttribute("aria-activedescendant")).toBeNull();
  });

  it("shows the same line when the filter matches nothing", async () => {
    await renderComposer();
    await type("/zzz");

    expect(menuOrFail().textContent).toContain("No commands found");
    expect(rows()).toHaveLength(0);
  });

  it("takes a list that arrives late into the menu that is already open", async () => {
    await renderComposer({ availableCommands: [] });
    await type("/");
    expect(menuOrFail().textContent).toContain("No commands found");

    await rerenderComposer();

    expect(rows()).toHaveLength(COMMANDS.length);
    expect(textarea().value).toBe("/");
    expect(activeRow()).toBe(rows()[0]);
  });

  it("names the highlighted row for the assistive tech on the focused textarea", async () => {
    await renderComposer();
    expect(textarea().getAttribute("aria-activedescendant")).toBeNull();

    await type("/");

    const first = activeRow();
    expect(first?.getAttribute("role")).toBe("option");
    expect(first?.getAttribute("aria-selected")).toBe("true");
    expect(first?.closest('[role="listbox"]')).toBe(menuOrFail());

    await press("ArrowDown");

    expect(activeRow()).toBe(rows()[1]);
    expect(rows()[0].getAttribute("aria-selected")).toBe("false");
    expect(rows()[1].getAttribute("aria-selected")).toBe("true");
  });
});
