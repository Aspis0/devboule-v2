// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return {
    ...actual,
    isCommandError: vi.fn(
      (error: unknown) =>
        typeof error === "object" && error !== null && "code" in error && "message" in error,
    ),
    daemonStatus: vi.fn(async () => ({
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
      ],
      message: null,
    })),
    journalRetentionGet: vi.fn(),
    journalRetentionSet: vi.fn(),
    journalUsage: vi.fn(),
    // The General panel's close-behavior and notification-sound rows read
    // and write their own surface settings.
    surfaceSettingsGet: vi.fn(async () => ({ status: "absent" })),
    surfaceSettingsSet: vi.fn(async () => undefined),
    projectAdd: vi.fn(),
    projectsList: vi.fn(async () => []),
    providersList: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providersRefresh: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providerUpdate: vi.fn(async () => ({ ok: true, exitCode: 0, log: "" })),
    toolPolicyGet: vi.fn(async () => ({ policies: [] })),
    toolPolicySet: vi.fn(async () => undefined),
    agentProfilesGet: vi.fn(async () => ({
      document: {
        profiles: [],
        standingInstructions: "",
      } as AgentProfilesDocument,
    })),
    agentProfilesSet: vi.fn(async () => undefined),
    // No default answer: a vocabulary query only ever leaves the app when the
    // handshake advertised `provider_vocabulary`, and the tests that arm it
    // queue their own replies.
    providerVocabularyGet: vi.fn(),
    // The delegation pair: never called unless the handshake advertised
    // `permission_delegation`, and every test arms its own replies.
    delegationGet: vi.fn(),
    delegationSet: vi.fn(async () => undefined),
    workspacesList: vi.fn(async () => []),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("../../oracle/OraclePanel", () => ({
  OraclePanel: () => <div>Oracle mock</div>,
}));

import {
  agentProfilesGet,
  agentProfilesSet,
  daemonStatus,
  providerVocabularyGet,
  providersList,
} from "../../../lib/tauri";
import type {
  AgentProfile,
  VocabularyFeature,
  AgentProfilesDocument,
  AgentProfilesReply,
  DaemonStatus,
  ProviderInfo,
  ProviderVocabulary,
} from "../../../types/ipc";
import { SettingsSurface } from "../SettingsSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
describe("Settings agents panel", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  function makeProfile(overrides: Partial<AgentProfile> = {}): AgentProfile {
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

  /** A connected supervisor status whose capability list is the handshake's. */
  function daemonStatusWith(capabilities: string[]): DaemonStatus {
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

  // Renders the surface, waits for the handshake, opens the Agents tab and
  // answers the document fetch, so assertions see the settled list. The
  // capability mock is `mockResolvedValue`, not `Once`: the hook polls per
  // consumer, so ProvidersPanel (the default tab) consumes a one-shot answer
  // before the Agents tab ever mounts.
  async function renderAgentsPanel(doc: AgentProfilesDocument) {
    vi.mocked(daemonStatus).mockResolvedValue(
      daemonStatusWith([
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "agent_profiles",
      ]),
    );
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({ document: doc });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);
  }

  /** Same, but the document fetch never answers: the loading lock's state. */
  async function renderAgentsPanelLoading() {
    vi.mocked(daemonStatus).mockResolvedValue(
      daemonStatusWith([
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "agent_profiles",
      ]),
    );
    vi.mocked(agentProfilesGet).mockImplementationOnce(
      () => new Promise<AgentProfilesReply>(() => undefined),
    );
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);
  }

  /** Same, but the document fetch rejects: the failed load's state. */
  async function renderAgentsPanelErrored() {
    vi.mocked(daemonStatus).mockResolvedValue(
      daemonStatusWith([
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "agent_profiles",
      ]),
    );
    vi.mocked(agentProfilesGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);
  }

  function profileRows(): HTMLElement[] {
    return Array.from(container.querySelectorAll<HTMLElement>(".agent-profile-row"));
  }

  function rowByName(name: string): HTMLElement {
    const row = profileRows().find((row) => row.textContent?.includes(name));
    if (!row) throw new Error(`profile row ${name} did not render`);
    return row;
  }

  /** The one checkbox in a profile row is its "agents may create this" tick. */
  function tickBox(name: string): HTMLInputElement {
    const box = rowByName(name).querySelector<HTMLInputElement>("input[type='checkbox']");
    if (!box) throw new Error(`tick for ${name} did not render`);
    return box;
  }

  function rowButton(name: string, text: string): HTMLButtonElement {
    const button = Array.from(rowByName(name).querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === text,
    );
    if (!button) throw new Error(`button ${text} on ${name} did not render`);
    return button;
  }

  function sectionButton(text: string): HTMLButtonElement {
    const button = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".agent-profiles button"),
    ).find((candidate) => candidate.textContent === text);
    if (!button) throw new Error(`button ${text} did not render`);
    return button;
  }

  /** What a field's `aria-describedby` points at, joined in the order it lists. */
  function describedText(field: Element): string {
    const ids = (field.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean);
    expect(ids.length).toBeGreaterThan(0);
    return ids.map((id) => container.querySelector(`[id="${id}"]`)?.textContent ?? "").join(" ");
  }

  // Drives a controlled React field directly (the suite's raw createRoot/act
  // style has no testing-library fireEvent): calls the rendered onChange with
  // the value a paste would leave in the field.
  async function typeText(field: HTMLTextAreaElement | HTMLInputElement, value: string) {
    const reactKey = Object.keys(field).find((key) => key.startsWith("__reactProps"));
    const props = (field as unknown as Record<string, unknown>)[reactKey ?? ""] as
      | { onChange?: (event: { target: { value: string } }) => void }
      | undefined;
    if (!props?.onChange) throw new Error("field onChange did not render");
    await act(async () => {
      props.onChange?.({ target: { value } });
    });
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [],
      unreadableDirs: 0,
    }));
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root!.unmount());
    container.remove();
    vi.clearAllMocks();
    // `clearAllMocks` keeps queued `mockImplementationOnce` entries, so a test
    // that queued an unsettled write and failed before consuming it would
    // leave the next test's click reading a hanging write. Reset back to the
    // resolved default, exactly as the tool-toggles block does for its write.
    vi.mocked(agentProfilesSet).mockReset();
    vi.mocked(agentProfilesSet).mockImplementation(async () => undefined);
  });

  it("hides the section and never fetches when the daemon lacks agent_profiles", async () => {
    // The module mock's default daemonStatus advertises tool_policy but not
    // agent_profiles: an older daemon. The tab still navigates; the section
    // is absent, not disabled and not an error.
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);

    expect(agentProfilesGet).not.toHaveBeenCalled();
    expect(container.querySelector(".agent-profiles")).toBeNull();
    expect(container.textContent).not.toContain("Agents may create this");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("locks every control while the document is in flight", async () => {
    await renderAgentsPanelLoading();

    expect(container.textContent).toContain("Loading agent profiles…");
    const controls = container.querySelectorAll<HTMLInputElement | HTMLButtonElement>(
      ".agent-profiles input, .agent-profiles textarea, .agent-profiles button",
    );
    expect(controls.length).toBeGreaterThan(0);
    for (const control of controls) expect(control.disabled).toBe(true);
    expect(agentProfilesSet).not.toHaveBeenCalled();
  });

  it("renders the profiles in the order the daemon returned", async () => {
    // Deliberately not alphabetical: the human's order is the feature.
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "b", name: "Beta" }), makeProfile({ id: "a", name: "Alpha" })],
      standingInstructions: "",
    });

    expect(
      profileRows().map((row) => row.querySelector(".settings-card-title")?.textContent),
    ).toEqual(["Beta", "Alpha"]);
    expect(rowByName("Beta").querySelector<HTMLElement>(".agent-profile-meta")?.textContent).toBe(
      "grok · grok-4 · mode ask",
    );
    // The edges are where a reorder bug would show: the first row cannot move
    // up and the last cannot move down.
    const firstUp = rowByName("Beta").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Beta up']",
    );
    const lastDown = rowByName("Alpha").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Alpha down']",
    );
    expect(firstUp?.disabled).toBe(true);
    expect(lastDown?.disabled).toBe(true);
  });

  it("ticking agents-may-create writes exactly that flag and nothing else", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: the tick is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), enabledForAgents: true }],
        standingInstructions: "",
      },
    });

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    // Deep equality on the whole document: provider, model, mode, features,
    // note, order and the standing instructions travel untouched — only the
    // tick flipped.
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), enabledForAgents: true }],
      standingInstructions: "",
    });
    expect(tickBox("Explorer").checked).toBe(true);
  });

  it("reverts the tick and shows the daemon sentence verbatim on a failed write", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(tickBox("Explorer").checked).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("shows the off-switch sentence once the last ticked profile is untoggled", async () => {
    await renderAgentsPanel({
      profiles: [
        makeProfile({ enabledForAgents: true }),
        makeProfile({ id: "profile-2", name: "Coder" }),
      ],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: both rows, untoggled.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
        standingInstructions: "",
      },
    });

    // One ticked: the door is open, the sentence must be absent.
    expect(container.textContent).not.toContain("agents cannot start agents");

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    // The last tick is gone, so the section reads as the off switch it is.
    expect(container.textContent).toContain("agents cannot start agents");
    expect(tickBox("Explorer").checked).toBe(false);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("moving a profile up sends the reordered document and re-renders in that order", async () => {
    const beta = makeProfile({ id: "b", name: "Beta" });
    const alpha = makeProfile({ id: "a", name: "Alpha" });
    await renderAgentsPanel({ profiles: [beta, alpha], standingInstructions: "" });
    // The store's read-back after the confirmed write: the new order.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [alpha, beta], standingInstructions: "" },
    });

    const up = rowByName("Alpha").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Alpha up']",
    );
    if (!up) throw new Error("move-up button did not render");
    await act(async () => up.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [alpha, beta],
      standingInstructions: "",
    });
    expect(
      profileRows().map((row) => row.querySelector(".settings-card-title")?.textContent),
    ).toEqual(["Alpha", "Beta"]);
  });

  it("deletes only the armed profile, and only after the inline confirm", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed delete: one row left.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [makeProfile({ id: "profile-2", name: "Coder" })],
        standingInstructions: "",
      },
    });

    // The first click arms the row and sends nothing.
    await act(async () => rowButton("Explorer", "Delete").click());
    await act(async () => undefined);
    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(container.textContent).toContain("Deletes this profile");

    await act(async () => sectionButton("Delete now").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles.map((profile) => profile.name)).toEqual(["Coder"]);
    expect(
      profileRows().map((row) => row.querySelector(".settings-card-title")?.textContent),
    ).toEqual(["Coder"]);
  });

  it("saves a rename and note while every other field travels untouched", async () => {
    const explorer = makeProfile();
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save: the rename stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...explorer, name: "Scout", note: "Maps the work before anyone builds." }],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    const nameField = editor.querySelector<HTMLInputElement>("input");
    const noteField = editor.querySelector<HTMLTextAreaElement>("textarea");
    if (!nameField || !noteField) throw new Error("editor fields did not render");
    await typeText(nameField, "Scout");
    await typeText(noteField, "Maps the work before anyone builds.");

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...explorer, name: "Scout", note: "Maps the work before anyone builds." }],
      standingInstructions: "",
    });
  });

  it("edits every field of a profile, the spawn prompt included, and saves them all", async () => {
    const explorer = makeProfile();
    // One installed provider, and no `provider_vocabulary` in the handshake:
    // the model and mode fall back to free text, and the editor must still
    // finish — the same sentence the create form offers.
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "grok",
          executable: "C:\\cli\\grok.cmd",
          acpAvailable: true,
          authentication: "ok",
          protocol: "acp",
          origin: "user-binary",
          installed: true,
        },
      ],
      unreadableDirs: 0,
    });
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save, carrying every edit.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          {
            ...explorer,
            name: "Scout",
            note: "Maps the work before anyone builds.",
            spawnPrompt: "Check the diff before you report.",
            model: "grok-4-fast",
            modeId: "reflect",
            thinkingOptionId: "high",
            features: { autoAccept: true },
          },
        ],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    const pick = <T extends HTMLElement>(selector: string): T => {
      const element = editor.querySelector<T>(selector);
      if (!element) throw new Error(`field ${selector} did not render`);
      return element;
    };
    await typeText(pick<HTMLInputElement>("input"), "Scout");
    await typeText(
      pick<HTMLTextAreaElement>('textarea[aria-label="Profile note"]'),
      "Maps the work before anyone builds.",
    );
    await typeText(
      pick<HTMLTextAreaElement>('textarea[aria-label="Profile spawn prompt"]'),
      "Check the diff before you report.",
    );
    await typeText(pick<HTMLInputElement>('[aria-label="Model"]'), "grok-4-fast");
    await typeText(pick<HTMLInputElement>('[aria-label="Mode"]'), "reflect");
    await typeText(pick<HTMLInputElement>('[aria-label="Thinking option"]'), "high");
    await act(async () =>
      pick<HTMLInputElement>(
        'input[aria-label="Auto accept for children of this profile"]',
      ).click(),
    );

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [
        {
          ...explorer,
          name: "Scout",
          note: "Maps the work before anyone builds.",
          spawnPrompt: "Check the diff before you report.",
          model: "grok-4-fast",
          modeId: "reflect",
          thinkingOptionId: "high",
          features: { autoAccept: true },
        },
      ],
      standingInstructions: "",
    });
  });

  it("round-trips the idle-close timer: the default 30, a custom value, and off", async () => {
    const explorer = makeProfile();
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    const idleField = (): HTMLInputElement => {
      const field = container.querySelector<HTMLInputElement>(
        '.agent-inline-editor input[aria-label="Close idle children after minutes"]',
      );
      if (!field) throw new Error("the idle-close minutes field did not render");
      return field;
    };
    const offTick = (): HTMLInputElement => {
      const tick = container.querySelector<HTMLInputElement>(
        '.agent-inline-editor input[aria-label="Never close idle children"]',
      );
      if (!tick) throw new Error("the idle-close off toggle did not render");
      return tick;
    };

    // 1. The default: a profile that says nothing opens showing 30, and a
    // save that never touched the field keeps the key out — absent is what
    // means 30 to the daemon, so an untouched field leaves the row alone.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    expect(idleField().value).toBe("30");
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [explorer], standingInstructions: "" },
    });
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const first = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect("idleCloseMinutes" in first.profiles[0]).toBe(false);

    // 2. A custom value: typed, stored under the key, and shown again on the
    // next open — the round trip the field exists for.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    await typeText(idleField(), "45");
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...explorer, idleCloseMinutes: 45 }],
        standingInstructions: "",
      },
    });
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenLastCalledWith({
      profiles: [{ ...explorer, idleCloseMinutes: 45 }],
      standingInstructions: "",
    });
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    expect(idleField().value).toBe("45");
    expect(offTick().checked).toBe(false);

    // 3. Off: the tick saves the daemon's `Some(0)`, and reopening it shows
    // the tick still on, the field at the default and out of the way.
    await act(async () => offTick().click());
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...explorer, idleCloseMinutes: 0 }],
        standingInstructions: "",
      },
    });
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenLastCalledWith({
      profiles: [{ ...explorer, idleCloseMinutes: 0 }],
      standingInstructions: "",
    });
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    expect(offTick().checked).toBe(true);
    expect(idleField().disabled).toBe(true);
    expect(idleField().value).toBe("30");
  });

  it("never lets a typed 0 in the minutes field mean never", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const idleField = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Close idle children after minutes"]',
    );
    const offTick = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Never close idle children"]',
    );
    if (!idleField || !offTick) throw new Error("the idle-close controls did not render");

    await typeText(idleField, "0");
    // The field shows what will be saved: a whole minute. "Never" is the
    // tick's meaning alone, so a typed 0 must not reach the daemon as
    // `Some(0)` — the opposite of what the human asked for.
    expect(idleField.value).toBe("1");
    expect(offTick.checked).toBe(false);

    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), idleCloseMinutes: 1 }],
        standingInstructions: "",
      },
    });
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenLastCalledWith({
      profiles: [{ ...makeProfile(), idleCloseMinutes: 1 }],
      standingInstructions: "",
    });
  });

  it("refuses minutes that are not a number, and never shows the form's own NaN", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const idleField = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Close idle children after minutes"]',
    );
    if (!idleField) throw new Error("the idle-close minutes field did not render");

    // A number the field cannot hold: it parses to Infinity, so the draft
    // must carry what the human typed (the refusal names it) rather than a
    // `String(NaN)` of the form's making — which no number input would
    // show either.
    await typeText(idleField, "1e999");
    expect(idleField.value).toBe("1e999");

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("whole number of minutes");
  });

  it("wires the idle-close hint to its input, the way every other field does", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const idleField = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Close idle children after minutes"]',
    );
    if (!idleField) throw new Error("the idle-close minutes field did not render");
    // The three sentences that carry the field's whole meaning — the
    // default, the cap and what the close waits for — are read out with the
    // control, not left beside it.
    expect(describedText(idleField)).toContain("30 by default");
    expect(describedText(idleField)).toContain("10080");
    expect(describedText(idleField)).toContain("nobody looking at it");
  });

  it("refuses a rename that would put two enabled profiles on one name, before sending", async () => {
    const scout = makeProfile({ id: "s1", name: "Scout", enabledForAgents: true });
    const reviewer = makeProfile({ id: "r1", name: "Reviewer", enabledForAgents: true });
    await renderAgentsPanel({ profiles: [scout, reviewer], standingInstructions: "" });

    await act(async () => rowButton("Reviewer", "Edit").click());
    await act(async () => undefined);
    const nameField = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Profile name"]',
    );
    if (!nameField) throw new Error("name field did not render");
    await typeText(nameField, "Scout");
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    // Refused before the write: the daemon's rule, named by the form first.
    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Scout");
    expect(alert?.textContent).toContain("enabled");
  });

  it("saves an icon and clears it to none", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), icon: "eye" }],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const iconField = container.querySelector<HTMLInputElement>('[aria-label="Profile icon"]');
    if (!iconField) throw new Error("icon field did not render");
    await typeText(iconField, "eye");
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [{ ...makeProfile(), icon: "eye" }],
      standingInstructions: "",
    });

    // Clearing the field is none on the wire: null, never an empty string.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const iconAgain = container.querySelector<HTMLInputElement>('[aria-label="Profile icon"]');
    if (!iconAgain) throw new Error("icon field did not render on the second open");
    await typeText(iconAgain, "");
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    const sent = vi.mocked(agentProfilesSet).mock.calls[1]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.icon).toBeNull();
  });

  it("edits the agents tick and the peer restriction from the editor", async () => {
    const restricted = makeProfile({
      enabledForAgents: false,
      toolOverlay: ["devboule_send_message", "devboule_create_agent"],
    });
    await renderAgentsPanel({ profiles: [restricted], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const agentsTick = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Available to agents"]',
    );
    const peersTick = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Children cannot message peers or create further agents"]',
    );
    if (!agentsTick || !peersTick) throw new Error("the editor's ticks did not render");
    expect(agentsTick.checked).toBe(false);
    expect(peersTick.checked).toBe(true);
    await act(async () => agentsTick.click());
    await act(async () => peersTick.click());
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [{ ...restricted, enabledForAgents: true, toolOverlay: [] }],
      standingInstructions: "",
    });
  });

  it("keeps a stored feature it cannot draw, and carries it through a save", async () => {
    // The provider was never asked what it offers: this test's mock answers the
    // vocabulary query with no `features` axis at all, which is what a daemon
    // older than the field replies. With no list there is nothing to prune by,
    // so a stored key survives the save — the rule that separates "nobody could
    // ask" from "offers nothing", and the reason the daemon's own prune is
    // conditional in the same way.
    const featured = makeProfile({ features: { autoAccept: true, sandbox: "none" } });
    await renderAgentsPanel({ profiles: [featured], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [featured],
      standingInstructions: "",
    });
  });

  it("edits and saves a profile that arrived without features or spawn prompt", async () => {
    // The profile exactly as the daemon serves one whose omittable fields are
    // all empty: serde skips `icon`, `spawnPrompt`, `thinkingOptionId`,
    // `features` and `toolOverlay`, and a document older builds wrote never
    // had those keys. Only the keys the daemon always sends are here —
    // reading an omitted one without a default blanked the app live.
    const legacy: AgentProfile = {
      id: "profile-1",
      name: "Explorer",
      note: "Written by an older build.",
      provider: "grok",
      model: "grok-4",
      modeId: "ask",
      enabledForAgents: false,
    };
    await renderAgentsPanel({ profiles: [legacy], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("the editor did not open for the legacy profile");
    // Absent features read as none, not as a crash: the tick is drawn (this
    // daemon answers the vocabulary query without a features axis, so the form
    // falls back to the one feature every agent family applies) and nothing
    // invented is stored for the keys the profile never had.
    expect(
      editor.querySelector('[aria-label="Auto accept for children of this profile"]'),
    ).not.toBeNull();
    expect(editor.querySelector<HTMLInputElement>('[aria-label="Profile name"]')?.value).toBe(
      "Explorer",
    );
    expect(editor.querySelector<HTMLTextAreaElement>('[aria-label="Profile note"]')?.value).toBe(
      "Written by an older build.",
    );

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    // Every field the profile had survives the save; the omitted ones come
    // back as their empty values, which the daemon skips again on disk.
    expect(sent.profiles[0]).toEqual({
      ...legacy,
      icon: null,
      thinkingOptionId: null,
      features: {},
      toolOverlay: [],
    });
  });

  it("keeps a peer restriction that shares the overlay with another denial", async () => {
    const guarded = makeProfile({
      toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
    });
    await renderAgentsPanel({ profiles: [guarded], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    // The peer tick is on: the peer tools are in the overlay, whatever else
    // is there with them.
    const peersTick = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Children cannot message peers or create further agents"]',
    );
    if (!peersTick) throw new Error("the peer tick did not render");
    expect(peersTick.checked).toBe(true);
    // Edit only the note and save: the overlay must survive untouched.
    const noteField = container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile note"]',
    );
    if (!noteField) throw new Error("note field did not render");
    await typeText(noteField, "Updated note for the agent.");
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [
        {
          ...guarded,
          note: "Updated note for the agent.",
          toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
        },
      ],
      standingInstructions: "",
    });
  });

  it("holds the row's agents tick while that profile's editor is open", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ enabledForAgents: true })],
      standingInstructions: "",
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    let rowTick = container.querySelector<HTMLInputElement>(
      '.agent-profile-row input[type="checkbox"]',
    );
    if (!rowTick) throw new Error("row tick did not render");
    expect(rowTick.disabled).toBe(true);
    // The reason is on the screen, not a silent lock.
    expect(container.textContent).toContain("The open editor holds this setting");

    await act(async () => rowButton("Explorer", "Close editor").click());
    await act(async () => undefined);
    rowTick = container.querySelector<HTMLInputElement>(
      '.agent-profile-row input[type="checkbox"]',
    );
    if (!rowTick) throw new Error("row tick did not render after close");
    expect(rowTick.disabled).toBe(false);
  });

  it("accepts a spawn prompt whose trimmed bytes fit the cap", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const spawnField = container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile spawn prompt"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // One U+0085 over the cap raw, exactly the cap after the daemon's trim:
    // Rust's `str::trim` drops U+0085 and ECMAScript's `trim()` keeps it, so
    // the preflight must trim the way Rust trims — a JS-trimmed count is
    // 8194 bytes and would refuse a prompt the store accepts.
    const prompt = `\u{0085}${"a".repeat(8192)}`;
    await typeText(spawnField, prompt);

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.spawnPrompt).toBe("a".repeat(8192));
  });

  it("refuses a spawn prompt over 8 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const spawnField = container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile spawn prompt"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // 4097 two-byte characters: 8194 UTF-8 bytes, 2 over the cap. The byte
    // count is what the daemon enforces, so a char-counting UI would pass it.
    const flood = "é".repeat(4097);
    await typeText(spawnField, flood);

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("8194");
    expect(alert?.textContent).toContain("8192");
    // The refusal changed nothing: the field still holds every byte.
    expect(spawnField.value).toBe(flood);
  });

  it("saves a cleared spawn prompt as the field's absence, never as an empty string", async () => {
    const carrying = makeProfile({ spawnPrompt: "Check the diff before you report." });
    await renderAgentsPanel({ profiles: [carrying], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const spawnField = container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile spawn prompt"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // Whitespace only: the daemon trims the field, so this is none, and the
    // wire shape of none is the key's absence.
    await typeText(spawnField, "   ");

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect("spawnPrompt" in sent.profiles[0]).toBe(false);
  });

  it("says when the spawn prompt is sent and that running agents keep what they started with", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    expect(editor.textContent).toContain(
      "Sent at the start of every agent created from this profile, before the creator's prompt",
    );
    expect(editor.textContent).toContain("Agents already running keep what they started with");
    // The spawn prompt carries its own counter, in the daemon's units.
    expect(editor.textContent).toContain("8192 bytes");
  });

  it("ties one hint to the spawn prompt field, keep-it-short sentence included", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    const spawnField = editor.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Profile spawn prompt"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // One hint under the field: the keep-it-short note was merged into the
    // hint the field already had rather than stacked as a second one.
    expect(spawnField.closest("label")?.querySelectorAll(".device-field-hint").length).toBe(1);
    // …and the field's described-by is what points at that hint.
    const hinted = describedText(spawnField);
    expect(hinted).toContain(
      "Sent at the start of every agent created from this profile, before the creator's prompt",
    );
    expect(hinted).toContain(
      "Keep it short. An agent created from this profile also receives the standing instructions and the task written by the agent that creates it — write only what is specific to this kind of agent.",
    );
  });

  it("ties one hint to the standing instructions box, keep-it-short sentence included", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "Rules to reuse." });

    const box = container.querySelector<HTMLTextAreaElement>(
      '[aria-label="Standing instructions for every agent"]',
    );
    if (!box) throw new Error("the standing instructions box did not render");
    // One field-hint in the box — the section's `device-copy` description is
    // not a hint, so nothing was merged there — tied through described-by.
    expect(box.closest(".agent-standing")?.querySelectorAll(".device-field-hint").length).toBe(1);
    expect(describedText(box)).toBe("Keep it short: every agent also receives its own task.");
  });

  it("keeps the editor's draft on screen under its error when a rename is refused", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" })],
      standingInstructions: "",
    });
    // The store's read-back after the (only) confirmed save, at the retry.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          makeProfile({ id: "x1", name: "Scout", note: "Maps the work before anyone builds." }),
        ],
        standingInstructions: "",
      },
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editorNameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    const editorNoteField = container.querySelector<HTMLTextAreaElement>(
      ".agent-inline-editor textarea",
    );
    if (!editorNameField || !editorNoteField) throw new Error("editor fields did not render");
    await typeText(editorNameField, "Scout");
    await typeText(editorNoteField, "Maps the work before anyone builds.");

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The refusal is named, the editor still stands, and the draft is in
    // its fields — the create form's rule, held here too.
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const editor = container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
    expect(editor?.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe(
      "Maps the work before anyone builds.",
    );
    // The row under it is exactly what the human was seeing before.
    expect(rowByName("Explorer").querySelector(".settings-card-title")?.textContent).toBe(
      "Explorer",
    );

    // The retry sends the same draft, and confirmation — never submission —
    // closes the editor.
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const sent = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(sent?.profiles[0]?.name).toBe("Scout");
    expect(sent?.profiles[0]?.note).toBe("Maps the work before anyone builds.");
    expect(container.querySelector(".agent-inline-editor")).toBeNull();
  });

  it("keeps the editor's draft when a refused delete removes and restores its row", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" }), makeProfile({ id: "x2", name: "Coder" })],
      standingInstructions: "",
    });
    // The delete's fate is held outside, so each stage of the sequence is
    // observable deterministically: the optimistic removal, then the
    // refusal's revert.
    let rejectSet!: (cause: unknown) => void;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectSet = reject;
        }),
    );

    // Open the editor on the row that will be deleted, and type into it.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    const noteField = container.querySelector<HTMLTextAreaElement>(".agent-inline-editor textarea");
    if (!nameField || !noteField) throw new Error("editor fields did not render");
    await typeText(nameField, "Scout");
    await typeText(noteField, "Maps the work before anyone builds.");

    // Delete that same row: the optimistic removal unmounts the editor —
    // the row, and the editor rendered inside it, are gone from the screen.
    await act(async () => rowButton("Explorer", "Delete").click());
    await act(async () => sectionButton("Delete now").click());
    await act(async () => undefined);
    expect(container.querySelector(".agent-inline-editor")).toBeNull();

    // The write is refused; the revert brings the row back and the editor
    // remounts. It must come back with the draft in its fields, under the
    // error — the rule its own write obeys, held for a write that removed
    // the row. A draft kept inside the editor's own state would remount
    // empty here; it lives one level up for exactly this.
    await act(async () => {
      rejectSet({ code: "io", message: "profile file unwritable" });
    });
    await act(async () => undefined);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const editor = container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
    expect(editor?.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe(
      "Maps the work before anyone builds.",
    );
    expect(rowByName("Explorer").querySelector(".settings-card-title")?.textContent).toBe(
      "Explorer",
    );
  });

  it("keeps the editor's draft across a confirmed write from another row", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" }), makeProfile({ id: "x2", name: "Coder" })],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed tick.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          makeProfile({ id: "x1" }),
          makeProfile({ id: "x2", name: "Coder", enabledForAgents: true }),
        ],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("editor name field did not render");
    await typeText(nameField, "Scout");

    // Another row's write, confirmed with its read-back: the editor stays
    // open and the draft stays in it — no write that did not carry the
    // draft may release it.
    await act(async () => tickBox("Coder").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
  });

  it("refuses a note over 2 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const noteField = container.querySelector<HTMLTextAreaElement>(".agent-inline-editor textarea");
    if (!noteField) throw new Error("note field did not render");
    // 1100 two-byte characters: 2200 UTF-8 bytes, 152 over the cap. The byte
    // count is what the daemon enforces, so a char-counting UI would pass it.
    const flood = "é".repeat(1100);
    await typeText(noteField, flood);

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("2200 bytes");
    expect(alert?.textContent).toContain("2048");
    // The refusal changed nothing: the field still holds every byte.
    expect(noteField.value).toBe(flood);
  });

  it("saves standing instructions into the document and leaves the profiles alone", async () => {
    const explorer = makeProfile();
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save: the text is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [explorer],
        standingInstructions: "Report your result in your final message.",
      },
    });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Report your result in your final message.");

    await act(async () => sectionButton("Save standing instructions").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [explorer],
      standingInstructions: "Report your result in your final message.",
    });
  });

  it("refuses standing instructions over 8 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    // 4200 two-byte characters: 8400 UTF-8 bytes, 208 over the cap.
    const flood = "é".repeat(4200);
    await typeText(field, flood);

    await act(async () => sectionButton("Save standing instructions").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("8400 bytes");
    expect(alert?.textContent).toContain("8192");
    expect(field.value).toBe(flood);
  });

  it("keeps the standing draft on screen when an unrelated write confirms", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: no draft in it.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), enabledForAgents: true }],
        standingInstructions: "",
      },
    });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Always report your plan first.");

    // An unrelated write — the tick. It sends the document as the store
    // holds it (the draft is deliberately not smuggled into it), and when
    // it confirms the typed text must still be in the box: the tick did not
    // carry the text, so releasing the draft would destroy words no write
    // ever took. An earlier version of this test pinned the opposite — the
    // release — which is the silent loss the cumulative audit's finding 1.
    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), enabledForAgents: true }],
      standingInstructions: "",
    });
    const fieldAfter = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    expect(fieldAfter?.value).toBe("Always report your plan first.");
  });

  it("keeps keystrokes typed while the standing save itself was in flight", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed save: it holds what was sent.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [makeProfile()], standingInstructions: "Always report" },
    });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Always report");
    let resolveSet: (() => void) | undefined;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveSet = resolve;
        }),
    );
    await act(async () => sectionButton("Save standing instructions").click());
    await act(async () => undefined);

    // The write is in flight carrying "Always report"; the human keeps
    // typing (the box is deliberately editable mid-write). The confirmation
    // may release a draft that is still what was sent — this tail is newer
    // than the store and must survive it.
    await typeText(field, "Always report your plan first.");
    await act(async () => {
      resolveSet?.();
    });
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [makeProfile()],
      standingInstructions: "Always report",
    });
    const fieldAfter = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    expect(fieldAfter?.value).toBe("Always report your plan first.");
  });

  it("accepts a name at the daemon's own count: 40 astral-plane characters are 40 characters", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const nameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("name field did not render");
    // 40 emoji are 40 Unicode scalar values — what the daemon counts — but
    // 80 UTF-16 code units. A length-counting panel would refuse a legal
    // name; this one must send it.
    const name = "🦄".repeat(40);
    await typeText(nameField, name);

    // The store's read-back after the confirmed save: the name is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [{ ...makeProfile(), name }], standingInstructions: "" },
    });

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), name }],
      standingInstructions: "",
    });
  });

  it("refuses a name past the daemon's count with the daemon's number", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const nameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("name field did not render");
    // 61 emoji are 61 scalar values — 61 for the daemon too — but 122 UTF-16
    // code units. The refusal must name 61, the daemon's number, never 122.
    const name = "🦄".repeat(61);
    await typeText(nameField, name);

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("61 characters");
    expect(alert?.textContent).toContain("60-character cap");
    // The refusal truncates nothing and leaves the editor open.
    expect(nameField.value).toBe(name);
  });

  it("ends a failed load in a retryable state instead of loading forever", async () => {
    await renderAgentsPanelErrored();

    // Terminal state: the daemon's sentence and a Retry, no loading line.
    expect(container.textContent).not.toContain("Loading agent profiles…");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const retry = sectionButton("Retry");

    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [makeProfile()], standingInstructions: "" },
    });
    await act(async () => retry.click());
    await act(async () => undefined);

    expect(agentProfilesGet).toHaveBeenCalledTimes(2);
    expect(profileRows()).toHaveLength(1);
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("applies a refetch after a write, so a restarted daemon's store replaces the stale panel", async () => {
    vi.useFakeTimers();
    try {
      await renderAgentsPanel({
        profiles: [makeProfile()],
        standingInstructions: "",
      });

      // The write is confirmed with a read-back (the panel adopts the stored
      // document, ids included): the store holds the tick.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: {
          profiles: [{ ...makeProfile(), enabledForAgents: true }],
          standingInstructions: "",
        },
      });

      // A write puts the sequence past zero; a daemon restart then flips the
      // handshake capability off and back on, re-running the load effect
      // while the panel stays mounted.
      await act(async () => tickBox("Explorer").click());
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(true);

      // A draft typed after the write, never saved: the fresh load must
      // release it, so the box shows the restarted store's instructions.
      const fieldBefore = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
      if (!fieldBefore) throw new Error("standing instructions field did not render");
      await typeText(fieldBefore, "typed against the old daemon");

      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // The daemon came back with an emptied store — the quarantined-file
      // direction — and the panel must take that truth, not keep the stale
      // optimistic document from before the restart.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: { profiles: [], standingInstructions: "fresh from the restarted store" },
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith([
          "ping",
          "status",
          "sessions",
          "journal",
          "typed_permissions",
          "devices",
          "agent_profiles",
        ]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // Three reads in total: the load, the write's read-back, and the
      // restarted store's refetch.
      expect(agentProfilesGet).toHaveBeenCalledTimes(3);
      expect(profileRows()).toHaveLength(0);
      // A fresh load also releases any draft: the box reads the new store.
      const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
      expect(field?.value).toBe("fresh from the restarted store");
    } finally {
      vi.useRealTimers();
    }
  });

  it("holds the inline editor under the busy lock so a second write cannot start", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
      standingInstructions: "",
    });

    // The editor is opened BEFORE any write, so its Save button exists while
    // another row's write is still in flight — the hole the lock closes.
    await act(async () => rowButton("Coder", "Edit").click());
    await act(async () => undefined);
    const save = sectionButton("Save");

    vi.mocked(agentProfilesSet).mockImplementationOnce(() => new Promise<void>(() => undefined));
    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(save.disabled).toBe(true);
    // Even a dispatched click cannot start a second write while the first is
    // in flight: React does not invoke onClick on a disabled button.
    await act(async () => save.click());
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("keeps the panel locked until the read-back lands, so no write can re-send an empty id", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The write will confirm, and its read-back — where the daemon's minted
    // ids are adopted — is armed and does not answer yet. This constructs
    // the window the cumulative audit's finding 3: the moment between the
    // write's confirmation and its read-back.
    let resolveReadBack: ((reply: AgentProfilesReply) => void) | undefined;
    vi.mocked(agentProfilesGet).mockImplementationOnce(
      () =>
        new Promise<AgentProfilesReply>((resolve) => {
          resolveReadBack = resolve;
        }),
    );

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    // The write is confirmed but its read-back has not landed. Every writer
    // must still be locked: a write sent now would travel on the
    // pre-read-back document, re-send `id: ""` for a row the daemon has
    // already named, and its sequence would discard the very read-back that
    // was about to heal the panel. An earlier version released `busy`
    // before the read-back resolved; these assertions are what that got
    // wrong.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesGet).toHaveBeenCalledTimes(2);
    expect(tickBox("Explorer").disabled).toBe(true);
    expect(sectionButton("New profile").disabled).toBe(true);

    // The read-back lands: the window closes, the minted id is adopted,
    // and the panel is writable again.
    resolveReadBack?.({
      document: {
        profiles: [makeProfile({ id: "minted-1", enabledForAgents: true })],
        standingInstructions: "",
      },
    });
    await act(async () => undefined);

    expect(tickBox("Explorer").disabled).toBe(false);
    expect(sectionButton("New profile").disabled).toBe(false);
    expect(tickBox("Explorer").checked).toBe(true);
  });

  it("does not adopt a store fetch that raced a write still in flight", async () => {
    vi.useFakeTimers();
    try {
      await renderAgentsPanel({
        profiles: [makeProfile()],
        standingInstructions: "",
      });

      // A write whose fate is still open: the optimistic tick is on screen,
      // the daemon has not answered.
      let rejectSet!: (cause: unknown) => void;
      vi.mocked(agentProfilesSet).mockImplementationOnce(
        () =>
          new Promise<void>((_resolve, reject) => {
            rejectSet = reject;
          }),
      );
      await act(async () => tickBox("Explorer").click());
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(true);

      // While that write is in flight, a daemon restart flips the
      // capability off and back on, re-running the load effect. Its reply
      // is the store's pre-write truth — the reply that must adopt nothing.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: {
          profiles: [makeProfile({ note: "raced the write" })],
          standingInstructions: "",
        },
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith([
          "ping",
          "status",
          "sessions",
          "journal",
          "typed_permissions",
          "devices",
          "agent_profiles",
        ]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // The racing fetch was issued and adopted nothing: the optimistic
      // document still owns the panel. The old guard compared sequence
      // numbers only, so this reply WAS adopted here — and the write's
      // revert then clobbered it — because the guard never asked whether a
      // write was in flight when the fetch started.
      expect(agentProfilesGet).toHaveBeenCalledTimes(2);
      expect(tickBox("Explorer").checked).toBe(true);
      expect(container.textContent).not.toContain("raced the write");

      // The write then refuses: the revert restores exactly what the human
      // was seeing, under the error — with no adopted reply in between.
      await act(async () => {
        rejectSet({ code: "io", message: "profile file unwritable" });
      });
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(false);
      expect(container.querySelector('[role="alert"]')?.textContent).toContain(
        "A system or file operation failed on this machine.",
      );
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("Settings agents panel — new profile form", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  /** The handshake of every daemon shipping today: no `provider_vocabulary`. */
  const OLDER_DAEMON = [
    "ping",
    "status",
    "sessions",
    "journal",
    "typed_permissions",
    "devices",
    "agent_profiles",
  ];
  const VOCABULARY_DAEMON = [...OLDER_DAEMON, "provider_vocabulary"];

  function makeProfile(overrides: Partial<AgentProfile> = {}): AgentProfile {
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

  function makeProvider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
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
  function autoAcceptRow(): VocabularyFeature {
    return { id: "autoAccept", label: "Auto accept", author: "daemon", type: "toggle" };
  }

  function makeVocabulary(overrides: Partial<ProviderVocabulary> = {}): ProviderVocabulary {
    return {
      provider: "claude",
      models: { state: "absent", items: [] },
      modes: { state: "absent", items: [] },
      // The features axis an answer carries by default in this file: the tick,
      // which every agent family offers. A test that wants the no-answer case
      // passes `features: undefined` — a daemon older than the field — and a
      // test that wants a provider with more rows overrides `items`.
      features: { state: "present", items: [autoAcceptRow()] },
      source: "probe",
      probedAtMs: null,
      ...overrides,
    };
  }

  /**
   * A stored row as the daemon reads it back after the form created one:
   * the shape the form sends, under the id the daemon minted.
   */
  function storedProfile(id: string, overrides: Partial<AgentProfile> = {}): AgentProfile {
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

  function daemonStatusWith(capabilities: string[]): DaemonStatus {
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

  // Renders the surface, opens the Agents tab and answers the document
  // fetch. `capabilities` decides which daemon generation the form meets:
  // without `provider_vocabulary` (every daemon today) it must fall back to
  // free text and say that reason out loud.
  async function renderAgentsPanel(
    doc: AgentProfilesDocument,
    capabilities: string[] = OLDER_DAEMON,
  ) {
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(capabilities));
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({ document: doc });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);
  }

  async function openForm() {
    const button = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".agent-profile-create-row button"),
    ).find((candidate) => candidate.textContent === "New profile");
    if (!button) throw new Error("New profile button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
  }

  /**
   * Opens one stored row's editor. The form is shared with the New-profile
   * flow, so the editor is told apart by the class only the create mode
   * carries (`agent-profile-create`).
   */
  async function openRowEditor(name: string): Promise<HTMLElement> {
    const row = Array.from(container.querySelectorAll<HTMLElement>(".agent-profile-row")).find(
      (candidate) => candidate.textContent?.includes(name),
    );
    if (!row) throw new Error(`profile row ${name} did not render`);
    const edit = Array.from(row.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Edit",
    );
    if (!edit) throw new Error(`Edit button on ${name} did not render`);
    await act(async () => edit.click());
    await act(async () => undefined);
    const editor = container.querySelector<HTMLElement>(
      ".agent-inline-editor:not(.agent-profile-create)",
    );
    if (!editor) throw new Error(`the editor of ${name} did not render`);
    return editor;
  }

  function form(): HTMLElement {
    const element = container.querySelector<HTMLElement>(".agent-profile-create");
    if (!element) throw new Error("new-profile form did not render");
    return element;
  }

  function field<T extends Element>(selector: string): T {
    const element = form().querySelector<T>(selector);
    if (!element) throw new Error(`field ${selector} did not render in the form`);
    return element;
  }

  function nameField(): HTMLInputElement {
    return field<HTMLInputElement>('input[aria-label="Profile name"]');
  }

  function noteField(): HTMLTextAreaElement {
    return field<HTMLTextAreaElement>('textarea[aria-label="Profile note"]');
  }

  function providerField(): HTMLSelectElement {
    return field<HTMLSelectElement>('select[aria-label="Provider"]');
  }

  /** The model control is a select when the provider published, input otherwise. */
  function modelControl(): HTMLInputElement | HTMLSelectElement {
    return field<HTMLInputElement | HTMLSelectElement>('[aria-label="Model"]');
  }

  function modeControl(): HTMLInputElement | HTMLSelectElement {
    return field<HTMLInputElement | HTMLSelectElement>('[aria-label="Mode"]');
  }

  function createButton(): HTMLButtonElement {
    const button = Array.from(form().querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Create profile",
    );
    if (!button) throw new Error("Create profile button did not render");
    return button;
  }

  /** A profile row's one checkbox is its "agents may create this" tick. */
  function rowTicks(): HTMLInputElement[] {
    return Array.from(
      container.querySelectorAll<HTMLInputElement>(
        ".agent-profile-list .agent-profile-row input[type='checkbox']",
      ),
    );
  }

  function rowTick(name: string): HTMLInputElement {
    const row = rowTicks().find((candidate) =>
      candidate.closest(".agent-profile-row")?.textContent?.includes(name),
    );
    if (!row) throw new Error(`tick for ${name} did not render`);
    return row;
  }

  /** Select options' values, in wire order. */
  function selectValues(control: HTMLInputElement | HTMLSelectElement): string[] {
    if (control.tagName !== "SELECT") throw new Error(`control is a ${control.tagName}`);
    return Array.from((control as HTMLSelectElement).options).map((option) => option.value);
  }

  // Drives a controlled React field directly (the suite's raw createRoot/act
  // style has no testing-library fireEvent). Inputs and textareas take the
  // rendered onChange through their __reactProps key; a select carries no
  // such key under React 19 (its onChange rides the native bubbling change
  // event), so it is driven by setting the value and dispatching that event.
  async function typeText(
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

  // A fresh form draft filled for an older daemon: enough to save.
  async function fillDraft() {
    await typeText(nameField(), "Gamma");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
  }

  // Drives a controlled checkbox through its rendered onChange — the same
  // value a click would leave in the field. A raw `.click()` on a remounted
  // form's checkbox loses the synthetic change to a happy-dom/React event
  // quirk (the DOM ticks, the state does not), so the suite drives the
  // handler the way the paste-and-type helper does for text fields.
  async function tickCheckbox(box: HTMLInputElement, next: boolean) {
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

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [makeProvider()],
      unreadableDirs: 0,
    }));
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root!.unmount());
    container.remove();
    vi.clearAllMocks();
    // `clearAllMocks` keeps queued `mockImplementationOnce` entries, so a test
    // that queued an unsettled write and failed before consuming it would
    // leave the next test's click reading a hanging write. Reset back to the
    // resolved default, exactly as the tool-toggles block does for its write.
    vi.mocked(agentProfilesSet).mockReset();
    vi.mocked(agentProfilesSet).mockImplementation(async () => undefined);
    vi.mocked(providerVocabularyGet).mockReset();
  });

  it("saves a new profile with enabledForAgents false and an empty id at the end of the list", async () => {
    const beta = makeProfile({ id: "b", name: "Beta" });
    const gamma: AgentProfile = {
      id: "",
      name: "Gamma",
      icon: null,
      note: "Checks the build output.",
      provider: "claude",
      model: "claude-sonnet-4-5",
      modeId: "default",
      thinkingOptionId: null,
      features: {},
      toolOverlay: [],
      enabledForAgents: false,
    };
    await renderAgentsPanel({ profiles: [beta], standingInstructions: "" });
    // The store's read-back after the confirmed create: the daemon minted
    // the id the form could not know.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [beta, { ...gamma, id: "minted-1" }], standingInstructions: "" },
    });
    await openForm();

    await typeText(nameField(), "Gamma");
    await typeText(noteField(), "Checks the build output.");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    // The whole document travels: the old list first, in order, then exactly
    // one new entry. `id` stays empty — the daemon mints it.
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [beta, gamma],
      standingInstructions: "",
    });
  });

  it("passes auto accept into features only when it is ticked, and says what it does", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-backs after each confirmed create: the store grows by
    // one, under the ids the daemon minted.
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-1"),
            storedProfile("minted-2", { features: { autoAccept: true } }),
          ],
          standingInstructions: "",
        },
      });
    await openForm();

    // The copy must say what the tick does — it is the most consequential
    // control on the form.
    expect(form().textContent).toContain("approve their own permission prompts");

    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    // Unticked: features carries nothing.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const unticked = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(unticked?.profiles[0]?.features).toEqual({});

    // Again, with the tick: features.autoAccept is the one flag.
    await openForm();
    await fillDraft();
    const autoAccept = field<HTMLInputElement>(
      'input[aria-label="Auto accept for children of this profile"]',
    );
    await tickCheckbox(autoAccept, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    // The document now carries the first save too; the appended entry is the
    // one this second save created.
    const ticked = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(ticked?.profiles.at(-1)?.features).toEqual({ autoAccept: true });
  });

  it("defaults the agents tick to off and saves it only when the human ticks it", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create: the tick is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [storedProfile("minted-1", { enabledForAgents: true })],
        standingInstructions: "",
      },
    });
    await openForm();

    const tick = field<HTMLInputElement>('input[aria-label="Available to agents"]');
    expect(tick.checked).toBe(false);

    await fillDraft();
    await tickCheckbox(tick, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.enabledForAgents).toBe(true);
  });

  it("saves no overlay by default and both peer tools when the tick is on", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();

    const tick = field<HTMLInputElement>(
      'input[aria-label="Children cannot message peers or create further agents"]',
    );
    expect(tick.checked).toBe(false);
    // The tick names what it denies: peer messages and further creations.
    // It must not promise a surface it does not deliver, so no "design".
    expect(form().textContent).toContain("cannot message other agents or create further");
    expect(form().textContent).not.toContain("Design");

    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const unticked = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(unticked?.profiles[0]?.toolOverlay).toEqual([]);

    // Again, with the tick: the overlay denies exactly the two peer tools.
    await openForm();
    await fillDraft();
    const retick = field<HTMLInputElement>(
      'input[aria-label="Children cannot message peers or create further agents"]',
    );
    await tickCheckbox(retick, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const ticked = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(ticked?.profiles.at(-1)?.toolOverlay).toEqual([
      "devboule_send_message",
      "devboule_create_agent",
    ]);
  });

  it("shows the peer restriction on a stored profile row", async () => {
    await renderAgentsPanel({
      profiles: [
        storedProfile("p-1", {
          name: "Hermit",
          toolOverlay: ["devboule_send_message", "devboule_create_agent"],
        }),
        storedProfile("p-2", { name: "Social" }),
      ],
      standingInstructions: "",
    });

    const hermit = container.textContent ?? "";
    expect(hermit).toContain("cannot message peers or create further agents");
  });

  it("names the denial on a row whose overlay is not the exact peer pair", async () => {
    // A single-tool denial is valid daemon-side; the row must render it
    // instead of showing nothing.
    await renderAgentsPanel({
      profiles: [
        storedProfile("p-1", {
          name: "NoGrandchildren",
          toolOverlay: ["devboule_create_agent"],
        }),
      ],
      standingInstructions: "",
    });

    const text = container.textContent ?? "";
    expect(text).toContain("cannot use: devboule_create_agent");
    expect(text).not.toContain("cannot message peers");
  });

  it("renders absent vocabulary as free text with the spec's sentence, never as a select", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(makeVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(providerVocabularyGet).toHaveBeenCalledWith("claude", "", false);
    // The spec's own sentence, once per axis.
    expect(form().textContent).toContain(
      "This provider did not publish its models; what you type is checked when the session starts.",
    );
    expect(form().textContent).toContain(
      "This provider did not publish its modes; what you type is checked when the session starts.",
    );
    // Free text, not an empty select: the human can finish the form.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
  });

  it("shows a stored model and mode the provider no longer lists, instead of an empty field", async () => {
    // The reply publishes one model and one mode, neither of them the row's.
    // A select over published items alone would render both fields empty,
    // hiding the values the human opened the editor to change; the stored
    // value is appended and labelled as the saved one, so the row's value is
    // visible, selected, and kept by a save that touches nothing else.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "claude-opus-4-6", name: "Claude Opus 4.6" }],
        },
        modes: { state: "present", origin: "provider", items: [{ id: "plan", name: "Plan" }] },
      }),
    );
    const stored = storedProfile("p-1", { name: "Explorer" });
    await renderAgentsPanel({ profiles: [stored], standingInstructions: "" }, VOCABULARY_DAEMON);
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [stored], standingInstructions: "" },
    });
    const editor = await openRowEditor("Explorer");
    await act(async () => undefined);

    const model = editor.querySelector<HTMLSelectElement>('select[aria-label="Model"]');
    const mode = editor.querySelector<HTMLSelectElement>('select[aria-label="Mode"]');
    if (!model || !mode) throw new Error("the model and mode selects did not render");
    expect(selectValues(model)).toEqual(["", "claude-opus-4-6", "claude-sonnet-4-5"]);
    expect(selectValues(mode)).toEqual(["", "plan", "default"]);
    expect(model.value).toBe("claude-sonnet-4-5");
    expect(mode.value).toBe("default");
    expect(
      Array.from(model.options).find((option) => option.value === "claude-sonnet-4-5")?.textContent,
    ).toBe("claude-sonnet-4-5 (the value saved on this profile)");

    // A save that changes no vocabulary field carries the stored pair, not
    // the empty string a blank select would have left in the draft.
    const save = Array.from(editor.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Save",
    );
    if (!save) throw new Error("the editor's Save button did not render");
    await act(async () => save.click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.model).toBe("claude-sonnet-4-5");
    expect(sent.profiles[0]?.modeId).toBe("default");
  });

  it("says none and absent differently: a provider that answers 'I have none' is not a silent one", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({ models: { state: "none", items: [] } }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // `none`: the provider CAN answer and answered "I have none".
    expect(form().textContent).toContain("This provider reports no models");
    expect(form().textContent).not.toContain("did not publish its models");
    // The modes axis in the same reply is `absent`: the two sentences must
    // not collapse into one.
    expect(form().textContent).toContain("did not publish its modes");
    expect(form().textContent).not.toContain("reports no modes");
    // Both fields stay required and typeable either way.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
  });

  it("keeps the form completable on an older daemon, names that reason, and sends no vocabulary query", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();

    // The older-daemon sentence — not the provider's "did not publish".
    expect(form().textContent).toContain("older than this app");
    expect(form().textContent).not.toContain("did not publish");
    expect(providerVocabularyGet).not.toHaveBeenCalled();
    // Free text on both axes: the human can complete and save.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.model).toBe("claude-sonnet-4-5");
    expect(sent?.profiles[0]?.modeId).toBe("default");
  });

  it("shows the honest sentence for origin daemon exactly once, and not for origin provider", async () => {
    // Models are the daemon's own mapping; modes are the provider's own
    // answer. The honest sentence belongs to the first, only.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "daemon",
          items: [{ modelId: "opus", name: "Opus" }],
        },
        modes: {
          state: "present",
          origin: "provider",
          items: [{ id: "code", name: "Code" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(modelControl().tagName).toBe("SELECT");
    expect(modeControl().tagName).toBe("SELECT");
    const mentions = container.textContent?.match(/not something the provider published/g) ?? [];
    expect(mentions).toHaveLength(1);
    // The published items are offered as they arrived.
    expect(selectValues(modelControl())).toContain("opus");
    expect(selectValues(modeControl())).toContain("code");
  });

  it("never lets a vocabulary reply for the previously selected provider land in the form", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [
        makeProvider({ id: "a", executable: "C:\\cli\\a.cmd" }),
        makeProvider({ id: "b", executable: "C:\\cli\\b.cmd" }),
      ],
      unreadableDirs: 0,
    }));
    const resolvers = new Map<string, (reply: ProviderVocabulary) => void>();
    vi.mocked(providerVocabularyGet).mockImplementation((provider: string) => {
      return new Promise<ProviderVocabulary>((resolve) => {
        resolvers.set(provider, resolve);
      });
    });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);

    // The first provider's fetch is in flight when the human switches.
    expect(resolvers.has("a")).toBe(true);
    await typeText(providerField(), "b");
    await act(async () => undefined);
    expect(resolvers.has("b")).toBe(true);

    // The new provider answers.
    await act(async () => {
      resolvers.get("b")?.(
        makeVocabulary({
          provider: "b",
          models: {
            state: "present",
            origin: "provider",
            items: [{ modelId: "b-model", name: "Model B" }],
          },
        }),
      );
    });
    await act(async () => undefined);
    expect(selectValues(modelControl())).toContain("b-model");

    // Now the stale reply for the previous provider arrives.
    await act(async () => {
      resolvers.get("a")?.(
        makeVocabulary({
          provider: "a",
          models: {
            state: "present",
            origin: "provider",
            items: [{ modelId: "a-model", name: "Model A" }],
          },
        }),
      );
    });
    await act(async () => undefined);

    // The form shows provider b; the late answer for a must not have landed.
    expect(selectValues(modelControl())).toContain("b-model");
    expect(selectValues(modelControl())).not.toContain("a-model");
    expect(providerVocabularyGet).toHaveBeenNthCalledWith(1, "a", "", false);
    expect(providerVocabularyGet).toHaveBeenNthCalledWith(2, "b", "", false);
  });

  it("offers only installed providers in the picker", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [
        makeProvider({ id: "claude" }),
        makeProvider({ id: "codex", installed: false, protocol: null }),
      ],
      unreadableDirs: 0,
    }));
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();

    expect(selectValues(providerField())).toEqual(["claude"]);
  });

  it("refuses to save without a model and mode, then saves once both are chosen", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "opus", name: "Opus" }],
        },
        modes: {
          state: "present",
          origin: "provider",
          items: [{ id: "code", name: "Code" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    // The store's read-back after the (only) confirmed create, at the end.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [storedProfile("scout-1", { name: "Scout", model: "opus", modeId: "code" })],
        standingInstructions: "",
      },
    });
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // Name only; both selects still on their placeholder.
    await typeText(nameField(), "Scout");
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("model");

    await typeText(modelControl(), "opus");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("mode");

    await typeText(modeControl(), "code");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.model).toBe("opus");
    expect(sent?.profiles[0]?.modeId).toBe("code");
  });

  it("falls back to free text naming the failure when the vocabulary query rejects", async () => {
    vi.mocked(providerVocabularyGet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    // The store's read-back after the confirmed create, at the end.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The failure names its own reason — not the provider's "did not
    // publish", not the older-daemon sentence.
    expect(form().textContent).toContain(
      "The vocabulary query failed (A system or file operation failed on this machine.)",
    );
    expect(form().textContent).not.toContain("did not publish");
    expect(form().textContent).not.toContain("older than this app");
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");

    await typeText(nameField(), "Scout");
    await typeText(modelControl(), "whatever-the-human-knows");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("prefills the ACP mode suggestion labelled as a suggestion when the agent declares no modes", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [makeProvider({ id: "zed", protocol: "acp", executable: "C:\\cli\\zed.cmd" })],
      unreadableDirs: 0,
    }));
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        provider: "zed",
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "zed-model", name: "Zed model" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // Prefilled, and labelled a suggestion — never as something the
    // provider reported.
    expect((modeControl() as HTMLInputElement).value).toBe("default");
    expect(form().textContent).toContain("A suggestion, not something the provider reported");
    // The models axis here is present, so its absent sentence must not show.
    expect(form().textContent).not.toContain("did not publish its models");
  });

  it("ticks the row the human ticked once the daemon's minted ids are adopted", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after each confirmed create carries the ids the
    // daemon minted — the set reply names only the request.
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: {
          profiles: [storedProfile("minted-a", { name: "Alpha" })],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta", enabledForAgents: true }),
          ],
          standingInstructions: "",
        },
      });

    // Create Alpha, then Beta, letting each read-back land between them.
    await openForm();
    await typeText(nameField(), "Alpha");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    await openForm();
    await typeText(nameField(), "Beta");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(rowTicks()).toHaveLength(2);

    // The human ticks the second row (Beta).
    await tickCheckbox(rowTicks()[1]!, true);
    await act(async () => undefined);
    await act(async () => undefined);

    // The write flips exactly Beta, under the ids the daemon minted — never
    // the first empty-id row the panel used to mistake for it.
    const sent = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(sent?.profiles.map((profile) => [profile.id, profile.enabledForAgents])).toEqual([
      ["minted-a", false],
      ["minted-b", true],
    ]);
    expect(rowTicks()[0]?.checked).toBe(false);
    expect(rowTicks()[1]?.checked).toBe(true);
  });
  it("adopts the minted ids after every confirmed write, so no empty id is ever re-sent", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: {
          profiles: [storedProfile("minted-a", { name: "Alpha" })],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha", enabledForAgents: true }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      });

    await openForm();
    await typeText(nameField(), "Alpha");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    await openForm();
    await typeText(nameField(), "Beta");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The second create already travelled with the first profile's minted
    // id: the read-back after write one was adopted.
    const secondCreate = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(secondCreate?.profiles.map((profile) => profile.id)).toEqual(["minted-a", ""]);

    // Ticking Alpha re-sends the whole document: every id must be the
    // daemon's — an empty id would make the store mint yet another identity
    // for a row the human already created.
    await tickCheckbox(rowTicks()[0]!, true);
    await act(async () => undefined);
    await act(async () => undefined);

    const tickWrite = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(tickWrite?.profiles.map((profile) => profile.id)).toEqual(["minted-a", "minted-b"]);
    expect(tickWrite?.profiles.every((profile) => profile.id !== "")).toBe(true);
    expect(tickWrite?.profiles[0]?.enabledForAgents).toBe(true);
  });

  it("reverts exactly what the human was seeing when a write is refused, and adopts nothing", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" })],
      standingInstructions: "",
    });
    // The one read-back armed after the load is a reply that disagrees with
    // the revert — the error path must never touch it.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          makeProfile({ id: "x1", enabledForAgents: true, note: "adopted from the store" }),
        ],
        standingInstructions: "",
      },
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await act(async () => tickCheckbox(rowTick("Explorer"), true));
    await act(async () => undefined);
    await act(async () => undefined);

    // The row is exactly what the human was seeing before the click, and
    // the refusal is named.
    expect(rowTick("Explorer").checked).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    // A refused write re-reads nothing: a refusal adopts no reply.
    expect(agentProfilesGet).toHaveBeenCalledTimes(1);

    // The retry starts from the revert, not from any reply: it re-sends the
    // human's tick (the row went back to unchecked) over the document as it
    // stood — never the armed reply's note.
    await act(async () => tickCheckbox(rowTick("Explorer"), true));
    await act(async () => undefined);
    await act(async () => undefined);
    const retry = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(retry?.profiles[0]?.enabledForAgents).toBe(true);
    expect(retry?.profiles[0]?.note).toBe("Reads the code and reports back.");
  });

  it("names a malformed reply and keeps its usable models list instead of calling it a failed query", async () => {
    // No `modes` axis at all — a malformed reply; models arrived intact.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce({
      provider: "claude",
      models: {
        state: "present",
        origin: "provider",
        items: [{ modelId: "opus", name: "Opus" }],
      },
      source: "probe",
    } as unknown as ProviderVocabulary);
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The usable half is kept: a select over the models that arrived.
    expect(modelControl().tagName).toBe("SELECT");
    expect(selectValues(modelControl())).toContain("opus");
    // The missing half is named as its own state — malformed, which is not
    // a failed query and not `absent`.
    expect(modeControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("reply was malformed");
    expect(form().textContent).toContain("carried no modes axis");
    expect(form().textContent).not.toContain("The vocabulary query failed");
    expect(form().textContent).not.toContain("did not publish its modes");
    // And the malformed half did not take the models sentence with it.
    expect(form().textContent).not.toContain("carried no models axis");
  });

  it("names the contradiction when present arrives with an empty list, instead of an empty select", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "present", origin: "provider", items: [] },
        modes: { state: "absent", items: [] },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // `present` with no items is the reply the spec forbids: the form names
    // the contradiction and stays typeable rather than rendering a select
    // with nothing to select.
    expect(modelControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("listed none — a contradiction");
    expect(form().textContent).not.toContain("reports no models");
    expect(form().textContent).not.toContain("did not publish its models");
    // The modes axis in the same reply is a real `absent`.
    expect(modeControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("did not publish its modes");
  });

  it("names a state value it does not know instead of rendering a silent free-text field", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "expired", items: [] } as unknown as ProviderVocabulary["models"],
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(modelControl().tagName).toBe("INPUT");
    // The unknown value is shown as received, and none of the known states
    // is claimed for it — an unexplained field is the one dishonest answer.
    expect(form().textContent).toContain('a value this app does not know ("expired")');
    expect(form().textContent).not.toContain("did not publish its models");
    expect(form().textContent).not.toContain("reports no models");
    expect(form().textContent).not.toContain("reply was malformed");
  });

  it("renders a present list whose origin is undeclared, and says no author is declared", async () => {
    // `origin` omitted entirely — the type allows it at runtime, the spec
    // conditions it on `present`, and the daemon side will enforce it; this
    // is the side where an undeclared list must not read as a declared one.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "present", items: [{ modelId: "opus", name: "Opus" }] },
        modes: { state: "present", items: [{ id: "code", name: "Code" }] },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The items themselves are usable: a select, not free text.
    expect(modelControl().tagName).toBe("SELECT");
    expect(selectValues(modelControl())).toContain("opus");
    // The missing authorship is named on both axes. No sentence at all is
    // what the eye reads as "the provider published this" — the stronger
    // of the two authorships.
    expect(form().textContent).toContain("no author declared");
    expect(form().textContent).toContain("whether the provider published it");
    expect(form().textContent).not.toContain("not something the provider published");
  });

  it("saving after a provider switch sends no thinking option and keeps the stored features", async () => {
    // Two installed providers and no `provider_vocabulary` in the handshake:
    // the model and mode are free text, and the switch must still clear
    // everything that belonged to the old provider — and only that: a stored
    // feature key is the profile's own, whichever provider it runs on.
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "grok",
          executable: "C:\\cli\\grok.cmd",
          acpAvailable: true,
          authentication: "ok",
          protocol: "acp",
          origin: "user-binary",
          installed: true,
        },
        {
          id: "claude",
          executable: "C:\\cli\\claude.cmd",
          acpAvailable: false,
          authentication: "ok",
          protocol: "stream-json",
          origin: "user-binary",
          installed: true,
        },
      ],
      unreadableDirs: 0,
    });
    const stored = storedProfile("p-1", {
      name: "Explorer",
      provider: "grok",
      model: "grok-4",
      modeId: "reflect",
      thinkingOptionId: "high",
      features: { autoAccept: true, sandbox: "none" },
    });
    await renderAgentsPanel({ profiles: [stored], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          {
            ...stored,
            provider: "claude",
            model: "claude-opus-4-6",
            modeId: "plan",
            thinkingOptionId: null,
            features: { autoAccept: true, sandbox: "none" },
          },
        ],
        standingInstructions: "",
      },
    });

    const editor = await openRowEditor("Explorer");
    const providerSelect = editor.querySelector<HTMLSelectElement>('select[aria-label="Provider"]');
    if (!providerSelect) throw new Error("provider picker did not render");
    await typeText(providerSelect, "claude");
    await act(async () => undefined);

    const model = editor.querySelector<HTMLInputElement>('[aria-label="Model"]');
    const mode = editor.querySelector<HTMLInputElement>('[aria-label="Mode"]');
    const thinking = editor.querySelector<HTMLInputElement>('[aria-label="Thinking option"]');
    if (!model || !mode || !thinking) throw new Error("the cleared fields did not render");
    expect(thinking.value).toBe("");
    await typeText(model, "claude-opus-4-6");
    await typeText(mode, "plan");
    const save = Array.from(editor.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Save",
    );
    if (!save) throw new Error("the editor's Save button did not render");
    await act(async () => save.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.provider).toBe("claude");
    expect(sent.profiles[0]?.model).toBe("claude-opus-4-6");
    expect(sent.profiles[0]?.modeId).toBe("plan");
    // The old provider's thinking id is gone; the daemon's own tick and the
    // stored key a provider switch never touches both stay.
    expect(sent.profiles[0]?.thinkingOptionId).toBeNull();
    expect(sent.profiles[0]?.features).toEqual({ autoAccept: true, sandbox: "none" });
  });

  it("restores a provider's own fields when the human switches back before saving", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "grok",
          executable: "C:\\cli\\grok.cmd",
          acpAvailable: true,
          authentication: "ok",
          protocol: "acp",
          origin: "user-binary",
          installed: true,
        },
        {
          id: "claude",
          executable: "C:\\cli\\claude.cmd",
          acpAvailable: false,
          authentication: "ok",
          protocol: "stream-json",
          origin: "user-binary",
          installed: true,
        },
      ],
      unreadableDirs: 0,
    });
    const stored = storedProfile("p-1", {
      name: "Explorer",
      provider: "grok",
      model: "grok-4",
      modeId: "reflect",
      thinkingOptionId: "high",
      features: { autoAccept: true, sandbox: "none" },
    });
    await renderAgentsPanel({ profiles: [stored], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [stored], standingInstructions: "" },
    });

    const editor = await openRowEditor("Explorer");
    const providerSelect = editor.querySelector<HTMLSelectElement>('select[aria-label="Provider"]');
    if (!providerSelect) throw new Error("provider picker did not render");
    const field = (label: string) => {
      const control = editor.querySelector<HTMLInputElement>(`[aria-label="${label}"]`);
      if (!control) throw new Error(`${label} did not render`);
      return control;
    };
    expect(field("Model").value).toBe("grok-4");
    expect(field("Mode").value).toBe("reflect");
    expect(field("Thinking option").value).toBe("high");

    // A wrong pick: the new provider's own vocabulary is empty, not the old
    // provider's — but the stored feature key rides along untouched. It is not
    // drawn: this handshake advertises no `provider_vocabulary`, so nobody has
    // answered what either provider offers, and a form that guessed a control
    // for a key it cannot read would write a boolean over a value only the
    // provider can name. Carried, not shown, and not pruned — the save below
    // proves the carrying.
    await typeText(providerSelect, "claude");
    expect(field("Model").value).toBe("");
    expect(field("Mode").value).toBe("");
    expect(field("Thinking option").value).toBe("");

    // Text typed under the wrong provider is dropped with that provider's
    // own fields; switching back restores what grok held.
    await typeText(field("Model"), "claude-opus-4-6");
    await typeText(providerSelect, "grok");
    expect(field("Model").value).toBe("grok-4");
    expect(field("Mode").value).toBe("reflect");
    expect(field("Thinking option").value).toBe("high");

    const save = Array.from(editor.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Save",
    );
    if (!save) throw new Error("the editor's Save button did not render");
    await act(async () => save.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    // The round trip: switch away, type, switch back, save — the stored row
    // is what goes out, stored features included.
    expect(sent.profiles[0]).toEqual(stored);
  });

  it("gives every state its own sentence: no two rendered sentences are equal or substrings", async () => {
    // The property the sentences exist for, held over the render itself:
    // every sentence-bearing state the Agents panel can reach is rendered
    // here — the vocabulary states, the caps and their refusals, the load
    // errors, the catalog states, and the standing panel copy — and every
    // rendered sentence is compared with every other. Equal is a collapse,
    // and a substring is a collapse waiting for its neighbouring words to
    // change. An earlier version collected only the vocabulary hints inside
    // the new-profile form; bdf0318's claim to render "every
    // sentence-bearing state" was wider than that net, and this is the net
    // sized to the claim.
    const scenarioNames: string[] = [];
    const sentences: string[] = [];
    // A sentence already collected from an earlier state is the same
    // sentence: it enters the net once.
    const seen = new Set<string>();

    // Sentence-bearing elements only: labels, buttons, row titles and
    // option texts are not sentences. An element that contains another
    // collected element (the off-switch wrapper around its two paragraphs,
    // a role=status wrapper) is dropped — its text would falsely "contain"
    // the real sentences inside it.
    const SENTENCE_SELECTOR = [
      ".settings-page-heading p",
      ".device-field-hint",
      ".device-copy",
      ".agent-profile-tick-note",
      ".agent-profiles-off p",
      ".agent-profile-note-empty",
      ".agent-standing .agent-byte-counter",
      "[role='alert']",
      "[role='status']",
    ].join(",");

    async function collectScenario(name: string) {
      const panel = container.querySelector("#settings-panel-agents");
      if (!panel) throw new Error("agents panel did not render");
      const elements = Array.from(panel.querySelectorAll<HTMLElement>(SENTENCE_SELECTOR));
      const leaves = elements.filter(
        (element) => !elements.some((other) => other !== element && element.contains(other)),
      );
      for (const element of leaves) {
        let text = (element.textContent ?? "").replace(/\s+/g, " ").trim();
        // The standing counter's leading numbers are data, not copy, and
        // data prefixes manufacture fake containments ("8400 / 8192…"
        // contains "0 / 8192…"): compare the copy, tokenise the numbers.
        if (element.classList.contains("agent-byte-counter")) {
          text = text.replace(/^\d+ \/ \d+ bytes/, "N / M bytes");
        }
        if (text === "" || seen.has(text)) continue;
        seen.add(text);
        scenarioNames.push(name);
        sentences.push(text);
      }
      // A fresh mount for the next scenario.
      if (root !== undefined) {
        await act(async () => root!.unmount());
        root = undefined;
      }
      container.innerHTML = "";
    }

    function agentsSectionButton(text: string): HTMLButtonElement {
      const button = Array.from(
        container.querySelectorAll<HTMLButtonElement>("#settings-panel-agents button"),
      ).find((candidate) => candidate.textContent === text);
      if (!button) throw new Error(`button ${text} did not render`);
      return button;
    }

    function agentRow(name: string): HTMLElement {
      const row = Array.from(container.querySelectorAll<HTMLElement>(".agent-profile-row")).find(
        (candidate) => candidate.textContent?.includes(name),
      );
      if (!row) throw new Error(`profile row ${name} did not render`);
      return row;
    }

    async function openEditorOn(name: string) {
      const edit = Array.from(agentRow(name).querySelectorAll<HTMLButtonElement>("button")).find(
        (candidate) => candidate.textContent === "Edit",
      );
      if (!edit) throw new Error(`Edit button on ${name} did not render`);
      await act(async () => edit.click());
      await act(async () => undefined);
    }

    async function armAndOpen(reply: ProviderVocabulary | undefined) {
      if (reply !== undefined) {
        vi.mocked(providerVocabularyGet).mockResolvedValueOnce(reply);
      }
      await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
      await openForm();
      await act(async () => undefined);
      await act(async () => undefined);
    }

    // 1. Older daemon: no query is sent, the sentence is there at once.
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await collectScenario("older daemon");

    // 2. The query itself fails.
    vi.mocked(providerVocabularyGet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);
    await collectScenario("query failed");

    // 3. `none` on both axes: the provider answered "I have none".
    await armAndOpen(
      makeVocabulary({ models: { state: "none", items: [] }, modes: { state: "none", items: [] } }),
    );
    await collectScenario("none");

    // 4. `absent` on both axes: no source could answer.
    await armAndOpen(makeVocabulary());
    await collectScenario("absent");

    // 5. present with origin daemon on both axes.
    await armAndOpen(
      makeVocabulary({
        models: {
          state: "present",
          origin: "daemon",
          items: [{ modelId: "opus", name: "Opus" }],
        },
        modes: { state: "present", origin: "daemon", items: [{ id: "code", name: "Code" }] },
      }),
    );
    await collectScenario("daemon origin");

    // 5b. present with the origin left undeclared on both axes: the items
    // are still offered, and the missing authorship is named.
    await armAndOpen(
      makeVocabulary({
        models: { state: "present", items: [{ modelId: "opus", name: "Opus" }] },
        modes: { state: "present", items: [{ id: "code", name: "Code" }] },
      }),
    );
    await collectScenario("origin undeclared");

    // 6. Malformed: the reply arrived, neither axis did.
    await armAndOpen({ provider: "claude", source: "probe" } as unknown as ProviderVocabulary);
    await collectScenario("malformed");

    // 7. present with empty items on both axes: the forbidden contradiction.
    await armAndOpen(
      makeVocabulary({
        models: { state: "present", origin: "provider", items: [] },
        modes: { state: "present", origin: "provider", items: [] },
      }),
    );
    await collectScenario("present empty");

    // 8. A state value outside the union on both axes.
    await armAndOpen(
      makeVocabulary({
        models: { state: "expired", items: [] } as unknown as ProviderVocabulary["models"],
        modes: { state: "expired", items: [] } as unknown as ProviderVocabulary["modes"],
      }),
    );
    await collectScenario("unknown state");

    // 9. The ACP mode suggestion, labelled a suggestion. Two catalog
    // answers are queued because two panels fetch on mount: the default
    // ProvidersPanel tab consumes the first, the Agents panel's picker the
    // second — the form's provider must be the ACP one.
    const zedCatalog = {
      providers: [makeProvider({ id: "zed", protocol: "acp", executable: "C:\\cli\\zed.cmd" })],
      unreadableDirs: 0,
    };
    vi.mocked(providersList).mockResolvedValueOnce(zedCatalog).mockResolvedValueOnce(zedCatalog);
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        provider: "zed",
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "zed-model", name: "Zed model" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);
    await collectScenario("ACP suggestion");

    // 10. The vocabulary ask still in flight.
    vi.mocked(providerVocabularyGet).mockReturnValueOnce(
      new Promise<ProviderVocabulary>(() => undefined),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await collectScenario("vocabulary in flight");

    // 11. The panel load failed: the daemon's sentence and a Retry. The code
    // is `internal` so this scenario's sentence stays distinct from the io
    // ones in the uniqueness net below.
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(OLDER_DAEMON));
    vi.mocked(agentProfilesGet).mockRejectedValueOnce({
      code: "internal",
      message: "the store is unreachable",
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const failedTab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!failedTab) throw new Error("Agents tab did not render");
    await act(async () => failedTab.click());
    await act(async () => undefined);
    await collectScenario("load failed");

    // 12. The panel load still in flight.
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(OLDER_DAEMON));
    vi.mocked(agentProfilesGet).mockImplementationOnce(
      () => new Promise<AgentProfilesReply>(() => undefined),
    );
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const loadingTab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!loadingTab) throw new Error("Agents tab did not render");
    await act(async () => loadingTab.click());
    await act(async () => undefined);
    await collectScenario("loading");

    // 13. The off switch, with a note-less row.
    await renderAgentsPanel({
      profiles: [makeProfile({ note: "" }), makeProfile({ id: "x2", name: "Coder", note: "" })],
      standingInstructions: "",
    });
    await collectScenario("off switch");

    // 14. A delete armed: the inline confirm's copy.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await act(async () => agentsSectionButton("Delete").click());
    await act(async () => undefined);
    await collectScenario("delete armed");

    // 15. The row editor open: its not-editable-here hint.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    await collectScenario("editor open");

    // 16. The name-cap refusal.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    const editorName = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!editorName) throw new Error("editor name field did not render");
    await typeText(editorName, "🦄".repeat(61));
    await act(async () => agentsSectionButton("Save").click());
    await act(async () => undefined);
    await collectScenario("name cap refusal");

    // 17. The note-cap refusal.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    const editorNote = container.querySelector<HTMLTextAreaElement>(
      ".agent-inline-editor textarea",
    );
    if (!editorNote) throw new Error("editor note field did not render");
    await typeText(editorNote, "é".repeat(1100));
    await act(async () => agentsSectionButton("Save").click());
    await act(async () => undefined);
    await collectScenario("note cap refusal");

    // 18. The standing-instructions cap refusal.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    const standingField = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!standingField) throw new Error("standing instructions field did not render");
    await typeText(standingField, "é".repeat(4200));
    await act(async () => agentsSectionButton("Save standing instructions").click());
    await act(async () => undefined);
    await collectScenario("standing cap refusal");

    // 19. The create form refusing a missing model.
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await typeText(nameField(), "Scout");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await collectScenario("model missing refusal");

    // 20. The create form refusing a missing mode.
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await typeText(nameField(), "Scout");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await collectScenario("mode missing refusal");

    // 21. The create-time profile-cap refusal: the store reaches the cap
    // while the form is open (the read-back of an unrelated write adopts a
    // 64-row store), so the guard under the Create button is what speaks.
    const sixtyThree = Array.from({ length: 63 }, (_, index) =>
      makeProfile({ id: `p-${index}`, name: `P ${index}` }),
    );
    await renderAgentsPanel({ profiles: sixtyThree, standingInstructions: "" });
    await openForm();
    await typeText(nameField(), "Gamma");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [...sixtyThree, storedProfile("minted-cap")],
        standingInstructions: "",
      },
    });
    await tickCheckbox(rowTicks()[0]!, true);
    await act(async () => undefined);
    await act(async () => undefined);
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);
    await collectScenario("profile cap refusal");

    // 22. The store at the cap: the hint that names it before any typing.
    const full = Array.from({ length: 64 }, (_, index) =>
      makeProfile({ id: `c-${index}`, name: `C ${index}` }),
    );
    await renderAgentsPanel({ profiles: full, standingInstructions: "" });
    await collectScenario("at cap");

    // 23. The catalog read and found empty: the only state allowed to say
    // no agent CLI is installed.
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [], unreadableDirs: 0 });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await collectScenario("catalog empty");

    // 24. The catalog read failed: it names the failure, never emptiness.
    vi.mocked(providersList).mockRejectedValueOnce({ code: "io", message: "the scan failed" });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await collectScenario("catalog failed");

    // The one declared duplicate: the tick note exists in the form and on
    // the row — the same control in two places, so identical is right — and
    // this assertion is what holds them equal, so an edit to either is
    // loud instead of a silent parting.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openForm();
    const formAvailableNote = Array.from(
      form().querySelectorAll<HTMLElement>(".agent-profile-tick-note"),
    ).find((note) => note.textContent?.startsWith("Lets an agent start"));
    const rowTickNote = container.querySelector<HTMLElement>(
      ".agent-profile-row .agent-profile-tick-note",
    );
    if (!formAvailableNote || !rowTickNote) throw new Error("tick notes did not render");
    const normalize = (text: string) => text.replace(/\s+/g, " ").trim();
    expect(normalize(formAvailableNote.textContent ?? "")).toBe(
      normalize(rowTickNote.textContent ?? ""),
    );
    await collectScenario("tick note pin");

    // The count is part of the net: a scenario that stops rendering its
    // sentence, or a new sentence nobody rendered here, moves this number.
    // Forty-seven: the delegation section's one sentence on this panel (an
    // older daemon's named absence — the switch itself is gated harder and
    // only renders when the handshake advertises permission_delegation), the
    // fifteen vocabulary sentences, the ACP suggestion
    // and the in-flight ask, the load-failed and loading sentences, the
    // off-switch pair and the no-note sentence, the delete-confirm copy,
    // the editor's two hints (when the spawn prompt is sent, and that running
    // agents keep what they started with), the thinking option's own hint,
    // the one feature-list sentence (the provider answered and offered
    // nothing; the read-only rows and their "saved but not delivered"
    // sentence went with the D4 prune, and the ACP cold start renders a
    // role=status ask that the in-flight ask below already collects),
    // the overlay add
    // control's own sentence, the three cap refusals, the model/mode
    // refusals, the two profile-cap sentences, the two catalog sentences,
    // the idle-close field's own hint and the off toggle's note, the heading
    // description, the intro copy, the tick notes (including the
    // open-editor clause on the row tick), and the standing
    // copy with its counter (whose numbers are tokenised, so every scenario
    // renders it into one net entry), and the standing box's keep-it-short
    // hint under its textarea. A new sentence that does not come
    // through a scenario here moves this number; so does a sentence a
    // scenario stopped rendering.
    expect(sentences).toHaveLength(47);
    for (let i = 0; i < sentences.length; i++) {
      for (let j = i + 1; j < sentences.length; j++) {
        const a = sentences[i]!;
        const b = sentences[j]!;
        expect(
          a === b,
          `${scenarioNames[i]} and ${scenarioNames[j]} render the same sentence`,
        ).toBe(false);
        expect(
          a.includes(b),
          `${scenarioNames[i]} sentence contains the ${scenarioNames[j]} sentence: "${b}" inside "${a}"`,
        ).toBe(false);
        expect(
          b.includes(a),
          `${scenarioNames[j]} sentence contains the ${scenarioNames[i]} sentence: "${a}" inside "${b}"`,
        ).toBe(false);
      }
    }
  });

  it("does not claim no agent CLI is installed when the catalog read failed", async () => {
    // Every providers_list caller is refused: the catalog was never read.
    vi.mocked(providersList).mockRejectedValue({ code: "io", message: "the scan failed" });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();

    // The failed read names itself; the empty-catalog claim is not made.
    expect(form().textContent).toContain(
      "could not be read: A system or file operation failed on this machine.",
    );
    expect(form().textContent).not.toContain("No agent CLI is installed");
    // The picker does not pretend the (unread) catalog was read either: its
    // one option names the failed read, not an empty result. (Placeholder
    // options carry value="", so the option text is what is asserted.)
    const options = Array.from(providerField().options).map((option) => option.textContent);
    expect(options).toEqual(["The catalog could not be read"]);
  });

  it("keeps the draft on screen under its error when the create is refused", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "the document holds 65 profiles, over the 64-profile cap",
    });

    await openForm();
    await typeText(nameField(), "Gamma");
    await typeText(noteField(), "Checks the build output.");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The refusal is shown, the form still stands, and every field keeps
    // what the human typed into it.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    expect(nameField().value).toBe("Gamma");
    expect(noteField().value).toBe("Checks the build output.");
    expect((modelControl() as HTMLInputElement).value).toBe("claude-sonnet-4-5");
    expect((modeControl() as HTMLInputElement).value).toBe("default");

    // The same draft is what the retry sends once the daemon takes it — and
    // only confirmation closes the form.
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const sent = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(sent?.profiles.at(-1)?.name).toBe("Gamma");
    expect(sent?.profiles.at(-1)?.note).toBe("Checks the build output.");
    expect(container.querySelector(".agent-profile-create")).toBeNull();
  });

  it("mirrors the store's profile cap and does not offer the form at it", async () => {
    const full = Array.from({ length: 64 }, (_, index) =>
      makeProfile({ id: `p-${index}`, name: `Profile ${index}` }),
    );
    await renderAgentsPanel({ profiles: full, standingInstructions: "" });

    // The cap is named before the human fills anything in...
    expect(container.textContent).toContain("the maximum of 64 profiles");
    // ...and the form cannot be opened: a 65th creation is refused by the
    // store, so the panel does not offer the work.
    const open = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".agent-profile-create-row button"),
    ).find((candidate) => candidate.textContent === "New profile");
    expect(open?.disabled).toBe(true);
    await act(async () => open?.click());
    await act(async () => undefined);
    expect(container.querySelector(".agent-profile-create")).toBeNull();
  });
});
