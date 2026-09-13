import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import {
  agentProfilesGet,
  agentProfilesSet,
  projectsList,
  providerUpdate,
  providersList,
  providersRefresh,
  reasonFromCause,
  toolPolicyGet,
  toolPolicySet,
  workspacesList,
} from "../../lib/tauri";
import { DiagnosticsPanel } from "./DiagnosticsPanel";
import { DevicesPanel } from "./DevicesPanel";
import type {
  AgentProfile,
  AgentProfilesDocument,
  Project,
  ProviderCatalog,
  ProviderInfo,
  ToolPolicyEntry,
  Workspace,
} from "../../types/ipc";
import { OraclePanel } from "../oracle/OraclePanel";
import { useWorkspaceDaemon } from "../workspace/workspaceDaemon";
import { JournalRetentionPanel } from "./JournalRetentionPanel";
import { NewProjectDialog } from "../../components/NewProjectDialog";
import "./settings.css";

export type SettingsTab =
  | "general"
  | "projects"
  | "oracle"
  | "providers"
  | "agents"
  | "devices"
  | "diagnostics";

// Real navigation for the Settings surface, not a mock: add or remove an
// entry here when a tab comes or goes.
export const SETTINGS_TABS: readonly { id: SettingsTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "projects", label: "Projects" },
  { id: "oracle", label: "Oracle" },
  { id: "providers", label: "Providers & models" },
  { id: "agents", label: "Agents" },
  { id: "devices", label: "Devices" },
  { id: "diagnostics", label: "Diagnostics" },
];

export function SettingsSurface() {
  const [activeTab, setActiveTab] = useState<SettingsTab>("providers");

  const settingsTabs = SETTINGS_TABS.map((tab) => ({
    ...tab,
    active: activeTab === tab.id,
  }));
  function handleTabKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    const currentIndex = SETTINGS_TABS.findIndex((tab) => tab.id === activeTab);
    let nextIndex = currentIndex;

    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      nextIndex = (currentIndex + 1) % SETTINGS_TABS.length;
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      nextIndex = (currentIndex - 1 + SETTINGS_TABS.length) % SETTINGS_TABS.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = SETTINGS_TABS.length - 1;
    } else {
      return;
    }

    event.preventDefault();
    setActiveTab(SETTINGS_TABS[nextIndex].id);
  }

  function renderActivePanel() {
    switch (activeTab) {
      case "providers":
        return <ProvidersPanel />;
      case "oracle":
        return (
          <div id="settings-panel-oracle" role="tabpanel" aria-label="Oracle administration">
            <OraclePanel />
          </div>
        );
      case "projects":
        return <ProjectsPanel />;
      case "agents":
        return <AgentProfilesPanel />;
      case "devices":
        return <DevicesPanel />;
      case "general":
        return <GeneralPanel />;
      case "diagnostics":
        return <DiagnosticsPanel />;
    }
  }

  return (
    <section className="surface-card settings-surface" aria-labelledby="settings-title">
      <header className="settings-header">
        <div className="settings-header-title">
          <h1 id="settings-title">Settings</h1>
          <span className="settings-header-divider" aria-hidden="true" />
          <span className="settings-eyebrow">devboule 2.0 · rust · tauri shell</span>
        </div>
      </header>

      <div className="settings-tab-bar" role="tablist" aria-label="Settings sections">
        {settingsTabs.map((tab) => (
          <button
            className={`settings-section-tab${tab.active ? " settings-section-tab-active" : ""}`}
            type="button"
            role="tab"
            aria-selected={tab.active}
            aria-controls={`settings-panel-${tab.id}`}
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            onKeyDown={handleTabKeyDown}
          >
            {tab.label}
          </button>
        ))}
      </div>

      <div className="settings-content settings-scroll">
        <div className="settings-content-inner">{renderActivePanel()}</div>
      </div>
    </section>
  );
}

interface SettingsHeadingProps {
  title: string;
  description?: string;
}

export function SettingsHeading({ title, description }: SettingsHeadingProps) {
  return (
    <div className="settings-page-heading">
      <h2>{title}</h2>
      {description && <p>{description}</p>}
    </div>
  );
}

/** Status label for one provider, derived from the daemon's measured authentication. */
function providerStatusText(provider: ProviderInfo): string {
  const viaNpx = provider.origin === "npx-wrapper" ? "available via npx" : "installed";
  if (provider.authentication === "ok") return `${viaNpx} · last start ok`;
  if (provider.authentication.startsWith("failed:")) {
    const reason = provider.authentication.slice("failed:".length).trim();
    return reason.length > 0 ? `start failed — ${reason}` : "start failed";
  }
  return `${viaNpx} · authentication unknown`;
}

/**
 * Version segments for one provider card, in display order, or an empty list
 * when nothing is known. The agent segment is only kept when it disagrees with
 * the installed CLI version (or none is installed); otherwise it is noise.
 * Each segment carries its own tooltip; the render maps without re-deriving.
 */
interface ProviderVersionSegment {
  text: string;
  /** Hover explanation; absent when the text speaks for itself. */
  title?: string;
}

const AGENT_VERSION_TITLE =
  "Version the running agent adapter reported during its last live handshake; it may differ from the installed CLI version.";

const LATEST_VERSION_TITLE =
  "Latest known version from the last registry check; Refresh revalidates.";

function providerVersionSegments(provider: ProviderInfo): ProviderVersionSegment[] {
  // The daemon may send empty strings in place of absent versions; treat both
  // as "unknown" so "" never half-triggers a branch.
  const installed = provider.installedVersion || undefined;
  const latest = provider.latestVersion || undefined;
  const agent = provider.agentVersion || undefined;
  const segments: ProviderVersionSegment[] = [];
  if (installed) {
    segments.push({ text: `v${installed}` });
    if (latest && latest !== installed) {
      segments.push({ text: `v${latest} available`, title: LATEST_VERSION_TITLE });
    } else if (latest) {
      segments.push({ text: "up to date", title: LATEST_VERSION_TITLE });
    }
  } else if (latest) {
    segments.push({
      text:
        provider.installChannel === "npx-registry" ? `v${latest} via npx` : `v${latest} available`,
      title: LATEST_VERSION_TITLE,
    });
  }
  if (agent && agent !== installed) {
    segments.push({ text: `agent reports v${agent}`, title: AGENT_VERSION_TITLE });
  }
  return segments;
}

/** Muted version line under the executable path; renders nothing without data. */
function ProviderVersionLine({ provider }: { provider: ProviderInfo }) {
  const segments = providerVersionSegments(provider);
  if (segments.length === 0) return null;
  return (
    <span className="provider-version">
      {segments.map((segment, index) => (
        <span key={segment.text} title={segment.title}>
          {index > 0 ? " · " : ""}
          {segment.text}
        </span>
      ))}
    </span>
  );
}

/** A pending npm run on one provider card: what the daemon is doing right now. */
interface ProviderNpmRun {
  providerId: string;
  verb: "update" | "install";
}

/** A provider held open in the consent panel, waiting for the user's Confirm. */
interface ProviderConsent {
  provider: ProviderInfo;
  verb: "update" | "install";
}

/**
 * Update applies only to npm-installed CLIs whose package is known and whose
 * latest version differs from the installed one.
 */
function providerCanUpdate(provider: ProviderInfo): boolean {
  if (provider.installChannel !== "npm") return false;
  if (!provider.npmPackage || !provider.latestVersion) return false;
  return provider.latestVersion !== provider.installedVersion;
}

/** Last 500 characters of an npm log; the head is noise for a failed install. */
function logTail(log: string): string {
  return log.length > 500 ? log.slice(-500) : log;
}

/** The tool the daemon never gates: disabling it would hide the agent roster. */
export const ALWAYS_ON_TOOL = "devboule_list_agents";

/** One-line reason shown next to the always-on tool's disabled switch. */
export const ALWAYS_ON_REASON = "Always on: sessions need the agent roster.";

/**
 * The handshake capability that gates every tool-policy RPC. It is advertised
 * beside `devices`, and it is deliberately spelled exactly like the daemon's
 * own name for it. A daemon that does not advertise it cannot answer
 * `tool_policy_get`, so the toggles are not drawn and no request is sent.
 */
export const TOOL_POLICY_CAPABILITY = "tool_policy";

/**
 * What one provider's toggles read from a stored row. `undefined` is the
 * same as enabled: `ToolPolicyGet` returns stored rows only, so a provider
 * with no row is enabled by default — never an error, never "unknown".
 */
export function toolPolicyFor(
  providerId: string,
  policies: readonly ToolPolicyEntry[] | null,
): { enabled: boolean; disabledTools: readonly string[] } {
  const row = policies?.find((entry) => entry.providerId === providerId);
  if (row === undefined) return { enabled: true, disabledTools: [] };
  return {
    enabled: row.enabled !== false,
    // A stored row that names the always-on tool is stale daemon data:
    // the daemon never gates it, so the panel drops it on read and never
    // sends it back (persist strips again as the wire choke point).
    disabledTools: (row.disabledTools ?? []).filter((name) => name !== ALWAYS_ON_TOOL),
  };
}

/**
 * Per-provider tool toggles, under one provider card. Renders nothing when
 * `provider.tools` is empty: the daemon sends the `tools` key only for the
 * four native MCP-capable providers, and an empty list means there is
 * nothing to toggle. It renders nothing either when the handshake did not
 * negotiate [`TOOL_POLICY_CAPABILITY`], so a daemon that cannot answer
 * `tool_policy_get` is never asked — the section is absent, not broken.
 *
 * The always-on tool stays checked and disabled with its one-line reason.
 * Every other change applies optimistically and reverts on rejection; the
 * daemon's own sentence is shown verbatim inside the card.
 */
function ProviderToolSettings({
  provider,
  toolPolicySupported,
}: {
  provider: ProviderInfo;
  /** True only when the handshake advertised `tool_policy`. */
  toolPolicySupported: boolean;
}) {
  const tools = provider.tools ?? [];
  const [policies, setPolicies] = useState<readonly ToolPolicyEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Synchronous mirror of `policies`. It — never the render closure — is
  // what a second rapid write reads and the base its revert applies to
  // (audit findings 1, 8).
  const policiesRef = useRef<readonly ToolPolicyEntry[] | null>(null);
  // Monotonic write sequence: only the newest write owns the UI when it
  // settles, so an older rejection can never clobber a newer row.
  const seqRef = useRef(0);
  // A failed load is terminal, not a loading state: nothing will ever arrive
  // on its own, so the card shows the daemon's sentence and a Retry instead
  // of the loading lock. `loadNonce` re-runs the load effect.
  const [loadFailed, setLoadFailed] = useState(false);
  const [loadNonce, setLoadNonce] = useState(0);
  useEffect(() => {
    // No fetch when there is nothing to toggle: the daemon omits `tools`
    // for wrappers and non-MCP providers, and the section stays hidden.
    // Same rule for the handshake: a daemon that never advertised
    // `tool_policy` would refuse this request, so it is never sent.
    if (!toolPolicySupported || tools.length === 0) return;
    let cancelled = false;
    // Where the write sequence stood when this fetch started. A write issued
    // while the fetch is in flight is newer and owns the UI; a write that
    // settled before the fetch started does not poison it. Comparing against
    // the sequence at fetch start — never against zero — is what lets a
    // refetch (a reconnect's capability flip, a changed tool list) still
    // apply after a write.
    const seqAtFetch = seqRef.current;
    void toolPolicyGet()
      .then((reply) => {
        if (cancelled) return;
        // A write issued while this fetch was in flight is newer: keep it.
        if (seqRef.current !== seqAtFetch) return;
        policiesRef.current = reply.policies;
        setPolicies(reply.policies);
        // The store has spoken: a stale load error and its terminal state go.
        setError(null);
        setLoadFailed(false);
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          // Without stored rows there is nothing to show and nothing to
          // edit: that is a terminal state — the daemon's sentence plus a
          // Retry — not a loading state to sit under forever.
          if (policiesRef.current === null) setLoadFailed(true);
          setError(reasonFromCause(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [provider.id, tools.length, toolPolicySupported, loadNonce]);
  if (!toolPolicySupported || tools.length === 0) return null;
  const { enabled, disabledTools } = toolPolicyFor(provider.id, policies);
  const disabledSet = new Set(disabledTools);
  // The stored rows are still in flight: until they land, `toolPolicyFor`
  // reads the missing row as "everything on", which is a guess, so nothing
  // in the card may be edited yet (finding 6 — the master switch was
  // already locked here while the tool rows below stayed live).
  const loading = policies === null;

  /**
   * The daemon never gates this tool, so it is never sent in a deny list
   * and a stored row that names it (stale daemon data) is stripped here.
   */
  function stripAlwaysOn(names: readonly string[]): string[] {
    return names.filter((name) => name !== ALWAYS_ON_TOOL);
  }

  async function persist(nextEnabled: boolean, nextDisabled: readonly string[]) {
    const cleanDisabled = stripAlwaysOn(nextDisabled);
    // This write's own revert base: the provider row as it stands right
    // now, read through the ref and not through the render closure, so an
    // earlier write's optimistic row is part of the base (findings 1, 8).
    const previous = toolPolicyFor(provider.id, policiesRef.current);
    // Sequence guard (findings 1, 8). Every write is sent immediately, in
    // click order: a second toggle must still reach the daemon — dropping
    // it on a stale `busy` loses the user's click. Overlap is resolved when
    // a write settles instead: only the newest sequence owns the UI, so a
    // rejection a newer write has superseded reverts nothing and reports
    // nothing and the newer optimistic row stands.
    const seq = ++seqRef.current;
    setBusy(true);
    setError(null);
    // Optimistic row, appended to the ref mirror: it always holds the
    // newest rows, including an earlier write's optimistic row when two
    // writes overlap.
    const row: ToolPolicyEntry = {
      providerId: provider.id,
      enabled: nextEnabled ? null : false,
      disabledTools: cleanDisabled,
    };
    const optimistic: readonly ToolPolicyEntry[] = [
      ...(policiesRef.current ?? []).filter((entry) => entry.providerId !== provider.id),
      row,
    ];
    policiesRef.current = optimistic;
    setPolicies(optimistic);
    try {
      await toolPolicySet(provider.id, nextEnabled ? null : false, cleanDisabled);
      // Confirmed. An older write settling here must not clear a busy flag
      // the newest write still needs.
      if (seq === seqRef.current) setBusy(false);
      return;
    } catch (cause) {
      // A newer write superseded this one: its optimistic row stands, this
      // rejection reports nothing.
      if (seq !== seqRef.current) return;
      // No newer write exists, so the row in the ref is the one this write
      // wrote: put back the row this write itself replaced, applied to the
      // current rows (never a stale render snapshot).
      const reverted: readonly ToolPolicyEntry[] = [
        ...(policiesRef.current ?? []).filter((entry) => entry.providerId !== provider.id),
        {
          providerId: provider.id,
          enabled: previous.enabled ? null : false,
          disabledTools: [...previous.disabledTools],
        },
      ];
      policiesRef.current = reverted;
      setPolicies(reverted);
      setError(reasonFromCause(cause));
      setBusy(false);
    }
  }

  function toggleProvider(next: boolean) {
    void persist(next, toolPolicyFor(provider.id, policiesRef.current).disabledTools);
  }

  function toggleTool(name: string, next: boolean) {
    if (name === ALWAYS_ON_TOOL) return;
    // Live state, not this render's: two toggles in one tick must each flip
    // the row the other just wrote rather than re-send a duplicate write.
    const current = toolPolicyFor(provider.id, policiesRef.current);
    const nextDisabled = next
      ? current.disabledTools.filter((tool) => tool !== name)
      : [...current.disabledTools, name];
    void persist(current.enabled, nextDisabled);
  }

  function retryLoad() {
    setError(null);
    setLoadFailed(false);
    setLoadNonce((nonce) => nonce + 1);
  }

  return (
    <div className="provider-card-block provider-tools">
      <details>
        <summary>Tool settings</summary>
        <label className="provider-tool-row">
          <input
            type="checkbox"
            role="switch"
            aria-label={`Enable tools for ${provider.id}`}
            checked={enabled}
            disabled={busy || loading}
            onChange={(event) => toggleProvider(event.target.checked)}
          />
          <span>Enable tools</span>
        </label>
        <div className="provider-tool-list">
          {tools.map((tool) => {
            const alwaysOn = tool.name === ALWAYS_ON_TOOL;
            const checked = alwaysOn ? true : enabled && !disabledSet.has(tool.name);
            const inputId = `tool-${provider.id}-${tool.name}`;
            return (
              <div className="provider-tool-row" key={tool.name}>
                <input
                  id={inputId}
                  type="checkbox"
                  checked={checked}
                  disabled={busy || alwaysOn || !enabled || loading}
                  onChange={(event) => toggleTool(tool.name, event.target.checked)}
                />
                <label htmlFor={inputId}>
                  <span className="provider-tool-name">{tool.name}</span>
                  <span className="provider-tool-description"> {tool.description}</span>
                </label>
                {alwaysOn ? <span className="provider-tool-note">{ALWAYS_ON_REASON}</span> : null}
              </div>
            );
          })}
        </div>
        {error === null ? null : (
          <p role="alert" className="device-error">
            {error}
          </p>
        )}
        {loadFailed ? (
          <button type="button" className="settings-device-action" onClick={retryLoad}>
            Retry
          </button>
        ) : null}
      </details>
    </div>
  );
}

/**
 * The handshake capability that gates the whole Agents section, spelled
 * exactly like the daemon's own name for it. A daemon that does not
 * advertise it cannot answer `agent_profiles_get`, so the section renders
 * nothing and no request is sent — the section is absent, not broken.
 */
const AGENT_PROFILES_CAPABILITY = "agent_profiles";

/** The profile store's caps, the daemon's own constants mirrored. */
const MAX_PROFILE_NAME_CHARS = 60;
const MAX_PROFILE_NOTE_BYTES = 2 * 1024;
const MAX_STANDING_INSTRUCTIONS_BYTES = 8 * 1024;

/** The daemon counts UTF-8 bytes (`String::len`), so the on-screen counter must too. */
function utf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/**
 * The daemon counts a profile name in Unicode scalar values
 * (`str::chars().count()`), so the cap must count the same unit: iteration
 * yields whole code points, and one astral-plane character (emoji, CJK
 * extensions) is one — where UTF-16 code-unit counting would call it two and
 * refuse names the daemon accepts.
 */
function charCount(text: string): number {
  return [...text].length;
}

/**
 * Every write replaces the whole document, so each one travels on a deep
 * copy: a mutation made for one write must never sit under an earlier
 * write's revert base, and no render may alias the stored document.
 */
function cloneDocument(document: AgentProfilesDocument): AgentProfilesDocument {
  return JSON.parse(JSON.stringify(document)) as AgentProfilesDocument;
}

/**
 * One row's name/note editor — the only fields editable here on purpose.
 * Provider, model, mode, thinking option and features are the provider's own
 * vocabulary, and the app has no source for that vocabulary at authoring time
 * (a session manifest only exists once a session of that provider is already
 * running), so the form refuses to invent one: these fields stay exactly as
 * the daemon holds them and the row displays them. The name is capped in
 * characters, the note in UTF-8 bytes — both refusals name the size, and
 * nothing is ever truncated.
 *
 * Save sits under the panel's `busy` lock like every other write trigger:
 * while a write is in flight the editor cannot start a second one, so a
 * revert can never land on a document the daemon has just refused.
 */
function AgentProfileEditor({
  profile,
  busy,
  onSave,
  onClose,
}: {
  profile: AgentProfile;
  /** True while a panel write is in flight: Save must not start another. */
  busy: boolean;
  onSave: (name: string, note: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(profile.name);
  const [note, setNote] = useState(profile.note);
  const noteBytes = utf8Bytes(note);
  return (
    <div className="agent-inline-editor">
      <label className="device-field">
        Name
        <input value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <label className="device-field">
        Note — what a creating agent reads to choose this profile. Write it for the agent.
        <textarea value={note} onChange={(event) => setNote(event.target.value)} rows={3} />
        <span className="agent-byte-counter">
          {noteBytes} / {MAX_PROFILE_NOTE_BYTES} bytes
        </span>
      </label>
      <p className="device-field-hint">
        Provider, model, mode and features are shown on the row and are not editable here: the app
        has no live vocabulary to offer for them yet.
      </p>
      <div className="device-actions">
        <button
          type="button"
          className="settings-device-action"
          disabled={busy}
          onClick={() => onSave(name, note)}
        >
          Save
        </button>
        <button type="button" className="settings-device-action" onClick={onClose}>
          Cancel
        </button>
      </div>
    </div>
  );
}

/**
 * Settings → Agents: the daemon's agent-profile store, the twin of
 * `ProviderToolSettings` in discipline — capability gate, loading lock,
 * `role="alert"` errors, optimistic whole-document writes with the sequence
 * guard. What it holds:
 *
 * - The ordered profile list. The human's order is the order
 *   `devboule_list_profiles` serves an agent, so reordering is the feature,
 *   not a nicety.
 * - One tick per row, “agents may create this”. The tick is the consent: an
 *   unticked profile is the human's own and stays invisible to agents.
 * - The note, called out on the row as what a creating agent reads.
 * - The standing instructions, one document with the profiles, capped and
 *   refused over the cap — never truncated.
 * - An empty state that reads as the off switch: nothing ticked means agents
 *   create nothing at all.
 *
 * There is deliberately no add-profile form and no provider/model/mode
 * editing: a form needs that vocabulary, and no live source for it exists in
 * the app yet, so none is invented here.
 */
function AgentProfilesPanel() {
  const daemon = useWorkspaceDaemon();
  const agentProfilesSupported = daemon.capabilities.includes(AGENT_PROFILES_CAPABILITY);
  const [document, setDocument] = useState<AgentProfilesDocument | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Synchronous mirror of `document` — what a second rapid write reads and
  // what a rejection reverts onto, never a render closure (audit findings 1, 8).
  const documentRef = useRef<AgentProfilesDocument | null>(null);
  // Monotonic write sequence: only the newest write owns the UI when it settles.
  const seqRef = useRef(0);
  // Which row's editor / delete confirm is open. One of each, panel-wide.
  const [editingId, setEditingId] = useState<string | null>(null);
  const [deleteArmedId, setDeleteArmedId] = useState<string | null>(null);
  // The standing-instructions draft. Null means the textarea shows the
  // document; the first keystroke sets it, so the optimistic document swap
  // of an in-flight write cannot eat what the human is typing mid-write. It
  // is released — back to null — when a write is confirmed and when a fresh
  // load lands: the two moments the panel is back in agreement with the
  // store. A rejected save keeps the draft, so the text survives to be
  // shortened and retried.
  const [standingDraft, setStandingDraft] = useState<string | null>(null);
  // A failed load is terminal, not a loading state: nothing will ever arrive
  // on its own, so the panel shows the daemon's sentence and a Retry instead
  // of the loading lock. `loadNonce` re-runs the load effect.
  const [loadFailed, setLoadFailed] = useState(false);
  const [loadNonce, setLoadNonce] = useState(0);

  useEffect(() => {
    if (!agentProfilesSupported) return;
    let cancelled = false;
    // Where the write sequence stood when this fetch started. A write issued
    // while the fetch is in flight is newer and owns the UI; a write that
    // settled before the fetch started does not poison it. Comparing against
    // the sequence at fetch start — never against zero — is what lets a
    // refetch (a daemon restart's capability flip) apply after a write
    // instead of being discarded for the life of the mount.
    const seqAtFetch = seqRef.current;
    void agentProfilesGet()
      .then((reply) => {
        if (cancelled) return;
        // A write issued while this fetch was in flight is newer: keep it.
        if (seqRef.current !== seqAtFetch) return;
        documentRef.current = reply.document;
        setDocument(reply.document);
        // The store has spoken: the panel agrees with it again, so a stale
        // load error goes and the standing box re-seeds from the document
        // rather than keeping a draft typed against an older store.
        setError(null);
        setLoadFailed(false);
        setStandingDraft(null);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        // Without a document there is nothing to show and nothing to edit:
        // that is a terminal state — the daemon's sentence plus a Retry —
        // not a loading state to sit under forever.
        if (documentRef.current === null) setLoadFailed(true);
        setError(reasonFromCause(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [agentProfilesSupported, loadNonce]);

  if (!agentProfilesSupported) return null;
  // A null document is the fetch in flight, and nothing may be edited from a
  // guess — same rule as the tool toggles.
  const loading = document === null;
  const profiles = document?.profiles ?? [];
  const standingValue = standingDraft ?? document?.standingInstructions ?? "";
  const standingBytes = utf8Bytes(standingValue);

  /**
   * Sends one whole-document write, optimistically, under the sequence
   * guard. `previous` is the ref value this write started from, so a
   * rejection puts back exactly what the human was seeing, and a rejection a
   * newer write superseded reverts nothing and reports nothing.
   */
  async function persist(next: AgentProfilesDocument) {
    const previous = documentRef.current;
    const seq = ++seqRef.current;
    setBusy(true);
    setError(null);
    documentRef.current = next;
    setDocument(next);
    try {
      await agentProfilesSet(next);
      // An older write settling here must not clear a busy flag the newest
      // write still needs.
      if (seq === seqRef.current) {
        setBusy(false);
        // Confirmed: the store now holds what was sent, so the standing box
        // goes back to reading the document. A rejected save keeps the draft.
        setStandingDraft(null);
      }
    } catch (cause) {
      if (seq !== seqRef.current) return;
      documentRef.current = previous;
      setDocument(previous);
      setError(reasonFromCause(cause));
      setBusy(false);
    }
  }

  function retryLoad() {
    setError(null);
    setLoadFailed(false);
    setLoadNonce((nonce) => nonce + 1);
  }

  function toggleEnabled(id: string, next: boolean) {
    const current = documentRef.current;
    if (current === null) return;
    const updated = cloneDocument(current);
    const row = updated.profiles.find((profile) => profile.id === id);
    if (row === undefined) return;
    row.enabledForAgents = next;
    void persist(updated);
  }

  function move(id: string, delta: -1 | 1) {
    const current = documentRef.current;
    if (current === null) return;
    const from = current.profiles.findIndex((profile) => profile.id === id);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= current.profiles.length) return;
    const updated = cloneDocument(current);
    const [row] = updated.profiles.splice(from, 1);
    updated.profiles.splice(to, 0, row);
    void persist(updated);
  }

  function remove(id: string) {
    const current = documentRef.current;
    if (current === null) return;
    const updated = cloneDocument(current);
    updated.profiles = updated.profiles.filter((profile) => profile.id !== id);
    setDeleteArmedId(null);
    void persist(updated);
  }

  function saveProfileFields(id: string, name: string, note: string) {
    const current = documentRef.current;
    if (current === null) return;
    const trimmed = name.trim();
    // The cap counts Unicode scalar values — the daemon's `chars().count()` —
    // not UTF-16 code units, which would double-count an astral-plane name
    // and refuse names the daemon accepts. The refusal names that same count.
    const trimmedChars = charCount(trimmed);
    // Refuse and name the size; never clip. The editor stays open with the
    // text intact, so the human can shorten it themselves.
    if (trimmedChars === 0) {
      setError(`A profile name is 1 to ${MAX_PROFILE_NAME_CHARS} characters.`);
      return;
    }
    if (trimmedChars > MAX_PROFILE_NAME_CHARS) {
      setError(
        `This name is ${trimmedChars} characters, over the ${MAX_PROFILE_NAME_CHARS}-character cap. Nothing was saved and nothing was truncated.`,
      );
      return;
    }
    const noteBytes = utf8Bytes(note);
    if (noteBytes > MAX_PROFILE_NOTE_BYTES) {
      setError(
        `This note is ${noteBytes} bytes, over the ${MAX_PROFILE_NOTE_BYTES}-byte cap. Nothing was saved and nothing was truncated.`,
      );
      return;
    }
    const updated = cloneDocument(current);
    const row = updated.profiles.find((profile) => profile.id === id);
    if (row === undefined) return;
    row.name = trimmed;
    row.note = note;
    setEditingId(null);
    void persist(updated);
  }

  function saveStandingInstructions() {
    const current = documentRef.current;
    if (current === null) return;
    const bytes = utf8Bytes(standingValue);
    if (bytes > MAX_STANDING_INSTRUCTIONS_BYTES) {
      setError(
        `The standing instructions are ${bytes} bytes, over the ${MAX_STANDING_INSTRUCTIONS_BYTES}-byte cap. Nothing was saved and nothing was truncated.`,
      );
      return;
    }
    const updated = cloneDocument(current);
    updated.standingInstructions = standingValue;
    void persist(updated);
  }

  return (
    <div id="settings-panel-agents" role="tabpanel" aria-label="Agents">
      <SettingsHeading
        title="Agents"
        description="Profiles are the kinds of agent an agent may start. The order here is the order agents read, top down. A profile without the tick stays yours alone: agents never see it."
      />
      <div className="settings-stack settings-stack-spaced agent-profiles">
        {error === null ? null : (
          <p role="alert" className="device-error">
            {error}
          </p>
        )}
        {loading && !loadFailed ? <div role="status">Loading agent profiles…</div> : null}
        {loadFailed ? (
          <button type="button" className="settings-device-action" onClick={retryLoad}>
            Retry
          </button>
        ) : null}
        {document !== null && profiles.every((profile) => !profile.enabledForAgents) ? (
          <div className="agent-profiles-off" role="status">
            <p>
              No profile is ticked, so agents cannot start agents — every creation attempt is
              refused.
            </p>
            <p>Tick “agents may create this” on a profile to let agents start that kind.</p>
          </div>
        ) : null}
        {document !== null ? (
          <p className="device-copy agent-profiles-intro">
            The note is what a creating agent reads to choose between profiles — write it for the
            agent, not for yourself.
          </p>
        ) : null}
        <ol className="agent-profile-list">
          {profiles.map((profile, index) => {
            const editing = editingId === profile.id;
            const deleteArmed = deleteArmedId === profile.id;
            return (
              <li className="agent-profile-row" key={profile.id}>
                <div className="agent-profile-order">
                  <button
                    type="button"
                    className="settings-device-action"
                    aria-label={`Move ${profile.name} up`}
                    disabled={busy || loading || index === 0}
                    onClick={() => move(profile.id, -1)}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="settings-device-action"
                    aria-label={`Move ${profile.name} down`}
                    disabled={busy || loading || index === profiles.length - 1}
                    onClick={() => move(profile.id, 1)}
                  >
                    ↓
                  </button>
                </div>
                <div className="agent-profile-main">
                  <span className="settings-card-title">{profile.name}</span>
                  <span className="agent-profile-meta">
                    {profile.provider} · {profile.model} · mode {profile.modeId}
                  </span>
                  {profile.note ? (
                    <span className="agent-profile-note">
                      <span className="agent-profile-note-label">When to use: </span>
                      {profile.note}
                    </span>
                  ) : (
                    <span className="agent-profile-note agent-profile-note-empty">
                      No note — agents choosing between profiles will be choosing blind on this one.
                    </span>
                  )}
                </div>
                <label className="agent-profile-tick">
                  <input
                    type="checkbox"
                    checked={profile.enabledForAgents}
                    disabled={busy || loading}
                    onChange={(event) => toggleEnabled(profile.id, event.target.checked)}
                  />
                  <span>
                    <span>Agents may create this</span>
                    <span className="agent-profile-tick-note">
                      Lets an agent start this kind of agent. If this profile answers its own
                      permission cards, its children run unattended.
                    </span>
                  </span>
                </label>
                <div className="device-actions">
                  <button
                    type="button"
                    className="settings-device-action"
                    disabled={busy || loading}
                    onClick={() => {
                      setEditingId(editing ? null : profile.id);
                      setDeleteArmedId(null);
                    }}
                  >
                    {editing ? "Close editor" : "Edit"}
                  </button>
                  <button
                    type="button"
                    className="settings-device-action"
                    disabled={busy || loading}
                    onClick={() => setDeleteArmedId(deleteArmed ? null : profile.id)}
                  >
                    Delete
                  </button>
                </div>
                {editing ? (
                  <AgentProfileEditor
                    profile={profile}
                    busy={busy}
                    onSave={(name, note) => saveProfileFields(profile.id, name, note)}
                    onClose={() => setEditingId(null)}
                  />
                ) : null}
                {deleteArmed ? (
                  <div className="device-inline-confirm">
                    <p className="device-copy">
                      Deletes this profile. Agents are no longer offered it, and a creation naming
                      it is refused.
                    </p>
                    <div className="device-actions">
                      <button
                        type="button"
                        className="settings-device-action"
                        disabled={busy || loading}
                        onClick={() => remove(profile.id)}
                      >
                        Delete now
                      </button>
                      <button
                        type="button"
                        className="settings-device-action"
                        onClick={() => setDeleteArmedId(null)}
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                ) : null}
              </li>
            );
          })}
        </ol>
        {/* Rendered from the first paint, locked while the fetch runs: the
            standing instructions are half of the same document, so the box
            must exist — disabled — before the store answers. */}
        <div className="agent-standing">
          <span className="settings-subheading">Standing instructions</span>
          <p className="device-copy">
            Rules you write once: every agent this daemon starts — one you open, one an agent
            created, a Design child — receives them with its first task.
          </p>
          <textarea
            aria-label="Standing instructions for every agent"
            value={standingValue}
            disabled={loading}
            onChange={(event) => setStandingDraft(event.target.value)}
            rows={6}
          />
          <div className="agent-standing-actions">
            <span className="agent-byte-counter">
              {standingBytes} / {MAX_STANDING_INSTRUCTIONS_BYTES} bytes — over the cap the save is
              refused, nothing is truncated
            </span>
            <button
              type="button"
              className="settings-device-action"
              disabled={busy || loading}
              onClick={saveStandingInstructions}
            >
              Save standing instructions
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function ProvidersPanel() {
  const [catalog, setCatalog] = useState<ProviderCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Bumped by every fetch (mount and refresh); a response only applies when its
  // sequence is still the latest, so a slow mount list cannot revert a refresh.
  const fetchSeqRef = useRef(0);
  // Set synchronously on click so a second click before the re-render is a no-op.
  const refreshInFlightRef = useRef(false);
  // The one npm run the daemon is executing on this client's behalf.
  const [npmRun, setNpmRun] = useState<ProviderNpmRun | null>(null);
  // Per-card, dismissible failure from the last npm run.
  const [npmFailure, setNpmFailure] = useState<{ providerId: string; text: string } | null>(null);
  const [consent, setConsent] = useState<ProviderConsent | null>(null);
  // Cleared in the consent effect (not at the end of confirm): a second
  // synchronous click still sees the stale non-null consent, so the ref must
  // stay armed until that re-render.
  const consentInFlightRef = useRef(false);
  const consentConfirmRef = useRef<HTMLButtonElement>(null);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);

  // The handshake's own capability list, through the same channel every other
  // surface reads it (Workspace, Design): the supervisor's `daemon_status`.
  // A daemon that never advertised `tool_policy` leaves the toggles off the
  // screen, so no card asks it for a policy it cannot answer.
  const daemon = useWorkspaceDaemon();
  const toolPolicySupported = daemon.capabilities.includes(TOOL_POLICY_CAPABILITY);

  useEffect(() => {
    consentInFlightRef.current = false;
    if (consent !== null) {
      consentConfirmRef.current?.focus();
    } else {
      consentRestoreRef.current?.focus();
      consentRestoreRef.current = null;
    }
  }, [consent]);

  useEffect(() => {
    if (consent === null) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setConsent(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [consent]);

  useEffect(() => {
    let cancelled = false;
    const seq = ++fetchSeqRef.current;
    void providersList()
      .then((listed) => {
        if (!cancelled && seq === fetchSeqRef.current) setCatalog(listed);
      })
      .catch((cause: unknown) => {
        if (!cancelled && seq === fetchSeqRef.current) {
          setCatalog({ providers: [], unreadableDirs: 0 });
          setError(reasonFromCause(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  function refresh() {
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    setRefreshing(true);
    setError(null);
    const seq = ++fetchSeqRef.current;
    void providersRefresh()
      .then((fresh) => {
        if (seq === fetchSeqRef.current) setCatalog(fresh);
      })
      .catch((cause: unknown) => {
        if (seq === fetchSeqRef.current) {
          setError(reasonFromCause(cause));
        }
      })
      .finally(() => {
        // Unconditional on purpose: React 18+ treats setState on an unmounted
        // component as a safe no-op, so the button can never get stuck on
        // "Refreshing…". Do not re-add an unmount guard here — StrictMode's
        // double mount kept a stale one true and wedged the button for real.
        refreshInFlightRef.current = false;
        setRefreshing(false);
      });
  }

  function openConsent(
    provider: ProviderInfo,
    verb: "update" | "install",
    trigger: HTMLButtonElement,
  ) {
    consentRestoreRef.current = trigger;
    setConsent({ provider, verb });
  }

  function confirmConsent() {
    if (consent === null || consentInFlightRef.current) return;
    consentInFlightRef.current = true;
    const { provider, verb } = consent;
    setConsent(null);
    setNpmFailure(null);
    setNpmRun({ providerId: provider.id, verb });
    // Sequence for the post-success refetch; a concurrent refresh supersedes it.
    const seq = ++fetchSeqRef.current;
    void providerUpdate(provider.id)
      .then((outcome) => {
        if (!outcome.ok) {
          setNpmFailure({ providerId: provider.id, text: logTail(outcome.log) });
          return;
        }
        // The refetch is the proof: the fresh catalog carries the new version.
        void providersList()
          .then((fresh) => {
            if (seq === fetchSeqRef.current) setCatalog(fresh);
          })
          .catch((cause: unknown) => {
            if (seq === fetchSeqRef.current) setError(reasonFromCause(cause));
          });
      })
      .catch((cause: unknown) => {
        setNpmFailure({ providerId: provider.id, text: reasonFromCause(cause) });
      })
      // Unconditional: setState on an unmounted component is a safe no-op in
      // React 18+, and an unmount guard wedged the Refresh button once under
      // StrictMode (see refresh() above).
      .finally(() => {
        setNpmRun(null);
      });
  }

  const providers = catalog?.providers ?? null;
  const unreadableDirs = catalog?.unreadableDirs ?? 0;
  return (
    <div id="settings-panel-providers" role="tabpanel" aria-label="Providers and models">
      <SettingsHeading
        title="Providers & models"
        description="CLI agents found on PATH. An executable is not a login: the status shows the last measured start outcome, or unknown until one is measured."
      />
      <button className="provider-refresh" type="button" disabled={refreshing} onClick={refresh}>
        {refreshing ? "Refreshing…" : "Refresh"}
      </button>
      {error ? <div role="alert">{error}</div> : null}
      {providers === null ? (
        <div role="status">Looking for agent CLIs on PATH…</div>
      ) : providers.length === 0 ? (
        <div className="provider-empty" role="status">
          <div>
            {unreadableDirs > 0
              ? `No agent CLI found, but ${unreadableDirs} PATH directories could not be read`
              : "No agent CLI found on PATH"}
          </div>
          <p>
            Install an agent CLI such as grok, claude, or gemini and restart Devboule. Until then
            there is no provider to start a session with.
          </p>
        </div>
      ) : (
        <>
          <div className="provider-list" aria-busy={refreshing || npmRun !== null}>
            {providers.map((provider) => {
              const isNotInstalled = provider.installed === false;
              const runHere = npmRun?.providerId === provider.id;
              const consentHere = consent?.provider.id === provider.id;
              const failureHere = npmFailure?.providerId === provider.id;
              const detail = isNotInstalled
                ? (provider.npmPackage ?? provider.executable)
                : provider.executable;
              const npmCommand =
                consent !== null && consent.provider.npmPackage
                  ? `npm install -g ${consent.provider.npmPackage}@latest`
                  : null;
              return (
                <div
                  className="provider-card"
                  key={provider.id}
                  aria-busy={runHere ? "true" : undefined}
                >
                  <span className="provider-copy">
                    <span className="provider-name">{provider.id}</span>
                    {detail ? <span className="provider-detail">{detail}</span> : null}
                    <ProviderVersionLine provider={provider} />
                  </span>
                  <span className="provider-controls">
                    {isNotInstalled ? (
                      <span className="provider-status provider-status-idle">not installed</span>
                    ) : (
                      <>
                        {provider.origin === "npx-wrapper" ? (
                          <span className="provider-status provider-status-ready">npx</span>
                        ) : null}
                        {provider.protocol === "acp" ? (
                          <span className="provider-status provider-status-ready">ACP</span>
                        ) : provider.protocol === "stream-json" ? (
                          <span className="provider-status provider-status-ready">stream-json</span>
                        ) : provider.protocol === "pi-rpc" ? (
                          <span className="provider-status provider-status-ready">pi-rpc</span>
                        ) : provider.protocol === "codex-app-server" ? (
                          <span className="provider-status provider-status-ready">app-server</span>
                        ) : null}
                        <span
                          className={`provider-status ${
                            provider.authentication === "ok"
                              ? "provider-status-ready"
                              : provider.authentication.startsWith("failed:")
                                ? "provider-status-missing"
                                : "provider-status-idle"
                          }`}
                        >
                          {providerStatusText(provider)}
                        </span>
                      </>
                    )}
                    {runHere ? (
                      <button
                        className={`provider-refresh ${
                          npmRun.verb === "install" ? "provider-install" : "provider-update"
                        }`}
                        type="button"
                        disabled
                      >
                        {npmRun.verb === "install" ? "Installing…" : "Updating…"}
                      </button>
                    ) : (
                      <>
                        {!isNotInstalled && providerCanUpdate(provider) ? (
                          <button
                            className="provider-refresh provider-update"
                            type="button"
                            disabled={npmRun !== null}
                            onClick={(event) =>
                              openConsent(provider, "update", event.currentTarget)
                            }
                          >
                            Update
                          </button>
                        ) : null}
                        {isNotInstalled && provider.npmPackage ? (
                          <button
                            className="provider-refresh provider-install"
                            type="button"
                            disabled={npmRun !== null}
                            onClick={(event) =>
                              openConsent(provider, "install", event.currentTarget)
                            }
                          >
                            Install
                          </button>
                        ) : null}
                      </>
                    )}
                  </span>
                  {consentHere ? (
                    <div
                      className="provider-card-block provider-consent"
                      role="group"
                      aria-label={`Confirm ${consent.verb} for ${provider.id}`}
                    >
                      <div className="provider-consent-command">{npmCommand}</div>
                      <p className="provider-consent-notice">
                        This changes your global npm installation; running sessions keep the old
                        version until they are restarted.
                      </p>
                      <div className="provider-consent-actions">
                        <button
                          type="button"
                          className="provider-refresh provider-consent-cancel"
                          onClick={() => setConsent(null)}
                        >
                          Cancel
                        </button>
                        <button
                          ref={consentConfirmRef}
                          type="button"
                          className="provider-refresh provider-consent-confirm"
                          onClick={confirmConsent}
                        >
                          Confirm
                        </button>
                      </div>
                    </div>
                  ) : null}
                  {failureHere ? (
                    <div className="provider-card-block provider-update-error">
                      <pre>{npmFailure.text}</pre>
                      <button
                        type="button"
                        className="provider-refresh provider-update-error-dismiss"
                        onClick={() => setNpmFailure(null)}
                      >
                        Dismiss
                      </button>
                    </div>
                  ) : null}
                  <ProviderToolSettings
                    key={provider.id}
                    provider={provider}
                    toolPolicySupported={toolPolicySupported}
                  />
                </div>
              );
            })}
          </div>
          {unreadableDirs > 0 ? (
            <p className="provider-empty" role="status">
              {unreadableDirs} PATH directories could not be read
            </p>
          ) : null}
        </>
      )}
    </div>
  );
}

function ProjectsPanel() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [workspacesByProject, setWorkspacesByProject] = useState<Record<string, Workspace[]>>({});
  const [workspaceErrors, setWorkspaceErrors] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const addProjectRef = useRef<HTMLButtonElement>(null);

  const loadProjects = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const listed = await projectsList();
      const results = await Promise.all(
        listed.map(async (project) => {
          try {
            return { id: project.id, workspaces: await workspacesList(project.id) };
          } catch (cause: unknown) {
            return { id: project.id, error: reasonFromCause(cause) };
          }
        }),
      );
      const nextWorkspaces: Record<string, Workspace[]> = {};
      const nextErrors: Record<string, string> = {};
      for (const result of results) {
        if (Array.isArray(result.workspaces)) nextWorkspaces[result.id] = result.workspaces;
        else if (typeof result.error === "string") nextErrors[result.id] = result.error;
      }
      setProjects(listed);
      setWorkspacesByProject(nextWorkspaces);
      setWorkspaceErrors(nextErrors);
    } catch (cause: unknown) {
      setProjects([]);
      setWorkspacesByProject({});
      setWorkspaceErrors({});
      setError(reasonFromCause(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadProjects();
  }, [loadProjects]);

  const closeDialog = useCallback(() => {
    setDialogOpen(false);
    addProjectRef.current?.focus();
  }, []);

  const handleProjectAdded = useCallback(async (project: Project) => {
    const workspaces = await workspacesList(project.id);
    setProjects((current) => {
      const index = current.findIndex((entry) => entry.id === project.id);
      if (index < 0) return [...current, project];
      return current.map((entry, entryIndex) => (entryIndex === index ? project : entry));
    });
    setWorkspacesByProject((current) => ({ ...current, [project.id]: workspaces }));
    setError(null);
  }, []);

  return (
    <div id="settings-panel-projects" role="tabpanel" aria-label="Projects">
      <SettingsHeading
        title="Projects"
        description="A project is a git repository or any directory this daemon can reach. Workspaces live inside it."
      />
      <div className="settings-stack settings-stack-spaced">
        {loading ? <div role="status">Loading projects…</div> : null}
        {error !== null ? (
          <div role="alert">
            {error}
            <button type="button" onClick={() => void loadProjects()}>
              Retry
            </button>
          </div>
        ) : null}
        {error === null
          ? projects.map((project) => {
              const workspaces = workspacesByProject[project.id];
              const workspaceCount = workspaces?.length;
              const workspaceError = workspaceErrors[project.id];
              return (
                <div className="settings-card settings-project-card" key={project.id}>
                  <span className="settings-card-copy">
                    <span className="settings-card-title">{project.name}</span>
                    <span className="settings-card-meta">{project.path}</span>
                    {(workspaces ?? []).map((workspace) =>
                      // Render exactly what the daemon sent: no project-path
                      // fallback, no joined path. Same contract as Session.cwd.
                      workspace.path ? (
                        <span className="settings-card-meta" key={workspace.id}>
                          {workspace.path}
                        </span>
                      ) : null,
                    )}
                  </span>
                  {workspaceError !== undefined ? (
                    <span role="alert">
                      Workspaces unavailable: {workspaceError}
                      <button type="button" onClick={() => void loadProjects()}>
                        Retry
                      </button>
                    </span>
                  ) : (
                    <span className="settings-card-value">
                      {workspaceCount ?? 0} workspace{workspaceCount === 1 ? "" : "s"}
                    </span>
                  )}
                </div>
              );
            })
          : null}
        {!loading && error === null && projects.length === 0 ? (
          <div role="status">No projects registered</div>
        ) : null}
        <button
          className="settings-dashed-action"
          type="button"
          ref={addProjectRef}
          onClick={() => setDialogOpen(true)}
        >
          <span aria-hidden="true">+</span>Add project
        </button>
      </div>

      <NewProjectDialog open={dialogOpen} onClose={closeDialog} onCreate={handleProjectAdded} />
    </div>
  );
}

function GeneralPanel() {
  return (
    <div id="settings-panel-general" role="tabpanel" aria-label="General">
      <JournalRetentionPanel />
    </div>
  );
}
