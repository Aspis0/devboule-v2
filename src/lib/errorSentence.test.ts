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
  it("maps 'no agent on PATH' without the env var or the acronym", () => {
    // acp_client.rs:584 — the owner's sighting.
    const { sentence } = errorSentence(
      rejection(
        "io",
        "No ACP-capable agent was found on PATH. Set DEVBOULE_ACP_COMMAND to a non-empty JSON string array to choose an ACP command explicitly.",
      ),
    );
    expect(sentence).toBe(
      "No agent CLI is installed on this machine. Install one — for example grok, claude, or gemini — and restart Devboule.",
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

  it("maps process containment, keeping the OS error only as detail", () => {
    // provider.rs ~1098 — the owner's "os error 5" sighting.
    const { sentence, detail } = errorSentence(
      rejection("io", "Could not contain the terminal process: Access is denied. (os error 5)"),
    );
    expect(sentence).toBe(
      "The system refused to start the terminal. Another program on this machine may be blocking it.",
    );
    expect(detail).toContain("Access is denied");
    expect(sentence).not.toContain("os error");
  });

  it("maps agent-process containment to the agent sentence", () => {
    // acp_client.rs:757.
    const { sentence } = errorSentence(
      rejection("io", "Could not contain the ACP agent process: Access is denied. (os error 5)"),
    );
    expect(sentence).toContain("the agent");
    expect(sentence).not.toContain("the terminal");
  });

  it("maps a lost subscription without the internal words", () => {
    // session_runtime.rs:2610.
    const { sentence } = errorSentence(
      rejection("invalid_request", "Session is not attached to this subscription."),
    );
    expect(sentence).toBe(
      "This view lost its live connection to the session. Reopen the tab to reconnect.",
    );
    expect(sentence).not.toContain("subscription");
    expect(sentence).not.toContain("attached");
  });

  it("maps the bridge's lost bookkeeping without the internal words", () => {
    // src-tauri/src/client/mod.rs:698.
    const { sentence } = errorSentence(
      rejection("internal", "session attachment is not registered"),
    );
    expect(sentence).toBe(
      "This view lost its live connection to the session. Reopen the tab to reconnect.",
    );
    expect(sentence).not.toContain("registered");
    expect(sentence).not.toContain("attachment");
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
