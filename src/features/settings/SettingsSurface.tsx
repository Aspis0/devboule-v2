import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import {
  agentProfilesGet,
  agentProfilesSet,
  projectsList,
  providerUpdate,
  providersList,
  providersRefresh,
  toolPolicyGet,
  toolPolicySet,
  workspacesList,
} from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import {
  DELEGATION_CAPABILITY,
  delegationController,
  useDelegationState,
  type DelegationController,
} from "../../lib/delegation";
import { DiagnosticsPanel } from "./DiagnosticsPanel";
import { DevicesPanel } from "./DevicesPanel";
import { AppearanceSection } from "./AppearanceSection";
import { overlayDenialsDescription, toolOverlayForPeerRestriction } from "./profileOverlay";
import {
  AgentProfileForm,
  EMPTY_PROFILE_FORM_SEED,
  MAX_PROFILE_FIELD_BYTES,
  MAX_PROFILE_SPAWN_PROMPT_BYTES,
  type NewProfileDraft,
  type ProfileFormSeed,
  profileTextsError,
  seedFromProfile,
  utf8Bytes,
} from "./AgentProfileForm";
import type {
  AgentProfile,
  AgentProfilesDocument,
  DelegationReply,
  Project,
  ProviderCatalog,
  ProviderInfo,
  ToolPolicyEntry,
  Workspace,
} from "../../types/ipc";
import { OraclePanel } from "../oracle/OraclePanel";
import { useWorkspaceDaemon } from "../workspace/workspaceDaemon";
import { JournalRetentionPanel } from "./JournalRetentionPanel";
import { CloseBehaviorSetting } from "./CloseBehaviorSetting";
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
 * `provider.tools` is empty: the daemon sends the `tools` key only for
 * providers whose sessions can host the broker (ACP families plus pi and
 * Codex since the broker switch-on), and an empty list means there is
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
  const [error, setError] = useState<ErrorSentence | null>(null);
  // Synchronous mirror of `policies`. It — never the render closure — is
  // what a second rapid write reads and the base its revert applies to
  // (audit findings 1, 8).
  const policiesRef = useRef<readonly ToolPolicyEntry[] | null>(null);
  // Monotonic write sequence: only the newest write owns the UI when it
  // settles, so an older rejection can never clobber a newer row.
  const seqRef = useRef(0);
  // How many writes are currently between "sent" and "settled". This card
  // deliberately lets writes overlap (see `persist`), so it is a counter,
  // and the load effect below reads it to tell "a write was in flight when
  // this fetch started" — the question the sequence number alone cannot
  // answer — apart from "a write has settled at some point".
  const writesInFlightRef = useRef(0);
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
    const writeWasInFlight = writesInFlightRef.current > 0;
    void toolPolicyGet()
      .then((reply) => {
        if (cancelled) return;
        // A write issued while this fetch was in flight is newer: keep it.
        if (seqRef.current !== seqAtFetch) return;
        // A write that was ALREADY in flight when the fetch started raced
        // it: whether the reply predates or postdates that write is
        // unknowable, so the reply adopts nothing — the write's own settle
        // (its optimistic row or its revert) is the state of record. The
        // guard answers "did a write overlap this fetch?", not just "is
        // there a newer write?".
        if (writeWasInFlight) return;
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
          setError(errorSentence(cause));
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
    // Sequence guard (findings 1, 8) — the card's real safety, stated here
    // and honoured by the controls: every write is sent immediately, in
    // click order; a second toggle must still reach the daemon, so the
    // controls stay reachable while a write is in flight and the only lock
    // is the load lock (`loading`) below. Overlap is resolved when a write
    // settles: only the newest sequence owns the UI, so a rejection a newer
    // write has superseded reverts nothing and reports nothing and the
    // newer optimistic row stands. (The agents panel — the twin that writes
    // a whole document whose row ids the daemon mints — serialises its
    // writes under one busy span instead, on purpose: an overlapping
    // whole-document write would re-send an id the daemon had already
    // replaced. Row-level toggles like these carry no minted identity, so
    // overlap is safe here and losing the click is not.)
    const seq = ++seqRef.current;
    writesInFlightRef.current += 1;
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
      // Confirmed. An older write settling here owns nothing: the newest
      // sequence keeps the UI, and there is no lock to release — the
      // controls were never locked against writes.
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
      setError(errorSentence(cause));
    } finally {
      writesInFlightRef.current -= 1;
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
            // Locked for the load only: a write in flight must not make the
            // controls unreachable — the sequence guard owns overlap (see
            // `persist`), and a control disabled on `busy` would drop the
            // user's second click, the one thing the policy says never
            // happens.
            disabled={loading}
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
                  disabled={alwaysOn || !enabled || loading}
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
            <ErrorText
              sentence={error.sentence}
              detail={error.detail}
              id="settings-tool-policy-error"
            />
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

/**
 * The handshake capability that gates the provider-vocabulary query, spelled
 * exactly like the daemon's own name for it. A daemon that does not advertise
 * it cannot answer `provider_vocabulary_get` — which is every daemon shipping
 * today, the request is never sent to one. The new-profile form still works
 * there: model and mode fall back to free text, and the form says THAT reason
 * — this daemon is older than this app — in its own sentence. It must never
 * show the provider's "did not publish" sentence instead: an old daemon and a
 * silent provider are different absences and get different sentences.
 */
const PROVIDER_VOCABULARY_CAPABILITY = "provider_vocabulary";

const MAX_STANDING_INSTRUCTIONS_BYTES = 8 * 1024;
/**
 * `MAX_PROFILES` in `crates/devboule-daemon/src/agent_profiles.rs`. The 65th
 * creation is refused by the store, so the panel mirrors the number and says
 * so before the human fills the form — offering a create it knows cannot be
 * kept, on a loop, is the failure this constant prevents.
 */
const MAX_PROFILES = 64;

/**
 * Every write replaces the whole document, so each one travels on a deep
 * copy: a mutation made for one write must never sit under an earlier
 * write's revert base, and no render may alias the stored document.
 */
function cloneDocument(document: AgentProfilesDocument): AgentProfilesDocument {
  return JSON.parse(JSON.stringify(document)) as AgentProfilesDocument;
}

/**
 * The sentence for the one absence the delegation section can name without
 * asking anyone: the daemon predates `permission_delegation`, so there is no
 * store to ask and no request may be sent. It names WHICH absence this is —
 * an old daemon, not a deliberate off — because a silent section reads exactly
 * like a switch somebody removed.
 */
const DELEGATION_UNAVAILABLE_TEXT =
  "This daemon is older than this app: it does not advertise the permission_delegation capability, so it cannot keep the switch this section is for. Nothing was sent to it.";

/**
 * The sentence for a reply that disagrees with itself: `enabled: true` beside
 * a source that can only read off. The daemon's vocabulary has no such pair,
 * so this renders a fact the app cannot smooth over — the reply said both.
 */
const DELEGATION_CONTRADICTION_LABEL =
  "The daemon's answer contradicts itself — the switch reads on, from a source that can only be off";

/**
 * The sentences the stored answer's `source` renders as, one row per value and
 * an arm for each switch reading: `off` is the sentence when the daemon's
 * answer agrees with an off switch, `on` the one beside an on switch. They are
 * pairwise distinct on purpose: `default` is "never configured" — a human said
 * nothing yet; `quarantined` is neither that nor "off" — a human DID
 * configure, and the file came back damaged; `file` is the deliberate case.
 * Collapsing any two is the absent-into-none defect wearing a settings label
 * (cross-check §2, the eighth catch). The `on` arms of `default` and
 * `quarantined` are the contradiction sentence (re-audit F11): an inconsistent
 * reply is reported as inconsistent, never dressed up as a coherent sentence
 * that contradicts the checked switch beside it.
 */
export const DELEGATION_SOURCE_LABELS: Record<
  DelegationReply["source"],
  { off: string; on: string }
> = {
  file: { off: "Off", on: "On" },
  default: { off: "Never configured", on: DELEGATION_CONTRADICTION_LABEL },
  quarantined: {
    off: "Settings file was damaged — delegation reads off",
    on: DELEGATION_CONTRADICTION_LABEL,
  },
};

/**
 * The sentence a source value OUTSIDE the closed vocabulary renders as. It is
 * its own visible line, never a blank status paragraph: a `Record` indexed
 * with an unknown key yields `undefined`, and `undefined !== null` would
 * render an empty `<p>` — a status line that says nothing, when unknown must
 * be present. The daemon's vocabulary may grow before this build learns it;
 * when it does, this sentence is what the human sees until the app catches up.
 */
const DELEGATION_SOURCE_UNKNOWN_LABEL =
  "Delegation's stored answer came from a source this app cannot name";

/**
 * The status line while the stored answer has not landed. The switch holds no
 * value yet — it must not sit there reading as a definite off with no
 * sentence saying otherwise (re-audit F10: the benign reading of an unknown
 * value).
 */
const DELEGATION_PENDING_LABEL = "Reading the stored answer…";

/**
 * The status line when the stored answer never arrived. A failed load is
 * terminal: the switch stays empty, and the empty state is named — never
 * styled into a definite off.
 */
const DELEGATION_UNREADABLE_LABEL =
  "The stored answer could not be read — the switch holds no value, not an off";

/**
 * Walks the closed source vocabulary by its raw string, so a value from a
 * newer daemon takes the visible unknown sentence instead of falling out of
 * the record into a blank render. Absent (an incomplete reply) never reaches
 * here: the controller refuses it at the wire boundary and the section shows
 * the failure instead.
 */
function delegationSourceSentence(source: string, enabled: boolean): string {
  return Object.hasOwn(DELEGATION_SOURCE_LABELS, source)
    ? DELEGATION_SOURCE_LABELS[source as DelegationReply["source"]][enabled ? "on" : "off"]
    : DELEGATION_SOURCE_UNKNOWN_LABEL;
}

/**
 * Settings → Agents, beside the profiles: the one consent surface for
 * delegated answering. The switch's home is this tab and no other — a switch
 * one tab from the profiles it governs rebuilds on screen the two-level
 * setting the committente rejected in their own words.
 *
 * The discipline is `ProviderToolSettings`' corrected one (the write rule at
 * the Agents panel's `persist`, the ref mirror + monotonic sequence + revert
 * of the tool toggles), living once in `lib/delegation.ts`'s controller —
 * the take-back on a roster row writes through the same path. Gating is the
 * handshake's: without `permission_delegation` no request is ever sent, the
 * toggle is not drawn, and the section says WHICH absence this is.
 */
export function DelegationSetting({
  controller = delegationController,
}: {
  /** Injectable so tests get a fresh controller, like the tauri seams. */
  controller?: DelegationController;
}) {
  const daemon = useWorkspaceDaemon();
  const delegationSupported = daemon.capabilities.includes(DELEGATION_CAPABILITY);
  const delegation = useDelegationState(controller);

  // Fetch on mount, only when the handshake advertised the capability, and
  // again whenever the daemon's identity changes (audit 3, F2): a daemon
  // restart — even one the 2 s poll never saw as a gap — or a reconnect
  // invalidates every cached answer, and the stored value may have been moved
  // by the `delegation.json` this section's own `source: "file"` sentence
  // advertises. The guard refuses a fetch the pipe cannot carry yet.
  useEffect(() => {
    if (!delegationSupported || daemon.state !== "connected") return;
    void controller.load();
  }, [controller, delegationSupported, daemon.state, daemon.instanceId]);

  if (!delegationSupported) {
    return (
      <p className="device-copy agent-delegation-unavailable" role="note">
        {DELEGATION_UNAVAILABLE_TEXT}
      </p>
    );
  }

  const { enabled, reply, loadFailed, error, retryLoad } = delegation;
  // The status line, from a walked table — never a definite sentence beside a
  // switch whose value is not known. While the answer has not landed, and
  // after a load has failed for good, the switch holds NO value (re-audit
  // F10): unchecked-and-disabled is not allowed to sit there reading as a
  // definite off with nothing saying otherwise, so the unknown state has its
  // own present sentence. A reply that somehow arrived without a value is
  // refused by the controller, so no arm claims off on silence.
  const statusLine =
    enabled === null
      ? loadFailed
        ? DELEGATION_UNREADABLE_LABEL
        : DELEGATION_PENDING_LABEL
      : reply === null
        ? null
        : delegationSourceSentence(reply.source, enabled);

  return (
    <section className="agent-delegation" aria-label="Answer for created children">
      <span className="settings-subheading">Answer for created children</span>
      <label className="agent-delegation-row">
        <input
          type="checkbox"
          role="switch"
          aria-label="Let agents answer their children's cards"
          checked={enabled === true}
          // An unknown switch must LOOK unknown, not look off (audit 3, F5):
          // `indeterminate` paints the dash a human reads as "no value", the
          // mixed aria state names it to assistive tech, and the class ties
          // it to the app's dashed unknown treatment elsewhere. `checked` is
          // false under it — a checkbox that paints the dash must not also
          // claim a definite checkedness — but the dash is what the eye gets.
          aria-checked={enabled === null ? "mixed" : enabled ? "true" : "false"}
          ref={(el) => {
            if (el !== null) el.indeterminate = enabled === null;
          }}
          className={enabled === null ? "agent-delegation-switch-unknown" : undefined}
          // Locked for the load only: a write in flight must not make the
          // switch unreachable — the sequence guard owns overlap, and a
          // control disabled on busy would drop the user's second click.
          disabled={enabled === null}
          onChange={(event) => void controller.setEnabled(event.target.checked)}
        />
        <span>
          <span>Let agents answer their children&apos;s cards</span>
          <span className="agent-profile-tick-note">
            While this is on, an agent may answer the permission cards of the children it created —
            allowing a write, a command, a network call: whatever the child asked for. It applies to
            every child of every agent, not the one you see. You keep seeing every card, and you can
            always still answer one yourself.
          </span>
        </span>
      </label>
      {statusLine !== null ? (
        <p className="device-field-hint agent-delegation-source" role="status">
          {statusLine}
        </p>
      ) : null}
      {error === null ? null : (
        <p role="alert" className="device-error">
          <ErrorText
            sentence={error.sentence}
            detail={error.detail}
            id="settings-delegation-error"
          />
        </p>
      )}
      {loadFailed ? (
        <button type="button" className="settings-device-action" onClick={retryLoad}>
          Retry
        </button>
      ) : null}
    </section>
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
 * - The profile form ([`AgentProfileForm`]), shared by creating and editing,
 *   which asks the daemon what a provider offers instead of inventing
 *   vocabulary, and falls back to free text — naming its own reason — when
 *   the daemon predates the query. It saves through the same `persist` path
 *   as every other write here.
 *
 * The provider catalog for the form's picker comes through the same
 * `providersList` path `ProvidersPanel` uses. The two panels never mount
 * together (one tab at a time), so there is no shared catalog to reuse and
 * this fetch is the only way the picker gets its rows.
 */
function AgentProfilesPanel() {
  const daemon = useWorkspaceDaemon();
  const agentProfilesSupported = daemon.capabilities.includes(AGENT_PROFILES_CAPABILITY);
  const providerVocabularySupported = daemon.capabilities.includes(PROVIDER_VOCABULARY_CAPABILITY);
  const [document, setDocument] = useState<AgentProfilesDocument | null>(null);
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [busy, setBusy] = useState(false);
  // Synchronous mirror of `document` — what a second rapid write reads and
  // what a rejection reverts onto, never a render closure (audit findings 1, 8).
  const documentRef = useRef<AgentProfilesDocument | null>(null);
  // Monotonic write sequence: only the newest write owns the UI when it settles.
  const seqRef = useRef(0);
  // How many writes are currently inside `persist` — sent, not yet settled
  // by their read-back or their revert. Unlike the tool card, this panel
  // never lets writes overlap (rule 1 of the write discipline, at
  // `persist`), so it is near-always 0 or 1, and the load effect reads it
  // for the one thing the sequence number cannot say: that a write was
  // already in flight when a fetch started.
  const writesInFlightRef = useRef(0);
  // Which row's editor / delete confirm is open. One of each, panel-wide.
  const [editingId, setEditingId] = useState<string | null>(null);
  const [deleteArmedId, setDeleteArmedId] = useState<string | null>(null);
  // The open editor's unsaved draft, keyed to its row. It lives HERE, not in
  // the editor's own state, because the row the editor is rendered in can be
  // removed by an in-flight delete and put back by that delete's revert: the
  // editor unmounts and remounts, and a draft kept locally would remount
  // empty. Rule 3 of the write discipline (at `persist`) applies to it
  // exactly as to the standing draft below: no write that did not carry the
  // text may release it.
  const [editorDraft, setEditorDraft] = useState<(ProfileFormSeed & { id: string }) | null>(null);
  // The new-profile form is open. Rendered closed by default; each open is a
  // fresh mount, so no stale draft survives a Cancel.
  const [creating, setCreating] = useState(false);
  // The provider catalog behind the form's picker, fetched once per mount.
  const [catalog, setCatalog] = useState<ProviderCatalog | null>(null);
  const [catalogError, setCatalogError] = useState<ErrorSentence | null>(null);
  // The standing-instructions draft. Null means the textarea shows the
  // document; the first keystroke sets it, so the optimistic document swap
  // of an in-flight write cannot eat what the human is typing mid-write. It
  // is released only under rule 3 of the write discipline (at `persist`):
  // by its own writer's confirmation — and only while it is still exactly
  // what that write sent — or by a fresh load, the one moment the store has
  // re-asserted itself out of band. No other write touches it: a tick, a
  // move, a delete or a create confirming while the human is typing has not
  // stored the typed text, so it must not drop it.
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
    const writeWasInFlight = writesInFlightRef.current > 0;
    void agentProfilesGet()
      .then((reply) => {
        if (cancelled) return;
        // A write issued while this fetch was in flight is newer: keep it.
        if (seqRef.current !== seqAtFetch) return;
        // A write that was ALREADY in flight when this fetch started raced
        // it: whether the reply predates or postdates that write is
        // unknowable, so the reply adopts nothing — the write's own
        // read-back (or its revert) is the state of record. Rule 2 of the
        // write discipline: the guard answers "did a write overlap this
        // fetch?", not just "is there a newer write?".
        if (writeWasInFlight) return;
        documentRef.current = reply.document;
        setDocument(reply.document);
        // The store has spoken: the panel agrees with it again, so a stale
        // load error goes and BOTH drafts are released (rule 3) — the
        // standing box and the open editor re-seed from the document rather
        // than keeping text typed against an older store.
        setError(null);
        setLoadFailed(false);
        setStandingDraft(null);
        setEditorDraft(null);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        // Without a document there is nothing to show and nothing to edit:
        // that is a terminal state — the daemon's sentence plus a Retry —
        // not a loading state to sit under forever.
        if (documentRef.current === null) setLoadFailed(true);
        setError(errorSentence(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [agentProfilesSupported, loadNonce]);

  // The provider picker's rows. Fetched whether or not the form is open yet,
  // so opening it needs no round trip; a failure is the form's problem to
  // show, not the panel's.
  useEffect(() => {
    if (!agentProfilesSupported) return;
    let cancelled = false;
    void providersList()
      .then((listed) => {
        if (!cancelled) setCatalog(listed);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setCatalogError(errorSentence(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [agentProfilesSupported]);

  // The form offers only installed providers: an executable that is not on
  // the machine cannot start the session the profile asks for. Memoised on
  // the catalog: the form's vocabulary effect depends on this list, so a new
  // array identity per render would re-run it — and re-ask the daemon — on
  // every parent re-render. Above the early return, like every hook here.
  const installedProviders = useMemo(
    () => (catalog?.providers ?? []).filter((provider) => provider.installed !== false),
    [catalog],
  );

  if (!agentProfilesSupported) return null;
  // A null document is the fetch in flight, and nothing may be edited from a
  // guess — same rule as the tool toggles.
  const loading = document === null;
  const profiles = document?.profiles ?? [];
  const standingValue = standingDraft ?? document?.standingInstructions ?? "";
  const standingBytes = utf8Bytes(standingValue);

  /**
   * The write discipline — one rule for all six writers (tick, editor,
   * delete, move, create, standing instructions), stated here once because
   * it is the one place every writer passes through. Three clauses:
   *
   * 1. One write owns the panel from its optimistic swap until it settles:
   *    a confirmation PLUS its read-back, or a refusal's revert. `busy`
   *    spans that whole stretch, so no second write can start inside it —
   *    the read-back is part of the write, not an afterthought. This is
   *    what keeps the daemon's minted ids adopted before the panel is
   *    writable again: a write that could start in the read-back's window
   *    would travel on the pre-read-back document, re-send an empty id, and
   *    make the daemon mint a second identity for the same row.
   * 2. A document fetch adopts its reply only when NO write overlapped it:
   *    none in flight when the fetch started (`writesInFlightRef`, read by
   *    the load effect) and none started while it flew (`seqRef`
   *    unchanged). The guard answers "did a write overlap this fetch?",
   *    never just "is there a newer write?" — the write's own read-back or
   *    revert is the state of record for anything it raced.
   * 3. What the human typed but has not saved is not a write. The standing
   *    draft and the open editor's draft (`editorDraft`) live above the
   *    write plane, and no write that did not carry their text releases
   *    them. A draft is released only by its own writer's confirmation —
   *    and only while it is still exactly what was sent — by the human
   *    abandoning it, or by a fresh load, the one moment the store has
   *    re-asserted itself out of band.
   *
   * `previous` is the ref value this write started from, so a rejection
   * puts back exactly what the human was seeing, and a rejection a newer
   * write superseded reverts nothing and reports nothing. Resolves to true
   * only when this write confirmed as the newest one and its read-back has
   * landed.
   *
   * On a confirmation the panel re-reads the document and adopts it: the
   * set reply names the request, not the stored rows, and a created
   * profile travels with `id: ""` while the daemon mints the real
   * identity — on every write that still receives an empty id. Without the
   * read-back the panel would keep guessing at an identity the store owns,
   * and every further save of such a row would mint it a new one.
   */
  async function persist(next: AgentProfilesDocument): Promise<boolean> {
    const previous = documentRef.current;
    const seq = ++seqRef.current;
    setBusy(true);
    setError(null);
    documentRef.current = next;
    setDocument(next);
    writesInFlightRef.current += 1;
    try {
      await agentProfilesSet(next);
      // The read-back rides this write's sequence (rule 1): `busy` is still
      // held while it is in flight, so no newer write can have started —
      // the guard stays only as the same defence every fetch here carries.
      try {
        const reply = await agentProfilesGet();
        if (seqRef.current === seq) {
          documentRef.current = reply.document;
          setDocument(reply.document);
        }
      } catch (cause: unknown) {
        // The write itself is confirmed, so a failed read-back reverts
        // nothing; it is named — the panel would otherwise sit on ids the
        // daemon has already replaced — unless a newer write owns the UI.
        if (seqRef.current === seq) setError(errorSentence(cause));
      }
      // An older write settling here must not clear a busy flag the newest
      // write still needs.
      const confirmed = seq === seqRef.current;
      if (confirmed) setBusy(false);
      return confirmed;
    } catch (cause) {
      // A refusal adopts nothing: no read-back is issued on this path, and
      // the document goes back to exactly what the human was seeing.
      if (seq !== seqRef.current) return false;
      documentRef.current = previous;
      setDocument(previous);
      setError(errorSentence(cause));
      setBusy(false);
      return false;
    } finally {
      writesInFlightRef.current -= 1;
    }
  }

  function retryLoad() {
    setError(null);
    setLoadFailed(false);
    setLoadNonce((nonce) => nonce + 1);
  }

  // Closing the editor is the human abandoning it: the draft goes with the
  // editor (rule 3). A write — even one that removes the editor's row and
  // then reverts — must never reach this.
  function closeEditor() {
    setEditingId(null);
    setEditorDraft(null);
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

  function saveProfileFields(id: string, draft: ProfileFormSeed) {
    const current = documentRef.current;
    if (current === null) return;
    const trimmed = draft.name.trim();
    // The caps are shared with the new-profile form: the name counts Unicode
    // scalar values — the daemon's `chars().count()` — not UTF-16 code units;
    // the note and the spawn prompt count UTF-8 bytes. Refuse and name the
    // size; never clip.
    const refusal = profileTextsError(trimmed, draft.note, draft.spawnPrompt);
    if (refusal !== null) {
      setError({ sentence: refusal, detail: null });
      return;
    }
    const model = draft.model.trim();
    const modeId = draft.modeId.trim();
    // `model` and `modeId` are required, non-optional strings on the daemon
    // side; the form refuses with its own sentence rather than shipping a
    // write the store will bounce.
    if (model === "") {
      setError({ sentence: "Choose or type a model for the profile.", detail: null });
      return;
    }
    if (modeId === "") {
      setError({ sentence: "Choose or type a mode for the profile.", detail: null });
      return;
    }
    const thinking = draft.thinkingOptionId.trim();
    const thinkingBytes = utf8Bytes(thinking);
    if (thinkingBytes > MAX_PROFILE_FIELD_BYTES) {
      setError({
        sentence: `The thinking option id is ${thinkingBytes} bytes, over the ${MAX_PROFILE_FIELD_BYTES}-byte cap. Nothing was saved and nothing was truncated.`,
        detail: null,
      });
      return;
    }
    const updated = cloneDocument(current);
    const row = updated.profiles.find((profile) => profile.id === id);
    if (row === undefined) return;
    row.name = trimmed;
    row.note = draft.note;
    // The spawn prompt is trimmed here to what the daemon would store, and an
    // empty one deletes the field — absent is its none shape on the wire, so
    // a cleared prompt is saved as cleared, never as "".
    const spawn = draft.spawnPrompt.trim();
    if (spawn === "") {
      delete row.spawnPrompt;
    } else {
      row.spawnPrompt = spawn;
    }
    row.provider = draft.provider;
    row.model = model;
    row.modeId = modeId;
    row.thinkingOptionId = thinking === "" ? null : thinking;
    // The one feature this form interprets, written the way the daemon reads
    // it; every other stored key travels verbatim — they are words this form
    // cannot interpret, never noise it may drop.
    const features = { ...row.features };
    if (draft.autoAccept) {
      features.autoAccept = true;
    } else {
      delete features.autoAccept;
    }
    row.features = features;
    // Untouched, on purpose: `id` is the identity, `icon` and the overlay are
    // the human's saved deny list, `enabledForAgents` is the row's own tick,
    // and the position is the order the agents read.
    // Close on CONFIRMATION, never on submission — the new-profile form's
    // rule, and there is one rule: a refusal must leave the editor on screen
    // with the human's draft in its fields, under the error, ready to retry.
    // (The draft lives in `editorDraft` above the write plane, so this holds
    // even for a refusal of a write that removed the editor's row: rule 3.)
    void persist(updated).then((confirmed) => {
      if (confirmed) closeEditor();
    });
  }

  /**
   * The new-profile form's save: validate, then append exactly one profile to
   * the document and send the whole thing through the panel's one `persist`
   * path — the same optimistic write, sequence guard, revert and error
   * surface as a rename or a tick. There is no second write path.
   */
  function createProfile(draft: NewProfileDraft) {
    const current = documentRef.current;
    if (current === null) return;
    // The store's cap, mirrored (MAX_PROFILES above): the daemon refuses a
    // 65th profile, so the panel refuses it first, with the same number,
    // instead of sending work it knows cannot be kept. The New-profile
    // button is already disabled at the cap; this guard covers the document
    // having changed under an open form.
    if (current.profiles.length >= MAX_PROFILES) {
      setError({
        sentence: `The store already holds ${MAX_PROFILES} profiles, the maximum the daemon allows: delete one before creating another.`,
        detail: null,
      });
      return;
    }
    const trimmedName = draft.name.trim();
    const refusal = profileTextsError(trimmedName, draft.note, draft.spawnPrompt);
    if (refusal !== null) {
      setError({ sentence: refusal, detail: null });
      return;
    }
    const model = draft.model.trim();
    const modeId = draft.modeId.trim();
    // `model` and `modeId` are required, non-optional strings on the daemon
    // side; the form refuses with its own sentence rather than shipping a
    // write the store will bounce.
    if (model === "") {
      setError({ sentence: "Choose or type a model for the profile.", detail: null });
      return;
    }
    if (modeId === "") {
      setError({ sentence: "Choose or type a mode for the profile.", detail: null });
      return;
    }
    const thinking = draft.thinkingOptionId.trim();
    const thinkingBytes = utf8Bytes(thinking);
    if (thinkingBytes > MAX_PROFILE_FIELD_BYTES) {
      setError({
        sentence: `The thinking option id is ${thinkingBytes} bytes, over the ${MAX_PROFILE_FIELD_BYTES}-byte cap. Nothing was saved and nothing was truncated.`,
        detail: null,
      });
      return;
    }
    const spawn = draft.spawnPrompt.trim();
    if (utf8Bytes(spawn) > MAX_PROFILE_SPAWN_PROMPT_BYTES) {
      // Unreachable through the form (the shared refusal fires first), but a
      // guard on its own terms: the counter and the save agree on the cap.
      setError({
        sentence: `This spawn prompt is ${utf8Bytes(spawn)} bytes, over the ${MAX_PROFILE_SPAWN_PROMPT_BYTES}-byte cap. Nothing was saved and nothing was truncated.`,
        detail: null,
      });
      return;
    }
    const profile: AgentProfile = {
      // The daemon mints the id: an empty id means "new" (see the type's doc
      // comment). Names may repeat; identity is the id.
      id: "",
      name: trimmedName,
      icon: null,
      note: draft.note,
      provider: draft.provider,
      model,
      modeId,
      // The vocabulary reply carries no thinking axis, so the field is free
      // text; empty saves none.
      thinkingOptionId: thinking === "" ? null : thinking,
      features: draft.autoAccept ? { autoAccept: true } : {},
      // The human's tick, not an agent's argument: a profile that denies
      // peer contact makes children that cannot message peers or create
      // further agents. Unticked saves nothing, exactly as before.
      toolOverlay: toolOverlayForPeerRestriction(draft.restrictPeers),
      // Default off, always: a profile that becomes agent-reachable the
      // moment it is saved is a profile nobody deliberately ticked.
      enabledForAgents: draft.enabledForAgents,
    };
    // Absent is the spawn prompt's none shape on the wire, so a profile born
    // without one carries no key at all.
    if (spawn !== "") {
      profile.spawnPrompt = spawn;
    }
    const updated = cloneDocument(current);
    // Append at the end: the human's order is the order agents read, and the
    // rows already there keep the positions the human gave them.
    updated.profiles = [...updated.profiles, profile];
    // Close on CONFIRMATION, never on submission: `persist` reverts and
    // reports a refusal, and the form must still be on screen when it does —
    // the draft stays in its fields under the error, ready to retry.
    void persist(updated).then((confirmed) => {
      if (confirmed) setCreating(false);
    });
  }

  function saveStandingInstructions() {
    const current = documentRef.current;
    if (current === null) return;
    const bytes = utf8Bytes(standingValue);
    if (bytes > MAX_STANDING_INSTRUCTIONS_BYTES) {
      setError({
        sentence: `The standing instructions are ${bytes} bytes, over the ${MAX_STANDING_INSTRUCTIONS_BYTES}-byte cap. Nothing was saved and nothing was truncated.`,
        detail: null,
      });
      return;
    }
    const sent = standingValue;
    const updated = cloneDocument(current);
    updated.standingInstructions = sent;
    void persist(updated).then((confirmed) => {
      // The store now holds `sent`. The draft is released only while it is
      // still exactly what was sent (rule 3): keystrokes typed while the
      // write was in flight are newer than the store and survive it, ready
      // for a second save.
      if (confirmed) setStandingDraft((draft) => (draft === sent ? null : draft));
    });
  }

  return (
    <div id="settings-panel-agents" role="tabpanel" aria-label="Agents">
      <SettingsHeading
        title="Agents"
        description="Profiles are the kinds of agent an agent may start. The order here is the order agents read, top down. A profile without the tick stays yours alone: agents never see it."
      />
      {/* Beside the profiles, on purpose: the switch and the profiles it
          governs are one consent surface, and splitting them across tabs
          rebuilds the two-level setting that was refused in so many words. */}
      <DelegationSetting />
      <div className="settings-stack settings-stack-spaced agent-profiles">
        {error === null ? null : (
          <p role="alert" className="device-error">
            <ErrorText sentence={error.sentence} detail={error.detail} id="settings-agents-error" />
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
        {document !== null ? (
          <div className="agent-profile-create-row">
            {/* The store's cap, mirrored: at the cap the form is not offered,
                and the sentence says why before the human types anything. */}
            {profiles.length >= MAX_PROFILES ? (
              <p className="device-field-hint" role="status">
                The store holds the maximum of {MAX_PROFILES} profiles the daemon allows: delete one
                before creating another.
              </p>
            ) : null}
            <button
              type="button"
              className="settings-device-action"
              aria-expanded={creating}
              disabled={busy || loading || profiles.length >= MAX_PROFILES}
              onClick={() => setCreating((open) => !open)}
            >
              {creating ? "Close the new-profile form" : "New profile"}
            </button>
          </div>
        ) : null}
        {creating && document !== null ? (
          <AgentProfileForm
            mode="create"
            seed={EMPTY_PROFILE_FORM_SEED}
            providers={installedProviders}
            catalogLoading={catalog === null && catalogError === null}
            catalogError={catalogError}
            vocabularySupported={providerVocabularySupported}
            busy={busy}
            onCreate={createProfile}
            onCancel={() => setCreating(false)}
          />
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
                  {/* The overlay is the human's saved deny list: shown here, editable in
                      neither mode of the form, travelling verbatim on every save. */}
                  {overlayDenialsDescription(profile.toolOverlay) === null ? null : (
                    <span className="agent-profile-note">
                      {overlayDenialsDescription(profile.toolOverlay)}
                    </span>
                  )}
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
                      // Opening an editor is a fresh draft (rule 3): the
                      // panel drops any draft left from a previous editing
                      // session and seeds this one from the stored row.
                      // Closing one is the human abandoning it.
                      if (editing) closeEditor();
                      else {
                        setEditingId(profile.id);
                        setEditorDraft({ id: profile.id, ...seedFromProfile(profile) });
                      }
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
                  <AgentProfileForm
                    mode="edit"
                    seed={editorDraft?.id === profile.id ? editorDraft : seedFromProfile(profile)}
                    providers={installedProviders}
                    catalogLoading={catalog === null && catalogError === null}
                    catalogError={catalogError}
                    vocabularySupported={providerVocabularySupported}
                    busy={busy}
                    onCreate={createProfile}
                    onSaveSeed={(draft) => saveProfileFields(profile.id, draft)}
                    onSeedChange={(draft) => setEditorDraft({ id: profile.id, ...draft })}
                    onCancel={closeEditor}
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
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Bumped by every fetch (mount and refresh); a response only applies when its
  // sequence is still the latest, so a slow mount list cannot revert a refresh.
  const fetchSeqRef = useRef(0);
  // Set synchronously on click so a second click before the re-render is a no-op.
  const refreshInFlightRef = useRef(false);
  // The one npm run the daemon is executing on this client's behalf.
  const [npmRun, setNpmRun] = useState<ProviderNpmRun | null>(null);
  // Per-card, dismissible failure from the last npm run.
  const [npmFailure, setNpmFailure] = useState<{
    providerId: string;
    text: string;
    detail: string | null;
  } | null>(null);
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
          setError(errorSentence(cause));
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
          setError(errorSentence(cause));
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
          setNpmFailure({ providerId: provider.id, text: logTail(outcome.log), detail: null });
          return;
        }
        // The refetch is the proof: the fresh catalog carries the new version.
        void providersList()
          .then((fresh) => {
            if (seq === fetchSeqRef.current) setCatalog(fresh);
          })
          .catch((cause: unknown) => {
            if (seq === fetchSeqRef.current) setError(errorSentence(cause));
          });
      })
      .catch((cause: unknown) => {
        const mapped = errorSentence(cause);
        setNpmFailure({ providerId: provider.id, text: mapped.sentence, detail: mapped.detail });
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
      {error ? (
        <div role="alert">
          <ErrorText
            sentence={error.sentence}
            detail={error.detail}
            id="settings-providers-error"
          />
        </div>
      ) : null}
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
                      <pre
                        title={npmFailure.detail ?? undefined}
                        aria-describedby={
                          npmFailure.detail ? "settings-npm-failure-detail" : undefined
                        }
                      >
                        {npmFailure.text}
                        {npmFailure.detail ? (
                          <span id="settings-npm-failure-detail" className="error-detail-sr-only">
                            {npmFailure.detail}
                          </span>
                        ) : null}
                      </pre>
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
  const [workspaceErrors, setWorkspaceErrors] = useState<Record<string, ErrorSentence>>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const addProjectRef = useRef<HTMLButtonElement>(null);

  const loadProjects = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const listed = await projectsList();
      const nextWorkspaces: Record<string, Workspace[]> = {};
      const nextErrors: Record<string, ErrorSentence> = {};
      await Promise.all(
        listed.map(async (project) => {
          try {
            nextWorkspaces[project.id] = await workspacesList(project.id);
          } catch (cause: unknown) {
            nextErrors[project.id] = errorSentence(cause);
          }
        }),
      );
      setProjects(listed);
      setWorkspacesByProject(nextWorkspaces);
      setWorkspaceErrors(nextErrors);
    } catch (cause: unknown) {
      setProjects([]);
      setWorkspacesByProject({});
      setWorkspaceErrors({});
      setError(errorSentence(cause));
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
            <ErrorText
              sentence={error.sentence}
              detail={error.detail}
              id="settings-projects-error"
            />
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
                      // A local workspace's path IS its project's path by
                      // construction, so repeating it prints the same line
                      // three times — skip only that duplicate.
                      workspace.path && workspace.path !== project.path ? (
                        <span className="settings-card-meta" key={workspace.id}>
                          {workspace.path}
                        </span>
                      ) : null,
                    )}
                  </span>
                  {workspaceError !== undefined ? (
                    <span role="alert">
                      <ErrorText
                        sentence={`Workspaces unavailable: ${workspaceError.sentence}`}
                        detail={workspaceError.detail}
                        id={`settings-workspaces-error-${project.id}`}
                      />
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
      <AppearanceSection />
      <CloseBehaviorSetting />
      <JournalRetentionPanel />
    </div>
  );
}
