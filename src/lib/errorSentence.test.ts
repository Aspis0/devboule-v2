import { describe, expect, it } from "vitest";
import { CODE_SENTENCES, errorSentence } from "./errorSentence";
import type { ErrorCode } from "../types/ipc";

/**
 * The daemon's code enum, pinned by `error_code_matches_frontend_union`
 * (`crates/devboule-protocol/src/error.rs`) and mirrored in
 * `src/types/ipc.ts`. Hard-coded here so the walked table below fails if
 * either side grows without this file noticing.
 */
const ALL_CODES: readonly ErrorCode[] = [
  "protocol_version_mismatch",
  "unauthorized",
  "unimplemented",
  "capability_not_supported",
  "invalid_request",
  "session_not_found",
  "session_generation_mismatch",
  "idempotency_conflict",
  "operation_conflict",
  "operation_in_flight",
  "shutting_down",
  "journal",
  "workspace_unavailable",
  "workspace_confinement_refused",
  "internal",
  "io",
  "connection_lost",
];

/** A message no shape arm claims, so the walk lands on the code table. */
const NEUTRAL_MESSAGE = "";

const rejection = (code: ErrorCode, message: string): unknown => ({ code, message });

describe("the walked code table", () => {
  it("reads a plain Tauri WireError without rendering [object Object]", () => {
    const cause = { code: "io", message: "the named pipe is busy" };
    expect(cause).not.toBeInstanceOf(Error);
    expect(errorSentence(cause)).toEqual({ sentence: CODE_SENTENCES.io, detail: cause.message });
  });

  it("has a row for every code the daemon can send, and no row nobody sends", () => {
    for (const code of ALL_CODES) {
      expect(CODE_SENTENCES[code], `no row for ${code}`).toBeTruthy();
    }
    expect(Object.keys(CODE_SENTENCES).sort()).toEqual([...ALL_CODES].sort());
  });

  it("answers every code with its own sentence, even with an empty message", () => {
    for (const code of ALL_CODES) {
      const { sentence } = errorSentence(rejection(code, NEUTRAL_MESSAGE));
      expect(sentence, code).toBe(CODE_SENTENCES[code]);
      expect(sentence.trim().length).toBeGreaterThan(0);
    }
  });

  it("never puts the raw daemon text in the sentence when no arm claims it", () => {
    // "Could not save the agent profiles: {error}" — server/stores.rs.
    const { sentence, detail } = errorSentence(
      rejection("io", "Could not save the agent profiles: Access is denied. (os error 5)"),
    );
    expect(sentence).toBe(CODE_SENTENCES.io);
    expect(detail).toContain("os error 5");
  });

  it("keeps the raw text of an unknown code (a newer daemon) out of the sentence", () => {
    const { sentence } = errorSentence({ code: "hyperdrive", message: "graviton offline" });
    expect(sentence).not.toContain("graviton");
  });
});

describe("the message-shape arms", () => {
  it("maps 'no agent on PATH' to the refresh instruction, not a restart", () => {
    // acp_client.rs:584 — the owner's sighting. A1's providers_refresh
    // (server/providers.rs:59-64) rescans PATH without a restart.
    const { sentence } = errorSentence(
      rejection(
        "io",
        "No ACP-capable agent was found on PATH. Set DEVBOULE_ACP_COMMAND to a non-empty JSON string array to choose an ACP command explicitly.",
      ),
    );
    expect(sentence).toBe(
      "No agent CLI is installed on this machine. Install one — for example grok, claude, or gemini — then choose Refresh in Settings → Providers.",
    );
  });

  it("maps a named provider the machine lacks, naming the provider", () => {
    // acp_client.rs:622+.
    const { sentence } = errorSentence(
      rejection("io", "ACP agent 'grok' was not found on PATH or in the ACP registry."),
    );
    expect(sentence).toBe(
      "grok is not available on this machine. Install it, or pick another agent.",
    );
  });

  it("maps a named provider with no command, naming the provider", () => {
    // acp_client.rs:622+.
    const { sentence } = errorSentence(
      rejection("invalid_request", "Provider 'claude' has no command to spawn."),
    );
    expect(sentence).toBe(
      "claude is not available on this machine. Install it, or pick another agent.",
    );
  });

  it("maps a family client's own 'not found on PATH' text, naming the family", () => {
    // claude_client.rs:118.
    const { sentence } = errorSentence(
      rejection(
        "io",
        "Claude was not found on PATH. Set DEVBOULE_CLAUDE_COMMAND to a non-empty JSON string array to choose the Claude command explicitly.",
      ),
    );
    expect(sentence).toContain("Claude");
    expect(sentence).not.toContain("DEVBOULE_CLAUDE_COMMAND");
    expect(sentence).not.toContain("PATH");
  });

  it("keeps the reopen sentence for the bridge's lost bookkeeping", () => {
    // src-tauri/src/client/mod.rs:705.
    const { sentence } = errorSentence(
      rejection("internal", "session attachment is not registered"),
    );
    expect(sentence).toBe(
      "This view lost its live connection to the session. Reopen the tab to reconnect.",
    );
    expect(sentence).not.toContain("registered");
    expect(sentence).not.toContain("attachment");
  });

  it("keeps the reopen sentence for the bridge's 'no longer registered' loss", () => {
    // src-tauri/src/client/mod.rs:552 — bind_with_cursor's own wording.
    const { sentence } = errorSentence(
      rejection("internal", "session attachment is no longer registered"),
    );
    expect(sentence).toBe(
      "This view lost its live connection to the session. Reopen the tab to reconnect.",
    );
  });

  it("leaves the daemon's own 'attach before sending' failure to its code row", () => {
    // crates/devboule-daemon/src/client.rs:1824: no control subscription for
    // this session (DaemonError::Protocol, which the bridge maps to `internal`
    // — src-tauri/src/backend/error.rs:59). Reopening the tab is advice for a
    // different failure, so the raw text stays in the detail.
    const message = "Session is not attached; attach before sending session commands.";
    const { sentence, detail } = errorSentence(rejection("internal", message));
    expect(sentence).toBe(CODE_SENTENCES.internal);
    expect(detail).toBe(message);
    expect(sentence).not.toContain("Reopen");
  });

  it("leaves a subscription-scoped 'not attached' refusal to its code row", () => {
    // crates/devboule-daemon/src/session_runtime.rs:3295, :3386, :3407
    // (ErrorCode::InvalidRequest) and client.rs:415's bare test-harness form:
    // none of them is this view's attachment bookkeeping.
    const subscription = "Session is not attached to this subscription.";
    expect(errorSentence(rejection("invalid_request", subscription))).toEqual({
      sentence: CODE_SENTENCES.invalid_request,
      detail: subscription,
    });
    for (const message of [subscription, "session is not attached"]) {
      const { sentence } = errorSentence(rejection("internal", message));
      expect(sentence, message).not.toContain("Reopen");
      expect(sentence, message).not.toContain("reconnect");
    }
  });

  it("gives the calling-session refusal its own true reading, not the reopen sentence", () => {
    // session.rs:2909 (also session_child_commands.rs:181 and :295,
    // session_child_permission.rs:70, session_child_profile.rs:59): the
    // CALLING session is not a registered
    // creator — a validity refusal, not this view's connection.
    const { sentence } = errorSentence(
      rejection("invalid_request", "the calling session is not registered on this daemon"),
    );
    expect(sentence).toBe(CODE_SENTENCES.invalid_request);
    expect(sentence).not.toContain("Reopen");
  });

  it("states a containment access refusal without diagnosing a blocker", () => {
    // provider.rs:1057 -> 1096-1102: the process spawned and was then killed
    // by the daemon's own job handling; acp_client.rs:748-752 measures the
    // access denial as the daemon's own job hierarchy, never another program.
    const { sentence, detail } = errorSentence(
      rejection("io", "Could not contain the terminal process: Access is denied. (os error 5)"),
    );
    expect(sentence).toBe("The system could not start the terminal.");
    expect(detail).toContain("Access is denied");
    expect(sentence).not.toContain("blocking");
  });

  it("does not take an OS code that merely starts with 5 for an access refusal", () => {
    // session.rs:3307-3322 formats "(OS error {code}: ...)" — 50, 500, 577
    // all contain "os error 5" as a substring.
    const { sentence } = errorSentence(
      rejection("io", "Could not contain the agent process: (OS error 50: not supported)"),
    );
    expect(sentence).toBe("The system could not start the agent.");
    expect(sentence).not.toContain("blocking");
  });

  it("names the agent, not the terminal, for agent-process containment", () => {
    // acp_client.rs:757.
    const { sentence } = errorSentence(
      rejection("io", "Could not contain the ACP agent process: Access is denied. (os error 5)"),
    );
    expect(sentence).toBe("The system could not start the agent.");
    expect(sentence).not.toContain("the terminal");
  });

  it("states a spawn failure without an OS verdict as what happened, no diagnosis", () => {
    // session_terminal_transcript_tests.rs:815-818 (workspace_spawn_error,
    // session_workspaces.rs:427-435): resource exhaustion is not a block.
    const { sentence } = errorSentence(
      rejection("io", "Could not start the terminal shell. (OS error 1450: no system resources)"),
    );
    expect(sentence).toBe("The system could not start the terminal.");
    expect(sentence).not.toContain("blocking");
  });

  it("lets a working-directory failure fall to the io row, not a start refusal", () => {
    // claude_client.rs:98: nothing was refused — the directory could not be
    // established, an ordinary system failure.
    const { sentence } = errorSentence(
      rejection(
        "io",
        "Could not determine agent working directory: Access is denied. (os error 5)",
      ),
    );
    expect(sentence).toBe(CODE_SENTENCES.io);
  });

  it("does not claim a workspace birth failed when a workspace was being removed", () => {
    // worktree.rs:496-501 via session_workspaces.rs:284-300: the force-
    // removal recovery on the DELETE path also says "leftover checkout".
    const { sentence } = errorSentence(
      rejection(
        "workspace_unavailable",
        "Could not remove worktree 'w-1': fatal: dirty tree; failed to remove leftover checkout C:\\repo\\x: remove failed",
      ),
    );
    expect(sentence).toBe(CODE_SENTENCES.workspace_unavailable);
    expect(sentence).not.toContain("created");
  });

  it("maps the create path's journal-leftover text to the workspace's own sentence", () => {
    // session_workspaces.rs:139-151: journal.workspace_create failed AND the
    // checkout cleanup failed too, ErrorCode::Journal — the branch's exact
    // shape is "{journal error}; leftover checkout at '{path}' ({cleanup_error})".
    // The journal row would be true of the write and silent about the
    // workspace that was never created.
    const { sentence, detail } = errorSentence(
      rejection(
        "journal",
        "journal is unavailable: disk on fire; leftover checkout at 'C:\\repo\\proj' (remove failed)",
      ),
    );
    expect(sentence).toBe("This workspace could not be created.");
    expect(sentence).not.toContain("history");
    expect(detail).toContain("leftover checkout at");
  });

  it("maps the worktree-add branch of a failed birth to the same sentence", () => {
    // session_workspaces.rs:122-134, ErrorCode::WorkspaceUnavailable: the
    // checkout was never created (with or without a leftover), so the
    // workspace_unavailable row's "folder not available" would misplace it.
    for (const message of [
      "Could not add git worktree for 'p-1': fatal: 'x' already exists",
      "Could not add git worktree for 'p-1': fatal: 'x' already exists; leftover checkout at 'C:\\repo\\x' (remove failed)",
    ]) {
      const { sentence } = errorSentence(rejection("workspace_unavailable", message));
      expect(sentence).toBe("This workspace could not be created.");
    }
  });

  it("lets the daemon's reader failure fall to the internal row, not a start refusal", () => {
    // session_spawn.rs:381 and :439: the SHELL spawned and the PTY exists;
    // what failed is spawning the daemon's own reader thread, and the session
    // is closed over it. "The system could not start the terminal" would
    // claim the terminal never started — the internal row claims only what is
    // true: the daemon went wrong inside.
    const { sentence, detail } = errorSentence(
      rejection("internal", "Could not start the terminal reader."),
    );
    expect(sentence).toBe(CODE_SENTENCES.internal);
    expect(sentence).not.toContain("terminal");
    expect(detail).toContain("terminal reader");
  });

  it("keeps the journal row for the store's own failures", () => {
    // journal.rs:177 (JournalError::Corrupt et al via the From impl) and
    // session.rs:3298 ("The conversation journal is unavailable.").
    for (const message of [
      "journal is corrupt: unexpected tail",
      "The conversation journal is unavailable.",
    ]) {
      const { sentence } = errorSentence(rejection("journal", message));
      expect(sentence).toBe(CODE_SENTENCES.journal);
    }
  });
  it("reads a vanished workspace file as gone, not as a permission failure", () => {
    const { sentence, detail } = errorSentence(
      rejection("io", "the file no longer exists in the workspace"),
    );
    expect(sentence).toBe("This file no longer exists in the workspace.");
    expect(detail).toBe("the file no longer exists in the workspace");
  });

  it("reads a denied workspace file as denied, not as gone", () => {
    const { sentence } = errorSentence(rejection("io", "this machine denied access to this file"));
    expect(sentence).toBe("This machine denied access to this file.");
  });

  it("states a failed editor launch as the launch, with no path in it", () => {
    const { sentence, detail } = errorSentence(rejection("io", "the editor could not be started"));
    expect(sentence).toBe("The editor could not be started.");
    expect(detail).toBe("the editor could not be started");
  });

  it("states a failed reveal as the file manager, not as an editor launch", () => {
    const { sentence } = errorSentence(rejection("io", "the file manager could not be opened"));
    expect(sentence).toBe("The file manager could not be opened.");
  });
});

describe("the workspace-delete refusals", () => {
  it("maps the live-session refusal to the stop instruction, not the generic row", () => {
    // crates/devboule-daemon/src/session_workspaces.rs — ErrorCode::InvalidRequest
    // with no details, so the daemon's one stable message is the only handle.
    const raw = "sessions or terminals are still running in this workspace; close them first";
    const { sentence } = errorSentence(rejection("invalid_request", raw));
    expect(sentence).toBe("Stop the agents and terminals in this workspace first.");
    expect(sentence).not.toContain("close them first");
  });

  it("maps a dirty-worktree refusal by its wire kind, not the path-carrying message", () => {
    // ErrorDetails::WorktreeDirty — the message quotes the checkout path,
    // so it can never be a stable handle.
    const { sentence } = errorSentence({
      code: "invalid_request",
      message:
        "fatal: 'C:\\dev\\wt-x' contains modified or untracked files, use --force to delete it",
      details: { type: "worktree_dirty", path: "C:\\dev\\wt-x", force_required: true },
    });
    expect(sentence).toBe("This worktree has uncommitted changes. Commit or discard them first.");
    expect(sentence).not.toContain("wt-x");
    expect(sentence).not.toContain("--force");
  });
});

describe("causes that are not daemon rejections", () => {
  it("passes an app-authored Error message through as the sentence", () => {
    expect(errorSentence(new Error("The daemon returned an invalid session sub."))).toEqual({
      sentence: "The daemon returned an invalid session sub.",
      detail: null,
    });
  });

  it("passes a plain string through as the sentence", () => {
    expect(errorSentence("Could not load sessions. The daemon is unreachable.")).toEqual({
      sentence: "Could not load sessions. The daemon is unreachable.",
      detail: null,
    });
  });

  it("falls back to one generic sentence when there is nothing to read", () => {
    for (const cause of [undefined, null, {}, new Error("   "), "", 42]) {
      expect(errorSentence(cause).sentence).toBe("Devboule could not complete that action.");
    }
  });

  it("never returns an empty sentence for an empty daemon message", () => {
    expect(errorSentence(rejection("internal", "   ")).sentence.trim().length).toBeGreaterThan(0);
  });
});

describe("raw causes that carry the daemon's own words", () => {
  it("maps a raw attach refusal instead of showing the daemon's words as the sentence", () => {
    const raw = "session attachment is not registered";
    const { sentence, detail } = errorSentence(new Error(raw));
    expect(sentence).toBe(
      "This view lost its live connection to the session. Reopen the tab to reconnect.",
    );
    expect(sentence).not.toContain("registered");
    expect(detail).toBe(raw);
  });

  it("keeps protocol words it has no arm for inside the detail, never the sentence", () => {
    const raw = "session attachment is not registered for subscription 7 (generation 3)";
    const { sentence, detail } = errorSentence(raw);
    expect(sentence).toBe(
      "This view lost its live connection to the session. Reopen the tab to reconnect.",
    );
    expect(sentence).not.toContain("subscription");
    expect(sentence).not.toContain("generation");
    expect(detail).toBe(raw);
  });

  it("maps the supervisor's silence report to the reconnect advice", () => {
    const raw =
      "The daemon has not answered status checks for at least 4 seconds (3 consecutive failures).";
    const { sentence, detail } = errorSentence(new Error(raw));
    expect(sentence).toBe(
      "Devboule is having trouble reaching its background service. Try reconnecting or restart Devboule.",
    );
    expect(detail).toBe(raw);
  });

  it("maps the status poll's two short failure tokens to the same advice", () => {
    for (const raw of ["daemon unreachable", "daemon_status timed out"]) {
      const { sentence, detail } = errorSentence(raw);
      expect(sentence, raw).toBe(
        "Devboule is having trouble reaching its background service. Try reconnecting or restart Devboule.",
      );
      expect(detail, raw).toBe(raw);
    }
  });

  it("lets an app-authored sentence no arm claims stand as the sentence", () => {
    const sentence = "The daemon returned an invalid session subscription.";
    expect(errorSentence(new Error(sentence))).toEqual({ sentence, detail: null });
  });
});
