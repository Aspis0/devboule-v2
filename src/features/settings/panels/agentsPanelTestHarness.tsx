// Mount lifecycle, fixtures and field drivers shared by the Agents panel
// test files. Each file still installs the module mock itself
// (`agentsPanelTestMocks.ts`): vi.mock is hoisted per file.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import {
  agentProfilesGet,
  agentProfilesSet,
  daemonStatus,
  providerVocabularyGet,
  providersList,
} from "../../../lib/tauri";
import type {
  AgentProfile,
  AgentProfilesDocument,
  AgentProfilesReply,
  DaemonStatus,
  ProviderInfo,
  ProviderVocabulary,
  VocabularyFeature,
} from "../../../types/ipc";
import { AgentProfilesPanel } from "./AgentsPanel";
import { DEFAULT_CAPABILITIES } from "./agentsPanelTestMocks";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** The mounted test surface; tests read and assign it directly. */
export const dom: { container: HTMLDivElement; root: Root | undefined } = {
  container: undefined as unknown as HTMLDivElement,
  root: undefined,
};

/**
 * Installs the per-test mount and the mock drain. `providers` is what the
 * catalog answers by default.
 */
export function useAgentsPanelDom(providers: () => ProviderInfo[]) {
  beforeEach(() => {
    dom.container = document.createElement("div");
    document.body.appendChild(dom.container);
    dom.root = undefined;
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: providers(),
      unreadableDirs: 0,
    }));
  });

  afterEach(async () => {
    if (dom.root !== undefined) await act(async () => dom.root!.unmount());
    dom.root = undefined;
    dom.container.remove();
    vi.clearAllMocks();
    // `clearAllMocks` keeps queued `mockImplementationOnce` entries: a test
    // that failed before consuming a queued write or read-back would leak it
    // into the next test's click or initial load.
    vi.mocked(agentProfilesSet).mockReset();
    vi.mocked(agentProfilesSet).mockImplementation(async () => undefined);
    vi.mocked(providerVocabularyGet).mockReset();
    // Tests arm the handshake with a sticky `mockResolvedValue`; back to the
    // module default so the next test meets the daemon it expects.
    vi.mocked(daemonStatus).mockReset();
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(DEFAULT_CAPABILITIES));
    vi.mocked(agentProfilesGet).mockReset();
    vi.mocked(agentProfilesGet).mockImplementation(async () => ({
      document: { profiles: [], standingInstructions: "" },
    }));
  });
}

/** The handshake of every daemon shipping today: no `provider_vocabulary`. */
export const OLDER_DAEMON = [
  "ping",
  "status",
  "sessions",
  "journal",
  "typed_permissions",
  "devices",
  "agent_profiles",
];
export const VOCABULARY_DAEMON = [...OLDER_DAEMON, "provider_vocabulary"];

/** A connected supervisor status whose capability list is the handshake's. */
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

export function makeProfile(overrides: Partial<AgentProfile> = {}): AgentProfile {
  return {
    id: "profile-1",
    name: "Explorer",
    icon: null,
    note: "Reads the code and reports back.",
    provider: "grok",
    model: "grok-4",
    modeId: "ask",
    thinkingOptionId: null,
    features: {},
    toolOverlay: [],
    enabledForAgents: false,
    ...overrides,
  };
}

export function makeProvider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "claude",
    executable: "C:\\cli\\claude.cmd",
    acpAvailable: false,
    authentication: "ok",
    protocol: "stream-json",
    origin: "user-binary",
    installed: true,
    ...overrides,
  };
}

/** The tick row every agent family's answer carries, as the daemon answers it. */
export function autoAcceptRow(): VocabularyFeature {
  return { id: "autoAccept", label: "Auto accept", author: "daemon", type: "toggle" };
}

export function makeVocabulary(overrides: Partial<ProviderVocabulary> = {}): ProviderVocabulary {
  return {
    provider: "claude",
    models: { state: "absent", items: [] },
    modes: { state: "absent", items: [] },
    // By default an answer carries the tick every agent family offers. Pass
    // `features: undefined` for a daemon older than the field.
    features: { state: "present", items: [autoAcceptRow()] },
    source: "probe",
    probedAtMs: null,
    ...overrides,
  };
}

/** A stored row as the daemon reads it back after the form created one. */
export function storedProfile(id: string, overrides: Partial<AgentProfile> = {}): AgentProfile {
  return {
    id,
    name: "Gamma",
    icon: null,
    note: "",
    provider: "claude",
    model: "claude-sonnet-4-5",
    modeId: "default",
    thinkingOptionId: null,
    features: {},
    toolOverlay: [],
    enabledForAgents: false,
    ...overrides,
  };
}

async function mountPanel() {
  dom.root = createRoot(dom.container);
  await act(async () => dom.root!.render(<AgentProfilesPanel />));
  await act(async () => undefined);
  await act(async () => undefined);
}

// The capability mock is `mockResolvedValue`, not `Once`: the panel and the
// delegation switch each poll the handshake.
export async function renderAgentsPanel(
  doc: AgentProfilesDocument,
  capabilities: string[] = OLDER_DAEMON,
) {
  vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(capabilities));
  vi.mocked(agentProfilesGet).mockResolvedValueOnce({ document: doc });
  await mountPanel();
}

/** The document fetch never answers: the loading lock's state. */
export async function renderAgentsPanelLoading() {
  vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(OLDER_DAEMON));
  vi.mocked(agentProfilesGet).mockImplementationOnce(
    () => new Promise<AgentProfilesReply>(() => undefined),
  );
  await mountPanel();
}

/** The document fetch rejects: the failed load's state. */
export async function renderAgentsPanelErrored() {
  vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(OLDER_DAEMON));
  vi.mocked(agentProfilesGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
  await mountPanel();
}

/** What a field's `aria-describedby` points at, joined in the order it lists. */
export function describedText(field: Element): string {
  const ids = (field.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean);
  expect(ids.length).toBeGreaterThan(0);
  return ids.map((id) => dom.container.querySelector(`[id="${id}"]`)?.textContent ?? "").join(" ");
}

// The suite has no testing-library fireEvent. Inputs and textareas take the
// rendered onChange through their __reactProps key; a select carries no such
// key under React 19 (its onChange rides the native change event), so it is
// driven by setting the value and dispatching that event.
export async function typeText(
  fieldElement: HTMLTextAreaElement | HTMLInputElement | HTMLSelectElement,
  value: string,
) {
  if ((fieldElement as HTMLSelectElement).tagName === "SELECT") {
    await act(async () => {
      fieldElement.value = value;
      fieldElement.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => undefined);
    return;
  }
  const reactKey = Object.keys(fieldElement).find((key) => key.startsWith("__reactProps"));
  const props = (fieldElement as unknown as Record<string, unknown>)[reactKey ?? ""] as
    | { onChange?: (event: { target: { value: string } }) => void }
    | undefined;
  if (!props?.onChange) throw new Error("field onChange did not render");
  await act(async () => {
    props.onChange?.({ target: { value } });
  });
  await act(async () => undefined);
}

// A raw `.click()` on a remounted form's checkbox loses the synthetic change
// to a happy-dom/React quirk (the DOM ticks, the state does not), so the
// handler is driven directly.
export async function tickCheckbox(box: HTMLInputElement, next: boolean) {
  const reactKey = Object.keys(box).find((key) => key.startsWith("__reactProps"));
  const props = (box as unknown as Record<string, unknown>)[reactKey ?? ""] as
    | { onChange?: (event: { target: { checked: boolean } }) => void }
    | undefined;
  if (!props?.onChange) throw new Error("checkbox onChange did not render");
  await act(async () => {
    props.onChange?.({ target: { checked: next } });
  });
  await act(async () => undefined);
}
