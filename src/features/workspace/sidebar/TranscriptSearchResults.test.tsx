// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionSearchHit, SessionSearchReply } from "../../../types/ipc";
import { TRANSCRIPT_SEARCH_DEBOUNCE_MS, TranscriptSearchResults } from "./TranscriptSearchResults";

const sessionsSearch = vi.fn<(query: string) => Promise<SessionSearchReply>>();
vi.mock("../../../lib/tauri", () => ({
  sessionsSearch: (query: string) => sessionsSearch(query),
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const hit = (overrides: Partial<SessionSearchHit> = {}): SessionSearchHit => ({
  sessionId: "s1",
  generation: 1,
  seq: 4,
  messageId: "m1",
  author: "agent",
  snippet: "deploy the Kalsa site",
  ...overrides,
});

let container: HTMLDivElement;
let root: Root;

function render(props: {
  query: string;
  canSearch?: boolean;
  onOpen?: (hit: SessionSearchHit) => void;
}): void {
  root.render(
    <TranscriptSearchResults
      query={props.query}
      canSearch={props.canSearch ?? true}
      titleOf={(sessionId) => `title ${sessionId}`}
      onOpen={props.onOpen ?? (() => undefined)}
    />,
  );
}

async function flush(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  sessionsSearch.mockReset();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.useRealTimers();
});

describe("TranscriptSearchResults", () => {
  it("waits for the typing to pause before it asks the daemon", async () => {
    sessionsSearch.mockResolvedValue({ hits: [], truncated: false, stoppedEarly: false });
    await act(async () => render({ query: "kal" }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS - 1);
    expect(sessionsSearch).not.toHaveBeenCalled();
    await flush(1);
    expect(sessionsSearch).toHaveBeenCalledTimes(1);
    expect(sessionsSearch).toHaveBeenCalledWith("kal");
  });

  it("asks once for a query that changed while the first was still waiting", async () => {
    sessionsSearch.mockResolvedValue({ hits: [], truncated: false, stoppedEarly: false });
    await act(async () => render({ query: "k" }));
    await flush(100);
    await act(async () => render({ query: "ka" }));
    await flush(100);
    await act(async () => render({ query: "kal" }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    expect(sessionsSearch.mock.calls.map(([query]) => query)).toEqual(["kal"]);
  });

  it("shows each match with its session title and who wrote it, and opens it on click", async () => {
    const onOpen = vi.fn<(hit: SessionSearchHit) => void>();
    sessionsSearch.mockResolvedValue({
      hits: [hit(), hit({ seq: 9, author: "person", snippet: "the Kalsa logo" })],
      truncated: false,
      stoppedEarly: false,
    });
    await act(async () => render({ query: "kalsa", onOpen }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    const rows = [...container.querySelectorAll<HTMLButtonElement>(".transcript-search-hit")];
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("title s1");
    expect(rows[0].textContent).toContain("Agent: deploy the Kalsa site");
    expect(rows[1].textContent).toContain("You: the Kalsa logo");
    await act(async () => rows[1].click());
    expect(onOpen).toHaveBeenCalledWith(
      hit({ seq: 9, author: "person", snippet: "the Kalsa logo" }),
    );
  });

  it("says when the scan stopped early, which is not the same as more matches", async () => {
    sessionsSearch.mockResolvedValueOnce({ hits: [hit()], truncated: false, stoppedEarly: true });
    await act(async () => render({ query: "kalsa" }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    expect(container.textContent).toContain("stopped early");
    expect(container.textContent).not.toContain("More matches exist.");
  });

  it("says so when no message matches, and when the scan stopped at a cap", async () => {
    sessionsSearch.mockResolvedValueOnce({ hits: [], truncated: false, stoppedEarly: false });
    await act(async () => render({ query: "nothing" }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    expect(container.textContent).toContain("No messages match.");

    sessionsSearch.mockResolvedValueOnce({ hits: [hit()], truncated: true, stoppedEarly: false });
    await act(async () => render({ query: "kalsa" }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    expect(container.textContent).toContain("More matches exist.");
  });

  it("drops an answer that arrives after the query moved on", async () => {
    let answerFirst: (reply: SessionSearchReply) => void = () => undefined;
    sessionsSearch.mockImplementationOnce(
      () =>
        new Promise<SessionSearchReply>((resolve) => {
          answerFirst = resolve;
        }),
    );
    await act(async () => render({ query: "old" }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    sessionsSearch.mockResolvedValueOnce({
      hits: [hit({ snippet: "new words" })],
      truncated: false,
      stoppedEarly: false,
    });
    await act(async () => render({ query: "new" }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    await act(async () =>
      answerFirst({
        hits: [hit({ snippet: "stale words" })],
        truncated: false,
        stoppedEarly: false,
      }),
    );
    expect(container.textContent).toContain("new words");
    expect(container.textContent).not.toContain("stale words");
  });

  it("names the missing capability instead of searching", async () => {
    await act(async () => render({ query: "kalsa", canSearch: false }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    expect(sessionsSearch).not.toHaveBeenCalled();
    expect(container.textContent).toContain("needs an updated daemon");
  });

  it("shows the daemon's sentence when the search fails", async () => {
    sessionsSearch.mockRejectedValueOnce(new Error("the journal is busy"));
    await act(async () => render({ query: "kalsa" }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("the journal is busy");
  });

  it("draws nothing for an empty query", async () => {
    await act(async () => render({ query: "   " }));
    await flush(TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    expect(sessionsSearch).not.toHaveBeenCalled();
    expect(container.textContent).toBe("");
  });
});
