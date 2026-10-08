// @vitest-environment happy-dom

// The finish report of a child, collapsed in the parent's chat: its header,
// the four-line summary and its expander, and the notice kinds that have no
// expander at all.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../lib/agentSession";
import { parseAgentDaemonNotice } from "../../lib/agentDaemonNotice";
import { DaemonNoticeCard } from "./DaemonNoticeCard";

type DaemonNoticeItem = Extract<AgentChatItem, { role: "daemon_notice" }>;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

interface FinishOptions {
  /** `null` leaves the state line out of the frame. */
  state?: string | null;
  /** Absent leaves the summary line out of the frame. */
  summary?: string[];
  /** Lines after the summary, as the daemon writes them: the note, the artifacts. */
  tail?: string[];
  /** `false` drops the closing tag, as a size bound that cut the frame does. */
  closed?: boolean;
}

function finish({
  state = "completed",
  summary,
  tail = [],
  closed = true,
}: FinishOptions = {}): string {
  const [first = "", ...rest] = summary ?? [];
  return [
    "<devboule-system>",
    "origin: local",
    "role: daemon",
    "from_agent: s.child.7",
    "kind: agent_finished",
    "timestamp: 1760000000000",
    "childSessionId: s.child.7",
    "displayName: worker one",
    ...(state === null ? [] : [`state: ${state}`]),
    ...(summary === undefined ? [] : [`summary: ${first}`, ...rest]),
    ...tail,
    ...(closed ? ["</devboule-system>"] : []),
  ].join("\n");
}

function itemFrom(text: string): DaemonNoticeItem {
  const notice = parseAgentDaemonNotice(text);
  if (notice === null) throw new Error("the fixture is not a daemon notice");
  return { id: "notice-1", role: "daemon_notice", notice };
}

const sixLines = ["line 1", "line 2", "line 3", "line 4", "line 5", "line 6"];

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement;

async function unmountCard(): Promise<void> {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host?.remove();
}

afterEach(unmountCard);

async function render(item: DaemonNoticeItem): Promise<HTMLElement> {
  await unmountCard();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<DaemonNoticeCard item={item} />));
  return host;
}

function expander(): HTMLButtonElement | null {
  return host.querySelector<HTMLButtonElement>(".workspace-chat-tool-more");
}

async function press(button: HTMLButtonElement | null): Promise<void> {
  await act(async () => button?.click());
}

function copyText(): string | null | undefined {
  return host.querySelector(".workspace-chat-copy")?.textContent;
}

describe("the finish report's header", () => {
  it("names the child and its state as a message from the subagent", async () => {
    await render(itemFrom(finish({ summary: ["ok"] })));
    expect(copyText()).toBe("Message from subagent · worker one · completed");
    expect(host.querySelector(".workspace-chat-copy")?.getAttribute("title")).toBe(
      "worker one · s.child.7 · state: completed",
    );
  });

  it("leaves the state out when the frame carries none", async () => {
    await render(itemFrom(finish({ state: null, summary: ["ok"] })));
    expect(copyText()).toBe("Message from subagent · worker one");
  });
});

describe("the finish report's summary", () => {
  it("shows the first four lines collapsed, with a Show more expander", async () => {
    await render(itemFrom(finish({ summary: sixLines })));
    const quoted = host.querySelector(".workspace-chat-child-said");
    expect(quoted?.classList.contains("is-collapsed")).toBe(true);
    expect(quoted?.querySelector("blockquote")?.textContent).toBe("line 1\nline 2\nline 3\nline 4");
    expect(expander()?.textContent).toBe("Show more");
    expect(expander()?.getAttribute("aria-expanded")).toBe("false");
  });

  it("opens to the full summary on Show more and folds again on Show less", async () => {
    await render(itemFrom(finish({ summary: sixLines })));
    await press(expander());
    const quoted = host.querySelector(".workspace-chat-child-said");
    expect(quoted?.classList.contains("is-collapsed")).toBe(false);
    expect(quoted?.querySelector("blockquote")?.textContent).toBe(sixLines.join("\n"));
    expect(expander()?.textContent).toBe("Show less");
    expect(expander()?.getAttribute("aria-expanded")).toBe("true");
    await press(expander());
    expect(host.querySelector(".workspace-chat-child-said blockquote")?.textContent).toBe(
      "line 1\nline 2\nline 3\nline 4",
    );
  });

  it("has no expander for a short summary", async () => {
    await render(itemFrom(finish({ summary: ["build is green", "all checks passed"] })));
    expect(expander()).toBeNull();
    expect(
      host.querySelector(".workspace-chat-child-said")?.classList.contains("is-collapsed"),
    ).toBe(false);
  });

  it("collapses a single long line too, though it has no second line to hide", async () => {
    const oneLine = "x".repeat(450);
    await render(itemFrom(finish({ summary: [oneLine] })));
    expect(expander()?.textContent).toBe("Show more");
    expect(
      host.querySelector(".workspace-chat-child-said")?.classList.contains("is-collapsed"),
    ).toBe(true);
    expect(host.querySelector(".workspace-chat-child-said blockquote")?.textContent).toBe(oneLine);
  });

  it("keeps the note and no expander when the frame carries no summary", async () => {
    await render(itemFrom(finish({ summary: undefined })));
    expect(host.querySelector(".workspace-chat-child-said-note")?.textContent).toContain(
      "no finish summary",
    );
    expect(host.querySelector(".workspace-chat-child-said")).toBeNull();
    expect(expander()).toBeNull();
  });
});

describe("the finish report's tail", () => {
  it("stays hidden until the report is expanded", async () => {
    await render(
      itemFrom(finish({ summary: ["build is green"], tail: ["note: one flake retried"] })),
    );
    expect(host.querySelector(".workspace-chat-unattributed")).toBeNull();
    expect(expander()?.textContent).toBe("Show more");
    await press(expander());
    expect(host.querySelector(".workspace-chat-unattributed blockquote")?.textContent).toBe(
      "note: one flake retried",
    );
  });

  it("keeps the cut-off note outside the expander, visible while collapsed", async () => {
    await render(itemFrom(finish({ summary: sixLines, closed: false })));
    expect(expander()?.textContent).toBe("Show more");
    const note = host.querySelector(".workspace-chat-child-said-note");
    expect(note?.textContent).toContain("cut off in transit");
  });
});

describe("a summary with no words", () => {
  it("reads as no summary when it holds only whitespace", async () => {
    const item = itemFrom(finish({ summary: ["   ", "\t", ""] }));
    // The parser keeps the blank continuation lines: the card is the one that
    // must treat them as no words.
    expect(item.notice).toMatchObject({ summary: expect.any(String) });
    await render(item);
    expect(host.querySelector(".workspace-chat-child-said-note")?.textContent).toContain(
      "no finish summary",
    );
    expect(host.querySelector(".workspace-chat-child-said")).toBeNull();
    expect(expander()).toBeNull();
  });
});

describe("the expander's thresholds", () => {
  it("shows no expander for exactly four lines", async () => {
    await render(itemFrom(finish({ summary: ["line 1", "line 2", "line 3", "line 4"] })));
    expect(expander()).toBeNull();
  });

  it("shows the expander from the fifth line", async () => {
    await render(itemFrom(finish({ summary: ["line 1", "line 2", "line 3", "line 4", "line 5"] })));
    expect(expander()?.textContent).toBe("Show more");
  });

  it("counts CRLF breaks as line breaks, and collapses them to LF", async () => {
    await render(itemFrom(finish({ summary: ["line 1\r\nline 2\r\nline 3\r\nline 4"] })));
    expect(expander()).toBeNull();
    await render(itemFrom(finish({ summary: ["line 1\r\nline 2\r\nline 3\r\nline 4\r\nline 5"] })));
    expect(expander()?.textContent).toBe("Show more");
    expect(host.querySelector(".workspace-chat-child-said blockquote")?.textContent).toBe(
      "line 1\nline 2\nline 3\nline 4",
    );
  });

  it("allows 400 graphemes on one line and shows the expander from 401", async () => {
    await render(itemFrom(finish({ summary: ["a".repeat(400)] })));
    expect(expander()).toBeNull();
    await render(itemFrom(finish({ summary: ["a".repeat(401)] })));
    expect(expander()?.textContent).toBe("Show more");
  });

  it("counts a combining mark with its letter, not as a unit of its own", async () => {
    // 400 decomposed letters are 800 UTF-16 units but 400 graphemes.
    await render(itemFrom(finish({ summary: ["é".repeat(400)] })));
    expect(expander()).toBeNull();
    await render(itemFrom(finish({ summary: ["é".repeat(401)] })));
    expect(expander()?.textContent).toBe("Show more");
  });
});

describe("the expander's relationship to its region", () => {
  it("names the region that holds the summary, and the tail once open", async () => {
    await render(itemFrom(finish({ summary: sixLines, tail: ["note: one flake retried"] })));
    const id = expander()?.getAttribute("aria-controls") ?? "";
    expect(id).not.toBe("");
    const region = () => document.getElementById(id);
    expect(region()?.querySelector(".workspace-chat-child-said blockquote")?.textContent).toBe(
      "line 1\nline 2\nline 3\nline 4",
    );
    expect(region()?.querySelector(".workspace-chat-unattributed")).toBeNull();
    await press(expander());
    expect(region()?.querySelector(".workspace-chat-unattributed blockquote")?.textContent).toBe(
      "note: one flake retried",
    );
  });
});

describe("the other notice kinds", () => {
  it("renders the quiet notice as before, with no expander", async () => {
    await render(
      itemFrom(
        [
          "<devboule-system>",
          "origin: local",
          "role: daemon",
          "from_agent: s.child.7",
          "kind: agent_quiet",
          "timestamp: 1760000000000",
          "childSessionId: s.child.7",
          "displayName: worker one",
          "state: working",
          "idleMs: 1234567",
          "</devboule-system>",
        ].join("\n"),
      ),
    );
    expect(copyText()).toBe(
      "Its child worker one has produced no output for 20 minutes. It may be thinking, building, or stuck; nothing was stopped.",
    );
    expect(expander()).toBeNull();
  });

  it("renders the idle-close notice as before, with no expander", async () => {
    await render(
      itemFrom(
        [
          "<devboule-system>",
          "origin: local",
          "role: daemon",
          "from_agent: s.child.7",
          "kind: agent_idle_closed",
          "timestamp: 1760000000000",
          "childSessionId: s.child.7",
          "displayName: worker one",
          "state: closed",
          "idleMinutes: 30",
          "</devboule-system>",
        ].join("\n"),
      ),
    );
    expect(copyText()).toBe("Its child worker one was closed: idle after 30 minutes.");
    expect(expander()).toBeNull();
  });

  it("renders the input-required notice as before, with no expander", async () => {
    await render(
      itemFrom(
        [
          "<devboule-system>",
          "origin: local",
          "role: daemon",
          "from_agent: s.child.7",
          "kind: agent_input_required",
          "timestamp: 1760000000000",
          "childSessionId: s.child.7",
          "displayName: worker one",
          "state: input_required",
          "</devboule-system>",
        ].join("\n"),
      ),
    );
    expect(copyText()).toBe(
      "Its child worker one is waiting for a person to answer a permission card.",
    );
    expect(expander()).toBeNull();
  });

  it("renders an unknown kind as before, with no expander", async () => {
    await render(
      itemFrom(
        [
          "<devboule-system>",
          "origin: local",
          "role: daemon",
          "from_agent: s.child.9",
          "kind: agent_hibernating",
          "timestamp: 1760000000000",
          "childSessionId: s.child.9",
          "someFutureField: whatever the future carries",
          "</devboule-system>",
        ].join("\n"),
      ),
    );
    expect(copyText()).toBe(
      "The daemon sent a notice this version of the app does not know how to format. It declared kind: agent_hibernating. It concerns child session s.child.9.",
    );
    expect(expander()).toBeNull();
  });
});
