// Module mocks the Providers panel test files install: `lib/tauri` answers
// from the defaults below, and the workspace session controller is a stub the
// terminal tests drive. Each file still calls vi.mock itself (hoisted per file).

import { vi } from "vitest";

import type * as Tauri from "../../../lib/tauri";
import type * as WorkspaceSessions from "../../workspace/workspaceSessions";
import type {
  DaemonDiagnostics,
  DaemonStatus,
  ProviderCatalog,
  ProviderUpdateOutcome,
} from "../../../types/ipc";

export const providerDefaults = {
  daemonStatus: (): DaemonStatus => ({
    state: "connected",
    pid: 1,
    instanceId: "settings-test",
    protocolVersion: 4,
    clients: 1,
    capabilities: [
      "ping",
      "status",
      "sessions",
      "journal",
      "typed_permissions",
      "devices",
      "tool_policy",
      "provider.switches",
      "provider.auth-check",
    ],
    message: null,
  }),
  providers: (): ProviderCatalog => ({ providers: [], unreadableDirs: 0 }),
  providerUpdate: (): ProviderUpdateOutcome => ({ ok: true, exitCode: 0, log: "" }),
  daemonDiagnostics: (): DaemonDiagnostics =>
    ({
      environment: { osVersion: "Windows 10.0.26200 (x86_64)" },
    }) as DaemonDiagnostics,
  toolPolicies: () => ({ policies: [] }),
};

export const sessionMocks = {
  create: vi.fn(),
  creating: false,
  error: null as null | { sentence: string; detail: string | null; workspaceId: string | null },
};

export function providersTauriMock(actual: typeof Tauri) {
  return {
    ...actual,
    daemonStatus: vi.fn(async () => providerDefaults.daemonStatus()),
    providersList: vi.fn(async () => providerDefaults.providers()),
    providersRefresh: vi.fn(async () => providerDefaults.providers()),
    providersAuthCheck: vi.fn(async () => providerDefaults.providers()),
    providerUpdate: vi.fn(async () => providerDefaults.providerUpdate()),
    providerSetEnabled: vi.fn(async () => undefined),
    daemonDiagnostics: vi.fn(async () => providerDefaults.daemonDiagnostics()),
    toolPolicyGet: vi.fn(async () => providerDefaults.toolPolicies()),
    toolPolicySet: vi.fn(async () => undefined),
    // No default answer: a vocabulary query only ever leaves the app for an
    // expanded row when the handshake advertised `provider_vocabulary`.
    providerVocabularyGet: vi.fn(),
  };
}

export function workspaceSessionsMock(actual: typeof WorkspaceSessions) {
  return {
    ...actual,
    sharedSessionController: () => ({
      create: sessionMocks.create,
      getState: () => ({ creating: sessionMocks.creating, error: sessionMocks.error }),
    }),
  };
}
