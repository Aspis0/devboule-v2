// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const { agentsPanelTauriMock } = await import("./agentsPanelTestMocks");
  return agentsPanelTauriMock(await importOriginal());
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

import {
  agentProfilesGet,
  agentProfilesSet,
  daemonStatus,
  providerVocabularyGet,
  providersList,
} from "../../../lib/tauri";
import type { AgentProfilesReply, ProviderVocabulary } from "../../../types/ipc";
import { AgentProfilesPanel } from "./AgentsPanel";
import { SettingsSurface } from "../SettingsSurface";
import {
  dom,
  useAgentsPanelDom,
  OLDER_DAEMON,
  VOCABULARY_DAEMON,
  daemonStatusWith,
  makeProfile,
  makeProvider,
  makeVocabulary,
  storedProfile,
  renderAgentsPanel,
  typeText,
} from "./agentsPanelTestHarness";
import {
  openForm,
  nameField,
  modelControl,
  modeControl,
  createButton,
} from "./agentsPanelTestQueries";

describe("Settings agents panel — sentence uniqueness", () => {
  // The profile fixtures name `grok`, so it is installed here: an edit of a
  // profile whose provider is not installed is a different state, locked.
  useAgentsPanelDom(() => [makeProvider(), makeProvider({ id: "grok" })]);

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
    // collected element (a role=status wrapper) is dropped — its text
    // would falsely "contain" the real sentences inside it.
    const SENTENCE_SELECTOR = [
      ".device-field-hint",
      ".device-copy",
      ".agent-profile-tick-note",
      ".agent-byte-counter",
      "[role='alert']",
      "[role='status']",
    ].join(",");

    async function collectScenario(name: string) {
      const panel = dom.container.querySelector("#settings-panel-agents");
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
      if (dom.root !== undefined) {
        await act(async () => dom.root!.unmount());
        dom.root = undefined;
      }
      dom.container.innerHTML = "";
    }

    function agentsSectionButton(text: string): HTMLButtonElement {
      const button = Array.from(
        dom.container.querySelectorAll<HTMLButtonElement>("#settings-panel-agents button"),
      ).find((candidate) => candidate.textContent === text);
      if (!button) throw new Error(`button ${text} did not render`);
      return button;
    }

    function agentRow(name: string): HTMLElement {
      const row = Array.from(
        dom.container.querySelectorAll<HTMLElement>(".agent-profile-row"),
      ).find((candidate) => candidate.querySelector(".profile-name")?.textContent === name);
      if (!row) throw new Error(`profile row ${name} did not render`);
      return row;
    }

    async function openEditorOn(name: string) {
      const edit = agentRow(name).querySelector<HTMLButtonElement>(
        `button[aria-label="Edit ${name}"]`,
      );
      if (!edit) throw new Error(`Edit button on ${name} did not render`);
      await act(async () => edit.click());
      await act(async () => undefined);
    }

    async function openAdvanced() {
      const advanced = Array.from(
        dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
      ).find((button) => button.textContent === "Advanced");
      if (!advanced) throw new Error("Advanced section did not render");
      await act(async () => advanced.click());
      await act(async () => undefined);
    }

    async function openStandingEditor() {
      const edit = Array.from(
        dom.container.querySelectorAll<HTMLButtonElement>("[data-settings-row] button"),
      ).find((button) => button.textContent === "Edit");
      if (!edit) throw new Error("standing instructions Edit row did not render");
      await act(async () => edit.click());
      await act(async () => undefined);
      const box = dom.container.querySelector<HTMLTextAreaElement>(
        '[aria-label="Standing instructions for every agent"]',
      );
      if (!box) throw new Error("standing instructions editor did not render");
      return box;
    }

    /** An unrelated panel write for the cap scenarios: the first live move. */
    async function tickMoveFirstRow() {
      const down = dom.container.querySelector<HTMLButtonElement>(
        '.agent-profile-row button[aria-label$=" down"]',
      );
      if (!down || down.disabled) throw new Error("live move button did not render");
      await act(async () => down.click());
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
    dom.root = createRoot(dom.container);
    await act(async () => dom.root!.render(<AgentProfilesPanel />));
    await act(async () => undefined);
    await act(async () => undefined);
    await collectScenario("load failed");

    // 12. The panel load still in flight.
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(OLDER_DAEMON));
    vi.mocked(agentProfilesGet).mockImplementationOnce(
      () => new Promise<AgentProfilesReply>(() => undefined),
    );
    dom.root = createRoot(dom.container);
    await act(async () => dom.root!.render(<AgentProfilesPanel />));
    await act(async () => undefined);
    await act(async () => undefined);
    await collectScenario("loading");

    // 13. Unticked profiles add no sentence: the old warning box is gone.
    // Its absence is the assertion — the net collects nothing here.
    await renderAgentsPanel({
      profiles: [makeProfile({ note: "" }), makeProfile({ id: "x2", name: "Coder", note: "" })],
      standingInstructions: "",
    });
    expect(dom.container.querySelector(".agent-profiles-off")).toBeNull();
    await collectScenario("no warning box");

    // 14. A delete armed: the inline confirm's copy.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    const trash = dom.container.querySelector<HTMLButtonElement>(
      '.agent-profile-row button[aria-label="Delete Explorer"]',
    );
    if (!trash) throw new Error("row trash button did not render");
    await act(async () => trash.click());
    await act(async () => undefined);
    await collectScenario("delete armed");

    // 15. The row editor open: its not-editable-here hint.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    await collectScenario("editor open");

    // 15b. The discard check armed: Escape on a dirty dialog.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    const discardName = dom.container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!discardName) throw new Error("editor name field did not render");
    await typeText(discardName, "Scout");
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    await collectScenario("discard armed");

    // 15c. Older stored denials: the quiet line under Advanced, not a control.
    await renderAgentsPanel({
      profiles: [
        makeProfile({
          toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
        }),
      ],
      standingInstructions: "",
    });
    await openEditorOn("Explorer");
    await openAdvanced();
    await collectScenario("legacy denials");

    // 15c2. Advanced open on an older daemon: the icon, effort and idle
    // hints plus the auto-accept note.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    await openAdvanced();
    await collectScenario("advanced open");

    // 15d. A save in flight: the honest exit while the write runs. The
    // write stays pending past the collect (the busy-lock test's shape) —
    // the pane unmounts under it without settling anything.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    const flightName = dom.container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!flightName) throw new Error("editor name field did not render");
    await typeText(flightName, "Scout");
    vi.mocked(agentProfilesSet).mockImplementationOnce(() => new Promise<void>(() => undefined));
    await act(async () => agentsSectionButton("Save").click());
    await act(async () => undefined);
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    await collectScenario("save in flight");

    // 16. The name-cap refusal.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    const editorName = dom.container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!editorName) throw new Error("editor name field did not render");
    await typeText(editorName, "🦄".repeat(61));
    await act(async () => agentsSectionButton("Save").click());
    await act(async () => undefined);
    await collectScenario("name cap refusal");

    // 17. The note-cap refusal.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    const editorNote = dom.container.querySelector<HTMLTextAreaElement>(
      ".agent-inline-editor textarea",
    );
    if (!editorNote) throw new Error("editor note field did not render");
    await typeText(editorNote, "é".repeat(1100));
    await act(async () => agentsSectionButton("Save").click());
    await act(async () => undefined);
    await collectScenario("note cap refusal");

    // 18. The standing-instructions cap refusal, from the row's editor.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    const standingField = await openStandingEditor();
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
    await tickMoveFirstRow();
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

    // 25. The shell header above the panel: the profiles page carries a
    // title and no paragraph. Rendered through the surface so the net
    // covers the title where users actually read it — and pins that no
    // intro paragraph rides with it.
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
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [], standingInstructions: "" },
    });
    dom.root = createRoot(dom.container);
    await act(async () => dom.root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const profilesRow = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>("button"),
    ).find((candidate) => candidate.textContent?.trim() === "Agent profiles");
    if (!profilesRow) throw new Error("Agent profiles row did not render");
    await act(async () => profilesRow.click());
    await act(async () => undefined);
    const content = dom.container.querySelector("[data-settings-content]");
    if (!content) throw new Error("settings content did not render");
    expect(content.querySelector(".settings-page-intro")).toBeNull();
    expect(content.querySelector(".settings-page-title")?.textContent).toBe("Agent profiles");
    await act(async () => dom.root!.unmount());
    dom.root = undefined;
    dom.container.innerHTML = "";

    // The count is part of the net: a scenario that stops rendering its
    // sentence, or a new sentence nobody rendered here, moves this number.
    // Forty: the delegation section's one sentence on this panel (an
    // older daemon's named absence), the vocabulary sentences, the ACP
    // suggestion and the in-flight ask, the load-failed and loading
    // sentences, the delete-confirm copy, the effort field's own hint, the
    // feature-list sentences, the three cap refusals, the model/mode
    // refusals, the two profile-cap sentences, the two catalog sentences,
    // the discard check's sentence, the legacy-denials line, the
    // save-in-flight sentence, the empty-list line, and the standing
    // refusal. Advanced carries labels only, so the icon and idle hints
    // and the tick notes enter nothing. A new sentence that does not come
    // through a scenario here moves this number; so does a sentence a
    // scenario stopped rendering.
    expect(sentences).toHaveLength(36);
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
    // One test renders every panel state: ~2.5 s alone, past 5 s under a full parallel run.
  }, 30_000);
});
