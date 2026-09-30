// @vitest-environment happy-dom
// The command row as the reducer builds it from a measured Codex
// commandExecution pair (the wire in codex_view.rs's
// command_execution_rows_carry_the_command_line_and_the_exit_code test):
// chip, exit dot, exit sentence, and the rows that must stay untouched.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../types/ipc";

const channelHarness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
  activeSubscriptionId: null as number | null,
  nextSubscriptionId: 41,
  handlers: new WeakMap<object, (event: SessionEvent) => void>(),
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: vi.fn(async () => []),
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    const channel = {};
    channelHarness.handlers.set(channel, onEvent);
    channelHarness.emit = onEvent;
    return channel;
  }),
  sessionAttach: vi.fn(async (...args: unknown[]) => {
    await Promise.resolve();
    const channel = args[2];
    const subscriptionId = channelHarness.nextSubscriptionId++;
    channelHarness.activeSubscriptionId = subscriptionId;
    channelHarness.active =
      typeof channel === "object" && channel !== null
        ? (channelHarness.handlers.get(channel) ?? null)
        : null;
    return subscriptionId;
  }),
  sessionDetach: vi.fn(async (subscriptionId: number) => {
    if (channelHarness.activeSubscriptionId !== subscriptionId) return;
    channelHarness.activeSubscriptionId = null;
    channelHarness.active = null;
  }),
  sessionSend: vi.fn(async () => undefined),
  sessionInterrupt: vi.fn(async () => undefined),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  isCommandError: (error: unknown): boolean =>
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof (error as { code: unknown }).code === "string" &&
    typeof (error as { message: unknown }).message === "string",
}));

import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// The measured line: the pwsh wrapper included, not the payload it wraps.
const PWSH_LINE =
  "\"C:\\Users\\gualt\\AppData\\Local\\Microsoft\\WindowsApps\\pwsh.exe\" -Command 'git status'";

const CODEX_CALL: SessionEvent = {
  type: "agent_tool_call",
  toolCallId: "exec-1",
  title: "git status",
  status: "in_progress",
  kind: "execute",
  command: PWSH_LINE,
};

describe("command tool row", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    channelHarness.emit = null;
    channelHarness.activeSubscriptionId = null;
    channelHarness.nextSubscriptionId = 41;
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    channelHarness.activeSubscriptionId = null;
    channelHarness.active = null;
    vi.clearAllMocks();
  });

  async function mount() {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface daemonState="connected" sessionId="codex-agent" title="Agent" />,
      );
    });
    await act(async () => undefined);
  }

  function onlyRow(): HTMLDetailsElement {
    const row = container.querySelector<HTMLDetailsElement>("details.workspace-chat-tool");
    if (row === null) throw new Error("tool row did not render");
    return row;
  }

  /** Name-from-content, approximated: text nodes joined with single spaces,
   * aria-hidden subtrees dropped. Pseudo-element content is not in the DOM. */
  function accessibleName(root: Element): string {
    const parts: string[] = [];
    const walk = (element: Element) => {
      if (element.getAttribute("aria-hidden") === "true") return;
      for (const child of element.childNodes) {
        if (child.nodeType === 3) {
          const text = child.textContent?.replace(/\s+/g, " ").trim();
          if (text) parts.push(text);
        } else if (child.nodeType === 1) {
          walk(child as Element);
        }
      }
    };
    walk(root);
    return parts.join(" ");
  }

  it("renders the failed Codex pair as a chip and one exit-1 mark", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.(CODEX_CALL);
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "exec-1",
        status: "failed",
        text: "fatal: not a git repository (or any of the parent directories): .git\n",
        kind: "execute",
        command: PWSH_LINE,
        exitCode: 1,
      });
    });

    const row = onlyRow();
    expect(row.tagName).toBe("DETAILS");
    expect(row.open).toBe(false);
    expect(row.classList.contains("is-failed")).toBe(true);
    // A command row follows the spec's composition: chip, no label, no icon.
    expect(row.querySelector(".workspace-chat-tool-label")).toBeNull();
    expect(row.querySelector("summary > svg")).toBeNull();
    // The text block comes from the real row output, not a test fixture.
    const textBlock = row.querySelector("summary > .workspace-chat-tool-text");
    if (textBlock === null) throw new Error("the row's text block did not render");
    const chip = textBlock.querySelector(".workspace-command-chip");
    if (chip === null) throw new Error("command chip did not render");
    // The chip carries the payload the summary shows — never the wire line.
    expect(chip.textContent).toBe("git status");
    expect(chip.getAttribute("title")).toBe("git status");
    expect(container.innerHTML).not.toContain("pwsh.exe");
    expect(textBlock.querySelector(".workspace-chat-tool-summary-text")).toBeNull();
    const dot = row.querySelector(".workspace-command-dot");
    if (dot === null) throw new Error("exit dot did not render");
    // The dot is decoration: the visible sentence carries the code.
    expect(dot.getAttribute("aria-hidden")).toBe("true");
    expect(dot.getAttribute("aria-label")).toBeNull();
    expect(dot.classList.contains("is-failed")).toBe(true);
    expect(row.querySelector(".workspace-command-exit")?.textContent).toBe("exit 1");
    const summary = row.querySelector("summary");
    if (summary === null) throw new Error("summary did not render");
    expect(accessibleName(summary)).toBe("Command git status exit 1");
    // One failure mark: the non-zero code supersedes the ×.
    expect(row.querySelector(".workspace-chat-tool-failed")).toBeNull();
    expect(row.querySelector(".workspace-chat-tool-body")?.textContent).toContain(
      "fatal: not a git repository",
    );
  });

  it("renders exit 0 with the live dot and no failure state", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.(CODEX_CALL);
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "exec-1",
        status: "completed",
        text: "On branch main\n",
        kind: "execute",
        command: PWSH_LINE,
        exitCode: 0,
      });
    });

    const row = onlyRow();
    expect(row.classList.contains("is-failed")).toBe(false);
    expect(row.classList.contains("is-running")).toBe(false);
    const dot = row.querySelector(".workspace-command-dot");
    if (dot === null) throw new Error("exit dot did not render");
    expect(dot.getAttribute("aria-hidden")).toBe("true");
    expect(dot.classList.contains("is-failed")).toBe(false);
    expect(row.querySelector(".workspace-command-exit")?.textContent).toBe("exit 0");
    expect(row.querySelector(".workspace-chat-tool-running")).toBeNull();
    expect(row.querySelector(".workspace-chat-tool-failed")).toBeNull();
  });

  it("shows the chip but no exit marker while the command still runs", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.(CODEX_CALL);
    });

    const row = onlyRow();
    expect(row.classList.contains("is-running")).toBe(true);
    expect(row.querySelector(".workspace-command-chip")?.textContent).toBe("git status");
    expect(row.querySelector(".workspace-command-dot")).toBeNull();
    expect(row.querySelector(".workspace-command-exit")).toBeNull();
    expect(row.querySelector('.workspace-chat-tool-running[aria-label="Running"]')).not.toBeNull();
  });

  it("keeps the failed mark when a command row fails without an exit code", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.(CODEX_CALL);
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "exec-1",
        status: "failed",
        text: "the shell never reported a code",
        kind: "execute",
        command: PWSH_LINE,
      });
    });

    const row = onlyRow();
    expect(row.classList.contains("is-failed")).toBe(true);
    expect(row.querySelector('.workspace-chat-tool-failed[aria-label="Failed"]')).not.toBeNull();
    expect(row.querySelector(".workspace-command-dot")).toBeNull();
    expect(row.querySelector(".workspace-command-exit")).toBeNull();
    expect(row.querySelector(".workspace-command-chip")?.textContent).toBe("git status");
  });

  it("keeps the failed mark when a failed row reports exit 0", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.(CODEX_CALL);
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "exec-1",
        status: "failed",
        text: "the tool failed after the process exited clean",
        kind: "execute",
        command: PWSH_LINE,
        exitCode: 0,
      });
    });

    const row = onlyRow();
    // The status says failure and the zero code does not carry it: the × stays.
    expect(row.classList.contains("is-failed")).toBe(true);
    expect(row.querySelector('.workspace-chat-tool-failed[aria-label="Failed"]')).not.toBeNull();
    const dot = row.querySelector(".workspace-command-dot");
    if (dot === null) throw new Error("exit dot did not render");
    expect(dot.classList.contains("is-failed")).toBe(false);
    expect(row.querySelector(".workspace-command-exit")?.textContent).toBe("exit 0");
  });

  it("keeps a Claude row without command and exit fields exactly as it renders today", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "toolu_bash",
        title: "ls -la && echo done",
        status: "pending",
        kind: "execute",
      });
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "toolu_bash",
        status: "completed",
        text: "total 0\n",
      });
    });

    const row = onlyRow();
    expect(row.classList.contains("is-running")).toBe(false);
    expect(row.querySelector(".workspace-command-chip")).toBeNull();
    expect(row.querySelector(".workspace-command-dot")).toBeNull();
    expect(row.querySelector(".workspace-command-exit")).toBeNull();
    // Generic rows keep the icon and the label, inside the real text block.
    expect(row.querySelector("summary > svg")).not.toBeNull();
    const textBlock = row.querySelector("summary > .workspace-chat-tool-text");
    if (textBlock === null) throw new Error("the row's text block did not render");
    // The summary claims the strip: the floor's class comes from real output.
    expect(textBlock.classList.contains("has-summary")).toBe(true);
    expect(textBlock.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Shell");
    expect(textBlock.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "ls -la && echo done",
    );
    expect(row.querySelector(".workspace-chat-tool-body")?.textContent).toContain("total 0");
  });

  it("leaves a label-only row without the summary floor's class", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "toolu_bare",
        title: "web_search",
        status: "pending",
      });
    });

    const row = onlyRow();
    const textBlock = row.querySelector("summary > .workspace-chat-tool-text");
    if (textBlock === null) throw new Error("the row's text block did not render");
    expect(textBlock.classList.contains("has-summary")).toBe(false);
    expect(textBlock.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Web search");
    expect(textBlock.querySelector(".workspace-chat-tool-summary-text")).toBeNull();
  });
});
