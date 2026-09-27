// Per-test reset and fixtures shared by the Providers panel test files.

import { beforeEach, vi } from "vitest";

import {
  daemonDiagnostics,
  daemonStatus,
  providerSetEnabled,
  providerUpdate,
  providersAuthCheck,
  providersList,
  providersRefresh,
  toolPolicyGet,
  toolPolicySet,
} from "../../../lib/tauri";
import type { DaemonStatus, ProviderInfo } from "../../../types/ipc";
import { providerDefaults } from "./providersPanelTestMocks";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/**
 * Every test starts from the defaults, for every mock: `vi.clearAllMocks()`
 * keeps implementations, so a sticky mock from an earlier test would make
 * the file's green an artifact of declaration order.
 */
export function installProvidersPanelMockReset() {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(daemonStatus).mockImplementation(async () => providerDefaults.daemonStatus());
    vi.mocked(providersList).mockImplementation(async () => providerDefaults.providers());
    vi.mocked(providersRefresh).mockImplementation(async () => providerDefaults.providers());
    vi.mocked(providersAuthCheck).mockImplementation(async () => providerDefaults.providers());
    vi.mocked(providerUpdate).mockImplementation(async () => providerDefaults.providerUpdate());
    vi.mocked(providerSetEnabled).mockImplementation(async () => undefined);
    vi.mocked(daemonDiagnostics).mockImplementation(async () =>
      providerDefaults.daemonDiagnostics(),
    );
    vi.mocked(toolPolicyGet).mockImplementation(async () => providerDefaults.toolPolicies());
    vi.mocked(toolPolicySet).mockImplementation(async () => undefined);
    // providerVocabularyGet deliberately keeps no default answer.
  });
}

export function daemonStatusWith(capabilities: string[]): DaemonStatus {
  return {
    state: "connected",
    pid: 1,
    instanceId: "settings-test",
    protocolVersion: 4,
    clients: 1,
    capabilities,
    message: null,
  };
}

export function installedProvider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "grok",
    executable: "C:\\npm\\grok.cmd",
    acpAvailable: true,
    authentication: "unknown",
    protocol: "acp",
    ...overrides,
  };
}
