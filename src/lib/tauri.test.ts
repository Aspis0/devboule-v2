import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Channel } from "@tauri-apps/api/core";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  COMMAND_ARG_KEYS,
  devicesList,
  invokeTyped,
  isCommandError,
  journalRetentionGet,
  journalRetentionSet,
  journalUsage,
  oracleAskFolder,
  oracleFolderStatus,
  pairingComplete,
  pairingConfirm,
  pairingStart,
  peerRevoke,
  peerSetCaps,
  providersRefresh,
  sessionAttach,
  sessionClaim,
  sessionClose,
  sessionDetach,
  sessionInterrupt,
  sessionPermissionRespond,
  sessionResize,
  sessionSend,
  sessionCreate,
  sessionDelete,
  sessionPresence,
  sessionResume,
  surfaceSettingsGet,
  surfaceSettingsSet,
  toolPolicyGet,
  toolPolicySet,
  type PairingOutcome,
} from "./tauri";
import type { PeerRow, PendingPairing } from "../types/ipc";

function rustCommandFiles(root: string): string[] {
  return readdirSync(root, { withFileTypes: true })
    .flatMap((entry) => {
      const path = join(root, entry.name);
      if (entry.isDirectory()) return rustCommandFiles(path);
      return entry.isFile() && entry.name.endsWith(".rs") ? [path] : [];
    })
    .sort();
}

function maskRustComments(source: string): string {
  const characters = [...source];
  let lineComment = false;
  let blockCommentDepth = 0;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    const next = source[index + 1];
    if (lineComment) {
      if (character === "\n") lineComment = false;
      else characters[index] = " ";
      continue;
    }
    if (blockCommentDepth > 0) {
      if (character === "/" && next === "*") {
        characters[index] = " ";
        characters[index + 1] = " ";
        blockCommentDepth += 1;
        index += 1;
      } else if (character === "*" && next === "/") {
        characters[index] = " ";
        characters[index + 1] = " ";
        blockCommentDepth -= 1;
        index += 1;
      } else if (character !== "\n") {
        characters[index] = " ";
      }
      continue;
    }
    if (character === "/" && next === "/") {
      characters[index] = " ";
      characters[index + 1] = " ";
      lineComment = true;
      index += 1;
    } else if (character === "/" && next === "*") {
      characters[index] = " ";
      characters[index + 1] = " ";
      blockCommentDepth = 1;
      index += 1;
    }
  }
  return characters.join("");
}

function matchingParenthesis(source: string, openIndex: number): number {
  let depth = 0;
  let lineComment = false;
  let blockCommentDepth = 0;
  let quote: '"' | null = null;
  for (let index = openIndex; index < source.length; index += 1) {
    const character = source[index];
    const next = source[index + 1];
    if (lineComment) {
      if (character === "\n") lineComment = false;
      continue;
    }
    if (blockCommentDepth > 0) {
      if (character === "/" && next === "*") {
        blockCommentDepth += 1;
        index += 1;
      } else if (character === "*" && next === "/") {
        blockCommentDepth -= 1;
        index += 1;
      }
      continue;
    }
    if (quote !== null) {
      if (character === "\\") index += 1;
      else if (character === quote) quote = null;
      continue;
    }
    if (character === "/" && next === "/") {
      lineComment = true;
      index += 1;
      continue;
    }
    if (character === "/" && next === "*") {
      blockCommentDepth = 1;
      index += 1;
      continue;
    }
    if (character === '"') {
      quote = character;
      continue;
    }
    if (character === "(") depth += 1;
    else if (character === ")") {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  throw new Error(`Unclosed Rust command signature at character ${openIndex}.`);
}

function splitRustParameters(signature: string): string[] {
  const parameters: string[] = [];
  let start = 0;
  let angleDepth = 0;
  let parenDepth = 0;
  let bracketDepth = 0;
  let braceDepth = 0;
  let quote: '"' | null = null;
  for (let index = 0; index < signature.length; index += 1) {
    const character = signature[index];
    if (quote !== null) {
      if (character === "\\") index += 1;
      else if (character === quote) quote = null;
      continue;
    }
    if (character === '"') {
      quote = character;
      continue;
    }
    if (character === "<") angleDepth += 1;
    else if (character === ">") angleDepth -= 1;
    else if (character === "(") parenDepth += 1;
    else if (character === ")") parenDepth -= 1;
    else if (character === "[") bracketDepth += 1;
    else if (character === "]") bracketDepth -= 1;
    else if (character === "{") braceDepth += 1;
    else if (character === "}") braceDepth -= 1;
    else if (
      character === "," &&
      angleDepth === 0 &&
      parenDepth === 0 &&
      bracketDepth === 0 &&
      braceDepth === 0
    ) {
      const parameter = signature.slice(start, index).trim();
      if (parameter !== "") parameters.push(parameter);
      start = index + 1;
    }
  }
  const finalParameter = signature.slice(start).trim();
  if (finalParameter !== "") parameters.push(finalParameter);
  return parameters;
}

function rustParameterColon(parameter: string): number {
  let angleDepth = 0;
  let parenDepth = 0;
  let bracketDepth = 0;
  for (let index = 0; index < parameter.length; index += 1) {
    const character = parameter[index];
    if (character === "<") angleDepth += 1;
    else if (character === ">") angleDepth -= 1;
    else if (character === "(") parenDepth += 1;
    else if (character === ")") parenDepth -= 1;
    else if (character === "[") bracketDepth += 1;
    else if (character === "]") bracketDepth -= 1;
    else if (character === ":" && angleDepth === 0 && parenDepth === 0 && bracketDepth === 0) {
      return index;
    }
  }
  return -1;
}

function snakeToCamel(name: string): string {
  return name.replace(/_([a-z])/g, (_match, character: string) => character.toUpperCase());
}

function parseRustCommandArguments(): Record<string, readonly string[]> {
  const sourceRoot = resolve(process.cwd(), "src-tauri", "src");
  if (!existsSync(sourceRoot)) throw new Error(`Rust source root not found: ${sourceRoot}`);
  const commands: Record<string, readonly string[]> = {};
  for (const file of rustCommandFiles(sourceRoot)) {
    const source = readFileSync(file, "utf8");
    const searchableSource = maskRustComments(source);
    const attribute = /#\[tauri::command(?:\([^\]]*\))?\]/g;
    let match: RegExpExecArray | null;
    while ((match = attribute.exec(searchableSource)) !== null) {
      const declarationStart = match.index + match[0].length;
      const declaration = source.slice(declarationStart);
      const functionMatch = declaration.match(
        /^\s*(?:(?:pub)(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_]\w*)\s*\(/,
      );
      if (functionMatch === null) {
        throw new Error(`Could not parse Tauri command declaration in ${file}.`);
      }
      const openIndex = declaration.indexOf(
        "(",
        (functionMatch.index ?? 0) + functionMatch[0].length - 1,
      );
      if (openIndex === -1) throw new Error(`Missing parameter list in ${file}.`);
      const closeIndex = matchingParenthesis(declaration, openIndex);
      const rustArguments = splitRustParameters(declaration.slice(openIndex + 1, closeIndex))
        .map((parameter) => {
          const colon = rustParameterColon(parameter);
          if (colon === -1) throw new Error(`Could not parse parameter in ${file}: ${parameter}`);
          const name = parameter
            .slice(0, colon)
            .trim()
            .replace(/^mut\s+/, "");
          const type = parameter.slice(colon + 1).trim();
          return /\bState\s*</.test(type) || /\bAppHandle\b/.test(type) ? null : snakeToCamel(name);
        })
        .filter((name): name is string => name !== null);
      const commandName = functionMatch[1];
      if (commands[commandName] !== undefined) {
        throw new Error(`Duplicate Tauri command declaration: ${commandName}.`);
      }
      commands[commandName] = rustArguments;
    }
  }
  return commands;
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class Channel {},
}));

describe("isCommandError", () => {
  it("recognizes the JSON object Tauri rejects a Serialize error as", () => {
    const payload = {
      code: "session_not_found",
      message: "No session with that id.",
    };
    expect(isCommandError(payload)).toBe(true);
    if (isCommandError(payload)) {
      expect(payload.code).toBe("session_not_found");
      expect(payload.message).toBe("No session with that id.");
    }
  });

  it("recognizes an error with details", () => {
    const payload = {
      code: "session_generation_mismatch",
      message: "session generation is 3, client cursor is 1",
      details: { type: "generation_mismatch", current: 3, requested: 1 },
    };
    expect(isCommandError(payload)).toBe(true);
  });

  it("rejects strings and Error instances that old commands used to throw", () => {
    expect(isCommandError("No session with that id.")).toBe(false);
    expect(isCommandError(new Error("No session with that id."))).toBe(false);
    expect(isCommandError(null)).toBe(false);
    expect(isCommandError({ message: "missing code" })).toBe(false);
  });
});

describe("invokeTyped rejected payload", () => {
  it("surfaces a structured command error, not a string", async () => {
    const payload = {
      code: "session_not_found",
      message: "No session with that id.",
    };
    vi.mocked(invoke).mockRejectedValueOnce(payload);

    try {
      await invokeTyped("session_detach", { subscriptionId: 41 });
      throw new Error("expected invokeTyped to reject");
    } catch (error) {
      expect(error).toBe(payload);
      expect(isCommandError(error)).toBe(true);
      if (!isCommandError(error)) return;
      expect(error.code).toBe("session_not_found");
      expect(error.message).toBe("No session with that id.");
    }
  });
});

describe("retention command wrappers", () => {
  it("calls all four registered retention commands with typed payloads", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({} as never);
    await journalUsage();
    await journalRetentionGet();
    await journalRetentionSet({ maxAgeMs: 0 });
    await sessionDelete("s.owner.1");

    expect(invoke).toHaveBeenNthCalledWith(1, "journal_usage", undefined);
    expect(invoke).toHaveBeenNthCalledWith(2, "journal_retention_get", undefined);
    expect(invoke).toHaveBeenNthCalledWith(3, "journal_retention_set", {
      maxAgeMs: 0,
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "session_delete", { id: "s.owner.1" });
  });
});

describe("surface settings wrappers", () => {
  it("passes the surfaceId and opaque value payloads through", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(null as never);
    await surfaceSettingsGet("design");
    await surfaceSettingsSet("design", { split: true, count: 3 });

    expect(invoke).toHaveBeenNthCalledWith(1, "surface_settings_get", {
      surfaceId: "design",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "surface_settings_set", {
      surfaceId: "design",
      value: { split: true, count: 3 },
    });
  });

  it("maps a resolved null to the absent case", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(null as never);

    // The backend's Ok(None) serializes to null: genuinely absent file.
    await expect(surfaceSettingsGet("design")).resolves.toEqual({ status: "absent" });
  });

  it("maps a resolved document to the value case", async () => {
    vi.mocked(invoke).mockClear();
    const document = { version: 1, mode: "all", enabledSlugs: [] };
    vi.mocked(invoke).mockResolvedValue(document as never);

    await expect(surfaceSettingsGet("design")).resolves.toEqual({
      status: "value",
      value: document,
    });
  });

  it("maps a rejected read to the unreadable case carrying the message", async () => {
    vi.mocked(invoke).mockClear();
    // A structured CommandError is what Tauri rejects a Serialize error with.
    vi.mocked(invoke).mockRejectedValueOnce({
      code: "io_error",
      message: "settings file unreadable",
    });
    await expect(surfaceSettingsGet("design")).resolves.toEqual({
      status: "unreadable",
      message: "settings file unreadable",
    });

    // A plain Error rejection lands in the same case, message preserved.
    vi.mocked(invoke).mockRejectedValueOnce(new Error("bridge down"));
    await expect(surfaceSettingsGet("design")).resolves.toEqual({
      status: "unreadable",
      message: "bridge down",
    });
  });
});

describe("resume command wrapper", () => {
  it("passes the camelCase sessionId expected by Tauri v2", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ type: "not_supported" } as never);
    await sessionResume("s.owner.1");

    expect(invoke).toHaveBeenCalledWith("session_resume", { sessionId: "s.owner.1" });
  });
});

describe("create and attach command wrappers", () => {
  it("sends session_create with the camelCase workspaceId expected by Tauri v2", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ id: "s.1" } as never);
    await sessionCreate(null, "terminal");

    // Tauri v2 derives JS arg names from the Rust parameters: the daemon's
    // `workspace_id: Option<String>` must be addressed as `workspaceId`. The
    // snake_case spelling was silently coerced to None before this was fixed.
    expect(invoke).toHaveBeenCalledWith("session_create", {
      workspaceId: null,
      kind: "terminal",
      provider: null,
    });
  });

  it("sends session_attach with the camelCase fromCursor expected by Tauri v2", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(41 as never);
    const ch = {} as Channel;
    await expect(sessionAttach("s.owner.1", null, ch)).resolves.toBe(41);

    // Same convention: the daemon's `from_cursor: Option<u64>` is `fromCursor`
    // on the JS side.
    expect(invoke).toHaveBeenCalledWith("session_attach", {
      id: "s.owner.1",
      fromCursor: null,
      ch,
    });
  });

  it("puts the subscription id on every observer-specific command", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(undefined as never);

    await sessionSend("s.owner.1", 41, "hello");
    await sessionInterrupt("s.owner.1", 41);
    await sessionClaim(41);
    await sessionPermissionRespond("s.owner.1", 41, "tool-1", "allow_once");
    await sessionResize("s.owner.1", 41, 80, 24);
    await sessionDetach(41);

    expect(invoke).toHaveBeenNthCalledWith(1, "session_send", {
      id: "s.owner.1",
      subscriptionId: 41,
      text: "hello",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "session_interrupt", {
      id: "s.owner.1",
      subscriptionId: 41,
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "session_claim", { subscriptionId: 41 });
    expect(invoke).toHaveBeenNthCalledWith(4, "session_permission_respond", {
      id: "s.owner.1",
      subscriptionId: 41,
      requestId: "tool-1",
      outcome: "allow_once",
    });
    expect(invoke).toHaveBeenNthCalledWith(5, "session_resize", {
      id: "s.owner.1",
      subscriptionId: 41,
      cols: 80,
      rows: 24,
    });
    expect(invoke).toHaveBeenNthCalledWith(6, "session_detach", { subscriptionId: 41 });
  });

  it("passes the subscription id when closing a session", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(undefined as never);

    await sessionClose("s.owner.1", 41);

    expect(invoke).toHaveBeenCalledWith("session_close", {
      id: "s.owner.1",
      subscriptionId: 41,
    });
  });

  it("omits the subscription when closing a session that never had one", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(undefined as never);

    await sessionClose("s.owner.1");

    // The key must be absent, not present-and-undefined: a session created
    // before its attach never held a subscription, and the daemon closes it by
    // id alone.
    expect(invoke).toHaveBeenCalledWith("session_close", { id: "s.owner.1" });
    expect(vi.mocked(invoke).mock.calls[0]?.[1]).not.toHaveProperty("subscriptionId");
  });
});

describe("bridge wire-key convention", () => {
  it("keeps every argument key in the command map camelCase", () => {
    const offenders = Object.entries(COMMAND_ARG_KEYS).flatMap(([command, keys]) =>
      keys.filter((key) => key.includes("_")).map((key) => `${command}: ${key}`),
    );
    // The message is carried on the assertion itself, so it shows up exactly
    // when the guard fires. Tauri v2 derives JS arg names from the Rust
    // snake_case parameters: a snake_case key is either rejected as `invalid
    // args` or silently coerced to None.
    expect(
      offenders,
      "Snake_case argument keys found in the Tauri bridge. Tauri v2 exposes Rust " +
        "snake_case parameters to JavaScript as camelCase, so a snake_case key is " +
        "either rejected as `invalid args` or silently coerced to None. Rename the " +
        "key in `CommandArgs` (src/lib/tauri.ts), in `COMMAND_ARG_KEYS`, in its " +
        "exported wrapper, and in every internal shim that builds the argument " +
        "object. Command NAMES stay snake_case; only argument keys are camelCase.\n" +
        offenders.map((line) => `  - ${line}`).join("\n"),
    ).toEqual([]);
  });

  it("matches every command key to its Rust Tauri command signature", () => {
    const expected = Object.fromEntries(
      Object.entries(COMMAND_ARG_KEYS).map(([command, keys]) => [command, [...keys]]),
    );
    const actual = parseRustCommandArguments();
    const missing = Object.keys(expected).filter((command) => actual[command] === undefined);
    const extra = Object.keys(actual).filter((command) => expected[command] === undefined);
    const mismatched = Object.keys(expected)
      .filter((command) => actual[command] !== undefined)
      .filter((command) => JSON.stringify(actual[command]) !== JSON.stringify(expected[command]))
      .map((command) => ({
        command,
        expected: expected[command],
        actual: actual[command],
      }));

    expect(
      { missing, extra, mismatched },
      "TypeScript Tauri argument keys must match the Rust command signatures. State and " +
        "AppHandle parameters are injected by Tauri and intentionally omitted.",
    ).toEqual({ missing: [], extra: [], mismatched: [] });
  });
});

describe("presence command wrapper", () => {
  it("passes the camelCase keys expected by Tauri v2", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(undefined as never);
    await sessionPresence("session-a", true);
    await sessionPresence(null, false);

    expect(invoke).toHaveBeenNthCalledWith(1, "session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });
  });
});

describe("provider refresh wrapper", () => {
  it("calls the providers_refresh command with no payload", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ providers: [], unreadableDirs: 0 } as never);
    await providersRefresh();

    expect(invoke).toHaveBeenCalledWith("providers_refresh", undefined);
  });
});

describe("folder-scoped Oracle wrappers", () => {
  it("sends only the folder path to oracle_folder_status", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({
      path: "/abs/folder",
      data_dir: "/abs/folder/oracle-data",
      state: "never_indexed",
      indexed_files: 0,
      total_files: 0,
      pending_files: 0,
      stale_files: 0,
      indexed_chunks: 0,
      message: "Oracle has no index for this folder yet.",
    } as never);

    const status = await oracleFolderStatus("/abs/folder");

    expect(invoke).toHaveBeenCalledWith("oracle_folder_status", { path: "/abs/folder" });
    // Exactly one key: the command has no runtime argument on the wire, which
    // is why it cannot switch the active root.
    expect(vi.mocked(invoke).mock.calls[0]?.[1]).toEqual({ path: "/abs/folder" });
    expect(status.state).toBe("never_indexed");
  });

  it("keeps the query key present even when it is an empty string", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ query: "", results: [] } as never);

    await oracleAskFolder("/abs/folder", "");

    // Tauri v2 drops a key only when it is absent from the payload, so an
    // empty string must still travel: the backend's own validation is what
    // rejects it, not a silently missing argument.
    expect(vi.mocked(invoke).mock.calls[0]?.[1]).toHaveProperty("query", "");
    expect(vi.mocked(invoke).mock.calls[0]?.[1]).toEqual({ path: "/abs/folder", query: "" });
  });

  it("declares both folder commands in the bridge manifest", () => {
    // The structural parity test below compares these with the Rust
    // signatures; this pins the exact keys so a rename cannot pass unnoticed.
    expect(COMMAND_ARG_KEYS.oracle_folder_status).toEqual(["path"]);
    expect(COMMAND_ARG_KEYS.oracle_ask_folder).toEqual(["path", "query"]);
  });
});

const PAIRED_PEER: PeerRow = {
  deviceId: "9f6b0f2e-6f1c-4a1e-9c62-1e2f7d59a9c3",
  displayName: "Xiaomi 14",
  role: "client",
  publicKey: "cHVibGljLWtleQ==",
  keyFingerprint: "0a1b2c3d4e5f60718293a4b5c6d7e8f9",
  bindingKind: "tailnet",
  bindingNodeName: "xiaomi-14.tail80a42d.ts.net.",
  bindingLoginName: "user@example.com",
  address: "100.74.116.126:47831",
  pairedAt: 1_760_000_000_000,
  revokedAt: null,
  caps: ["view", "send"],
  pairedByUser: "S-1-5-21-1004336348-1177238915-682003330-1001",
  online: true,
};

const WAITING_PAIRING: PendingPairing = {
  deviceId: "3ac1f0de-4b5a-4c3d-8e9f-0a1b2c3d4e5f",
  displayName: "Marco's MacBook Pro",
  role: "daemon",
  keyFingerprint: "f9e8d7c6b5a4938271605f4e3d2c1b0a",
  address: "100.74.116.126:47831",
  expiresAt: 1_760_000_060_000,
};

describe("pairing outcome narrowing", () => {
  // The whole point of `PairingOutcome` is that the two replies are told apart
  // by their wire tag, not by which fields look present. This helper only
  // compiles while the union stays discriminated; an added variant fails the
  // `never` assignment below instead of silently falling through.
  function pairingLine(outcome: PairingOutcome): string {
    switch (outcome.type) {
      case "pairing_pending":
        return `waiting for ${outcome.peer.displayName} to confirm`;
      case "pairing_done":
        return `paired ${outcome.peer.displayName}`;
      default: {
        const unhandled: never = outcome;
        return `unknown reply ${String(unhandled)}`;
      }
    }
  }

  it("reads the pending reply as a pending pairing", () => {
    expect(pairingLine({ type: "pairing_pending", peer: WAITING_PAIRING })).toBe(
      "waiting for Marco's MacBook Pro to confirm",
    );
  });

  it("reads the done reply as a written peer row", () => {
    expect(pairingLine({ type: "pairing_done", peer: PAIRED_PEER })).toBe("paired Xiaomi 14");
  });

  it("resolves pairing_complete with the reply the daemon sent", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ type: "pairing_done", peer: PAIRED_PEER } as never);

    const outcome = await pairingComplete("100.74.116.126:47831", "ABCD2345", "client");

    expect(invoke).toHaveBeenCalledWith("pairing_complete", {
      address: "100.74.116.126:47831",
      code: "ABCD2345",
      role: "client",
    });
    expect(outcome.type).toBe("pairing_done");
    if (outcome.type !== "pairing_done") throw new Error("narrowing failed");
    expect(outcome.peer.deviceId).toBe(PAIRED_PEER.deviceId);
  });
});

describe("device command wrappers", () => {
  it("sends each argument as the camelCase key Tauri v2 expects", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({} as never);

    await devicesList();
    await pairingStart("daemon");
    await pairingConfirm("device-1", true);
    await peerRevoke("device-1");
    await peerSetCaps("device-1", ["view", "send", "answer_permissions"]);

    expect(invoke).toHaveBeenNthCalledWith(1, "devices_list", undefined);
    expect(invoke).toHaveBeenNthCalledWith(2, "pairing_start", { role: "daemon" });
    expect(invoke).toHaveBeenNthCalledWith(3, "pairing_confirm", {
      deviceId: "device-1",
      accept: true,
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "peer_revoke", { deviceId: "device-1" });
    expect(invoke).toHaveBeenNthCalledWith(5, "peer_set_caps", {
      deviceId: "device-1",
      caps: ["view", "send", "answer_permissions"],
    });
  });

  it("carries a declined pairing through as null, not as a rejection", async () => {
    vi.mocked(invoke).mockClear();
    // The daemon's `pairing_declined` reply is a success: Tauri hands the
    // Option's None back as null, and the wrapper must not turn that into an
    // error the panel would show as a failed decline.
    vi.mocked(invoke).mockResolvedValue(null as never);

    await expect(pairingConfirm("device-1", false)).resolves.toBeNull();
    expect(invoke).toHaveBeenCalledWith("pairing_confirm", {
      deviceId: "device-1",
      accept: false,
    });
  });

  it("sends the full grant array, never a delta", () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(PAIRED_PEER as never);
    const caps = ["view"] as const;
    void peerSetCaps("device-1", caps);

    const payload = vi.mocked(invoke).mock.calls[0]?.[1] as { caps: string[] };
    // The daemon stores the array it receives, so dropping `view` here would
    // silently revoke it.
    expect(payload.caps).toEqual(["view"]);
  });

  it("pins the wire keys of every device command", () => {
    expect(COMMAND_ARG_KEYS.devices_list).toEqual([]);
    expect(COMMAND_ARG_KEYS.pairing_start).toEqual(["role"]);
    expect(COMMAND_ARG_KEYS.pairing_complete).toEqual(["address", "code", "role"]);
    expect(COMMAND_ARG_KEYS.pairing_confirm).toEqual(["deviceId", "accept"]);
    expect(COMMAND_ARG_KEYS.peer_revoke).toEqual(["deviceId"]);
    expect(COMMAND_ARG_KEYS.peer_set_caps).toEqual(["deviceId", "caps"]);
  });
});

describe("tool policy command wrappers", () => {
  it("calls tool_policy_get with no payload", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ policies: [] } as never);

    await expect(toolPolicyGet()).resolves.toEqual({ policies: [] });
    expect(invoke).toHaveBeenCalledWith("tool_policy_get", undefined);
  });

  it("sends the full policy row, with null meaning enabled", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(undefined as never);

    await toolPolicySet("grok", null, ["some_tool"]);
    await toolPolicySet("grok", false, []);

    // The daemon stores the array it receives, so the deny list is always
    // the complete set, never a delta. `null` is enabled (matches the
    // daemon's absent-means-enabled rule); `false` disables every tool.
    expect(invoke).toHaveBeenNthCalledWith(1, "tool_policy_set", {
      providerId: "grok",
      enabled: null,
      disabledTools: ["some_tool"],
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "tool_policy_set", {
      providerId: "grok",
      enabled: false,
      disabledTools: [],
    });
  });

  it("pins the wire keys of both tool policy commands", () => {
    expect(COMMAND_ARG_KEYS.tool_policy_get).toEqual([]);
    expect(COMMAND_ARG_KEYS.tool_policy_set).toEqual(["providerId", "enabled", "disabledTools"]);
  });
});
