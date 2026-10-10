import { useEffect, useState } from "react";
import { errorSentence } from "../../../lib/errorSentence";
import { sessionsSearch } from "../../../lib/tauri";
import type { SessionSearchHit, SessionSearchReply } from "../../../types/ipc";

/** The quiet time after the last keystroke before the search goes to the daemon. */
export const TRANSCRIPT_SEARCH_DEBOUNCE_MS = 250;

export interface TranscriptSearchProps {
  /** The daemon agreed `sessions.search`; without it the section says so instead of searching. */
  canSearch: boolean;
  /** The session's title as the sidebar shows it. */
  titleOf: (sessionId: string) => string;
  /** Opens the session at the matching message. */
  onOpen: (hit: SessionSearchHit) => void;
}

type SearchState =
  | { status: "idle" }
  | { status: "searching" }
  | { status: "done"; reply: SessionSearchReply }
  | { status: "failed"; sentence: string };

/**
 * The sidebar's message matches for the Ctrl+K query: what the person and the
 * agent wrote in each session, newest first. The daemon scans the transcripts
 * in a bounded pass, so the query is sent once the typing pauses, and an answer
 * that arrives after the query changed is dropped.
 */
export function TranscriptSearchResults({
  query,
  canSearch,
  titleOf,
  onOpen,
}: TranscriptSearchProps & { query: string }) {
  const trimmed = query.trim();
  const [state, setState] = useState<SearchState>({ status: "idle" });

  useEffect(() => {
    if (!canSearch || trimmed === "") {
      setState({ status: "idle" });
      return;
    }
    let current = true;
    setState({ status: "searching" });
    const timer = setTimeout(() => {
      sessionsSearch(trimmed).then(
        (reply) => {
          if (current) setState({ status: "done", reply });
        },
        (cause: unknown) => {
          if (current) setState({ status: "failed", sentence: errorSentence(cause).sentence });
        },
      );
    }, TRANSCRIPT_SEARCH_DEBOUNCE_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [canSearch, trimmed]);

  if (!canSearch) {
    return <div className="transcript-search-note">Message search needs an updated daemon.</div>;
  }
  if (state.status === "idle") return null;
  if (state.status === "searching") {
    return <div className="transcript-search-note">Searching messages…</div>;
  }
  if (state.status === "failed") {
    return (
      <div className="transcript-search-note" role="alert">
        {state.sentence}
      </div>
    );
  }
  const { hits, truncated } = state.reply;
  if (hits.length === 0) {
    return <div className="transcript-search-note">No messages match.</div>;
  }
  return (
    <div className="transcript-search" role="list" aria-label="Messages">
      {hits.map((hit) => (
        <button
          type="button"
          role="listitem"
          className="transcript-search-hit"
          key={`${hit.sessionId}:${hit.generation}:${hit.seq}`}
          onClick={() => onOpen(hit)}
        >
          <span className="transcript-search-title">{titleOf(hit.sessionId)}</span>
          <span className="transcript-search-snippet">
            {hit.author === "person" ? "You: " : "Agent: "}
            {hit.snippet}
          </span>
        </button>
      ))}
      {truncated ? (
        <div className="transcript-search-note">More matches exist. Type more to narrow.</div>
      ) : null}
    </div>
  );
}
