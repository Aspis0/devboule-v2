import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import {
  agentProfilesGet,
  agentProfilesSet,
  projectsList,
  providerUpdate,
  providerVocabularyGet,
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
import type {
  AgentProfile,
  AgentProfilesDocument,
  DelegationReply,
  Project,
  ProviderCatalog,
  ProviderInfo,
  ProviderVocabulary,
  ToolPolicyEntry,
  Workspace,
} from "../../types/ipc";
import { OraclePanel } from "../oracle/OraclePanel";
import { useWorkspaceDaemon } from "../workspace/workspaceDaemon";
import { JournalRetentionPanel } from "./JournalRetentionPanel";
import { CloseBehaviorSetting } from "./CloseBehaviorSetting";
import { NotificationSoundSetting } from "./NotificationSoundSetting";
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

/** The profile store's caps, the daemon's own constants mirrored. */
const MAX_PROFILE_NAME_CHARS = 60;
const MAX_PROFILE_NOTE_BYTES = 2 * 1024;
const MAX_STANDING_INSTRUCTIONS_BYTES = 8 * 1024;
/**
 * `MAX_PROFILES` in `crates/devboule-daemon/src/agent_profiles.rs`. The 65th
 * creation is refused by the store, so the panel mirrors the number and says
 * so before the human fills the form — offering a create it knows cannot be
 * kept, on a loop, is the failure this constant prevents.
 */
const MAX_PROFILES = 64;

/**
 * The sentence for the one absence the form can name without asking anyone:
 * the daemon predates the vocabulary query, so no answer exists to show. The
 * state sentences are pairwise distinguishable — no two are equal, and no one
 * is a substring of another, which the sentence-orthogonality test holds for
 * every rendered sentence — but they deliberately share tail clauses: the
 * discriminating words are each sentence's own reason, never the tail.
 */
const VOCABULARY_UNAVAILABLE_TEXT =
  "This daemon is older than this app: it does not advertise the provider_vocabulary capability, so it cannot say what this provider offers. Type the model and mode below; what you type is checked when the session starts.";

/** `origin: "daemon"` — the honest sentence that travels with such a list. */
const DAEMON_VOCABULARY_TEXT =
  "This list is the daemon's own vocabulary for this provider, not something the provider published.";

/**
 * `present` whose `origin` arrived undeclared (absent, null, or a value this
 * app does not know): the reply names no author for the list. The items are
 * usable and stay offered as they arrived — what is missing is WHO authored
 * them, which is exactly what the honesty sentence exists to carry. No
 * sentence at all is what the eye reads as "the provider published this",
 * and that is the stronger of the two authorships: an undeclared list must
 * not be rendered as a declared one.
 */
function undeclaredOriginVocabularyText(axisWord: "models" | "modes"): string {
  return `This ${axisWord} list arrived with no author declared: the daemon did not say whether the provider published it or the daemon mapped it itself. Choose one from the list, or type your own instead.`;
}

/** `none`: the provider CAN answer and answered "I have none". The field stays required. */
function noneVocabularyText(axisWord: "models" | "modes"): string {
  return `This provider reports no ${axisWord}: type the one to use; a name it does not serve fails at the provider when the session starts.`;
}

/** `absent`: no source could answer. The spec's own fallback sentence. */
function absentVocabularyText(axisWord: "models" | "modes"): string {
  return `This provider did not publish its ${axisWord}; what you type is checked when the session starts.`;
}

/**
 * A reply that arrived without this axis at all: the daemon answered, and
 * what it sent cannot be read as an answer for this axis. Malformed is its
 * own state — not `absent` (which names a silent source) and not the query
 * failing (which names the transport) — and the other axis of the same
 * reply, when it arrived intact, is still shown: a malformed half must not
 * throw away a usable half.
 */
function malformedVocabularyText(axisWord: "models" | "modes"): string {
  return `The daemon's reply was malformed — it carried no ${axisWord} axis at all — so nothing is known about what this provider offers there. Type the one to use; what you type is checked when the session starts.`;
}

/**
 * `present` with an empty `items` — the one reply the spec forbids a daemon
 * to send (§5.1: a collapsed absence). "Published" and "listed none" cannot
 * both hold, so the contradiction is named and the control stays free text,
 * never a select with nothing to select.
 */
function emptyPresentVocabularyText(axisWord: "models" | "modes"): string {
  return `The daemon answered that this provider publishes its ${axisWord} and then listed none — a contradiction on the wire. Type the one to use; what you type is checked when the session starts.`;
}

/**
 * A `state` outside the `present`/`none`/`absent` union — a newer daemon's
 * fourth value, or corrupt wire. The received value is shown, never guessed
 * into one of the known states, and the field is never left without a
 * sentence: silence is the one answer that is never honest here.
 */
function unknownStateVocabularyText(axisWord: "models" | "modes", state: unknown): string {
  return `The daemon answered for the ${axisWord} axis with a value this app does not know (${
    JSON.stringify(state) ?? "undefined"
  }); it is none of present, none or absent. Type the one to use; what you type is checked when the session starts.`;
}

/**
 * What one vocabulary axis renders, decided in one place so the two axes
 * cannot drift: `freeText` picks the control, `hint` names WHICH state the
 * axis is in, `items` feed the select. The axis is read as the untrusted
 * wire value it is — every state it can reach, including the malformed and
 * the unknown, is named here, and absent is a third state that never
 * borrows another state's answer.
 */
function vocabularyAxisView<T>(
  axis: { state: unknown; origin?: unknown; items?: readonly T[] } | undefined,
  replyArrived: boolean,
  queryFailed: boolean,
  axisWord: "models" | "modes",
  toItem: (item: T) => { value: string; label: string },
): { freeText: boolean; hint?: ReactNode; items: { value: string; label: string }[] } {
  // The query itself failed: the failure paragraph above the fields names
  // the transport reason once, and the fields stay free text under it.
  if (queryFailed) {
    return { freeText: true, items: [] };
  }
  if (axis === undefined) {
    // A reply that arrived without this axis is malformed, not absent.
    if (replyArrived) {
      return { freeText: true, hint: malformedVocabularyText(axisWord), items: [] };
    }
    // Still in flight: the fields are not rendered while the ask is out.
    return { freeText: true, items: [] };
  }
  if (axis.state === "present") {
    const items = axis.items ?? [];
    // `present` with nothing listed is the contradiction the spec forbids.
    if (items.length === 0) {
      return { freeText: true, hint: emptyPresentVocabularyText(axisWord), items: [] };
    }
    return {
      freeText: false,
      hint:
        axis.origin === "daemon"
          ? DAEMON_VOCABULARY_TEXT
          : axis.origin === "provider"
            ? undefined
            : undeclaredOriginVocabularyText(axisWord),
      items: items.map(toItem),
    };
  }
  if (axis.state === "none") {
    return { freeText: true, hint: noneVocabularyText(axisWord), items: [] };
  }
  if (axis.state === "absent") {
    return { freeText: true, hint: absentVocabularyText(axisWord), items: [] };
  }
  return { freeText: true, hint: unknownStateVocabularyText(axisWord, axis.state), items: [] };
}

/**
 * For an ACP provider whose modes are `absent`: the mode a session actually
 * runs in when the agent declares none. Prefilled once, labelled a
 * suggestion — never rendered as if the provider had said it.
 */
const ACP_MODE_SUGGESTION = "default";
const ACP_MODE_SUGGESTION_TEXT =
  'Suggested: "default" — the mode a session of this agent runs in when it declares none. A suggestion, not something the provider reported.';

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
 * The two text caps every profile write enforces — renaming an existing row
 * and creating a new one. Returns the refusal sentence sized in the daemon's
 * own units, or null when both texts fit. The name counts Unicode scalar
 * values (the daemon's `chars().count()`), the note UTF-8 bytes
 * (`String::len`); refusals name the size and nothing is ever truncated.
 */
function profileTextsError(trimmedName: string, note: string): string | null {
  const trimmedChars = charCount(trimmedName);
  if (trimmedChars === 0) {
    return `A profile name is 1 to ${MAX_PROFILE_NAME_CHARS} characters.`;
  }
  if (trimmedChars > MAX_PROFILE_NAME_CHARS) {
    return `This name is ${trimmedChars} characters, over the ${MAX_PROFILE_NAME_CHARS}-character cap. Nothing was saved and nothing was truncated.`;
  }
  const noteBytes = utf8Bytes(note);
  if (noteBytes > MAX_PROFILE_NOTE_BYTES) {
    return `This note is ${noteBytes} bytes, over the ${MAX_PROFILE_NOTE_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  return null;
}

/** What the new-profile form hands the panel on save. The panel validates and persists. */
interface NewProfileDraft {
  name: string;
  note: string;
  provider: string;
  model: string;
  modeId: string;
  autoAccept: boolean;
  enabledForAgents: boolean;
  restrictPeers: boolean;
}

/**
 * One vocabulary axis of the new-profile form: a select over the provider's
 * published items, or a free-text field when the answer is `none` or
 * `absent` or there is no answer at all. The `hint` names WHICH of those
 * happened: each state's sentence carries its own reason clause, no two
 * rendered sentences are equal or substrings of one another (the
 * sentence-orthogonality test holds them pairwise), though the sentences
 * deliberately share tail clauses.
 */
function VocabularyField({
  label,
  value,
  busy,
  freeText,
  hint,
  suggestion,
  items,
  onChange,
}: {
  label: string;
  value: string;
  busy: boolean;
  /** True: a free-text input. False: a select over `items`. */
  freeText: boolean;
  /** The sentence under the field naming why it reads what it reads. */
  hint?: ReactNode;
  /** The ACP mode suggestion, only where it applies; labelled a suggestion. */
  suggestion?: ReactNode;
  items: readonly { value: string; label: string }[];
  onChange: (next: string) => void;
}) {
  if (freeText) {
    return (
      <>
        <label className="device-field">
          {label}
          <input
            aria-label={label}
            value={value}
            disabled={busy}
            onChange={(event) => onChange(event.target.value)}
          />
        </label>
        {hint === undefined ? null : <p className="device-field-hint">{hint}</p>}
        {suggestion === undefined ? null : <p className="device-field-hint">{suggestion}</p>}
      </>
    );
  }
  return (
    <label className="device-field">
      {label}
      <select
        aria-label={label}
        value={value}
        disabled={busy}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="">Choose a {label.toLowerCase()}…</option>
        {items.map((item) => (
          <option key={item.value} value={item.value}>
            {item.label}
          </option>
        ))}
      </select>
      {hint === undefined ? null : <span className="device-field-hint">{hint}</span>}
    </label>
  );
}

/**
 * The new-profile form, inline in the Agents panel — the panel's own shape
 * (its editor and delete confirm are inline too; nothing here needs a modal).
 * Its one hard rule: model and modeId are the provider's own vocabulary,
 * stored verbatim, so the form never invents one. It asks — through the
 * `provider_vocabulary` handshake gate — and renders the answer's three
 * states distinctly; when the daemon predates the query it says so in its
 * own words and falls back to free text, so a human can always finish.
 *
 * The vocabulary refetch on a provider change rides the same sequence-guard
 * cadence as the panel's document load: only the newest fetch may apply, so
 * a reply for the previously selected provider never lands in a form that
 * now shows another one.
 *
 * There is no thinking-option field on purpose: the vocabulary reply carries
 * no thinking axis (spec §4), and an empty control would invent one. A new
 * profile saves `thinkingOptionId: null`.
 */
function NewAgentProfileForm({
  providers,
  catalogLoading,
  catalogError,
  vocabularySupported,
  busy,
  onCreate,
  onCancel,
}: {
  /** Installed providers only, catalog order. */
  providers: readonly ProviderInfo[];
  catalogLoading: boolean;
  catalogError: ErrorSentence | null;
  /** True only when the handshake advertised `provider_vocabulary`. */
  vocabularySupported: boolean;
  /** True while a panel write is in flight: Save must not start another. */
  busy: boolean;
  onCreate: (draft: NewProfileDraft) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [note, setNote] = useState("");
  const [providerId, setProviderId] = useState("");
  const [model, setModel] = useState("");
  const [mode, setMode] = useState("");
  const [autoAccept, setAutoAccept] = useState(false);
  const [enabledForAgents, setEnabledForAgents] = useState(false);
  const [restrictPeers, setRestrictPeers] = useState(false);
  const [vocabulary, setVocabulary] = useState<ProviderVocabulary | null>(null);
  const [vocabularyError, setVocabularyError] = useState<ErrorSentence | null>(null);
  // Monotonic fetch sequence for the vocabulary query: a reply may apply
  // only while it is still the newest fetch. This — never the provider id
  // echoed back — is what keeps a slow answer for provider A out of a form
  // now showing provider B, so the guard is the single mechanism the
  // stale-reply test mutates.
  const vocabularySeqRef = useRef(0);

  // The catalog lands after the first paint; default the picker to the first
  // installed provider once there is one, and let the vocabulary effect run.
  useEffect(() => {
    if (!catalogLoading && providerId === "" && providers.length > 0) {
      setProviderId(providers[0].id);
    }
  }, [catalogLoading, providerId, providers]);

  useEffect(() => {
    // A daemon that never advertised `provider_vocabulary` would refuse this
    // request: it is never sent. The form's free-text fallback and the
    // older-daemon sentence are the whole UI for that case.
    if (!vocabularySupported || providerId === "") return;
    // No `cancelled` flag beside the sequence guard on purpose: every path
    // that could make a reply stale (the provider changed, the form closed)
    // bumps the sequence, so the guard below is the one mechanism — and the
    // one thing the stale-reply test mutates. A `setState` after unmount is
    // a safe no-op in React 18+.
    const seq = ++vocabularySeqRef.current;
    setVocabulary(null);
    setVocabularyError(null);
    void providerVocabularyGet(providerId, false)
      .then((reply) => {
        // A newer fetch (the provider changed again) owns the form: this
        // reply is stale no matter which provider it names.
        if (vocabularySeqRef.current !== seq) return;
        // The reply is adopted as it arrived; the per-axis reader below
        // treats it as the untrusted wire value it is. A reply missing an
        // axis is malformed — its own state, named in the render — and must
        // not throw: throwing here would report a successful query as
        // failed and discard the axis that arrived intact.
        setVocabulary(reply);
        // The one prefill allowed: an ACP agent that declared no modes runs
        // in "default". Typed text is never clobbered — the suggestion only
        // fills an empty field, and it is labelled a suggestion.
        if (reply.modes?.state === "absent") {
          const info = providers.find((provider) => provider.id === providerId);
          if (info?.protocol === "acp") {
            setMode((current) => (current === "" ? ACP_MODE_SUGGESTION : current));
          }
        }
      })
      .catch((cause: unknown) => {
        if (vocabularySeqRef.current !== seq) return;
        setVocabularyError(errorSentence(cause));
      });
  }, [providerId, vocabularySupported, providers]);

  // The reply (or its failure) is what the fields read; before either, the
  // form shows the ask in flight and renders no field to guess into.
  const vocabularyKnown = vocabulary !== null || vocabularyError !== null;
  // One decision per axis, made by the shared reader: every state the wire
  // can reach — present, none, absent, malformed, the contradiction, the
  // unknown — is named there.
  const modelsView = vocabularyAxisView(
    vocabulary?.models,
    vocabulary !== null,
    vocabularyError !== null,
    "models",
    (item) => ({
      value: item.modelId,
      label:
        item.name && item.name !== item.modelId ? `${item.name} (${item.modelId})` : item.modelId,
    }),
  );
  const modesView = vocabularyAxisView(
    vocabulary?.modes,
    vocabulary !== null,
    vocabularyError !== null,
    "modes",
    (item) => ({
      value: item.id,
      label: item.name && item.name !== item.id ? `${item.name} (${item.id})` : item.id,
    }),
  );
  const noteBytes = utf8Bytes(note);

  function changeProvider(next: string) {
    // Reset the fields that depend on the answer before the fetch starts:
    // the old provider's selection must not survive into the new one.
    setProviderId(next);
    setModel("");
    setMode("");
  }

  function submit() {
    onCreate({
      name,
      note,
      provider: providerId,
      model,
      modeId: mode,
      autoAccept,
      enabledForAgents,
      restrictPeers,
    });
  }

  return (
    <div className="agent-inline-editor agent-profile-create">
      <span className="settings-subheading">New profile</span>
      <label className="device-field">
        Name
        <input
          aria-label="Profile name"
          value={name}
          disabled={busy}
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <label className="device-field">
        Note — what a creating agent reads to choose this profile. Write it for the agent.
        <textarea
          aria-label="Profile note"
          value={note}
          disabled={busy}
          rows={3}
          onChange={(event) => setNote(event.target.value)}
        />
        <span className="agent-byte-counter">
          {noteBytes} / {MAX_PROFILE_NOTE_BYTES} bytes
        </span>
      </label>
      <label className="device-field">
        Provider
        <select
          aria-label="Provider"
          value={providerId}
          disabled={busy || catalogLoading || providers.length === 0}
          onChange={(event) => changeProvider(event.target.value)}
        >
          {catalogLoading ? <option value="">Looking for installed providers…</option> : null}
          {!catalogLoading && catalogError !== null ? (
            <option value="">The catalog could not be read</option>
          ) : null}
          {!catalogLoading && catalogError === null && providers.length === 0 ? (
            <option value="">No provider installed</option>
          ) : null}
          {providers.map((provider) => (
            <option key={provider.id} value={provider.id}>
              {provider.id}
            </option>
          ))}
        </select>
      </label>
      {catalogError !== null ? (
        <p className="device-field-hint" role="alert">
          <ErrorText
            sentence={`The provider catalog could not be read: ${catalogError.sentence}`}
            detail={catalogError.detail}
            id="settings-profile-catalog-error"
          />
        </p>
      ) : null}
      {/* A completed read that found nothing is the only state allowed to
          claim "no agent CLI": not-read is not empty, and a failed read is
          its own fact — the paragraph above names it. */}
      {!catalogLoading && catalogError === null && providers.length === 0 ? (
        <p className="device-field-hint">
          No agent CLI is installed on this machine: install one and restart Devboule, then create
          the profile.
        </p>
      ) : null}
      {!vocabularySupported ? (
        <p className="device-field-hint">{VOCABULARY_UNAVAILABLE_TEXT}</p>
      ) : null}
      {vocabularySupported && providerId !== "" && !vocabularyKnown ? (
        <div role="status">Asking the daemon what {providerId} offers…</div>
      ) : null}
      {vocabularyError !== null ? (
        <p className="device-field-hint">
          <ErrorText
            sentence={`The vocabulary query failed (${vocabularyError.sentence}); type the model and mode below; what you type is checked when the session starts.`}
            detail={vocabularyError.detail}
            id="settings-vocabulary-error"
          />
        </p>
      ) : null}
      {(!vocabularySupported || vocabularyKnown) && providers.length > 0 ? (
        <>
          <VocabularyField
            label="Model"
            value={model}
            busy={busy}
            freeText={modelsView.freeText}
            hint={modelsView.hint}
            items={modelsView.items}
            onChange={setModel}
          />
          <VocabularyField
            label="Mode"
            value={mode}
            busy={busy}
            freeText={modesView.freeText}
            hint={modesView.hint}
            suggestion={
              vocabulary?.modes?.state === "absent" &&
              providers.find((provider) => provider.id === providerId)?.protocol === "acp"
                ? ACP_MODE_SUGGESTION_TEXT
                : undefined
            }
            items={modesView.items}
            onChange={setMode}
          />
        </>
      ) : null}
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          aria-label="Auto accept for children of this profile"
          checked={autoAccept}
          disabled={busy}
          onChange={(event) => setAutoAccept(event.target.checked)}
        />
        <span>
          <span>Auto accept</span>
          <span className="agent-profile-tick-note">
            Children created from this profile approve their own permission prompts instead of
            asking you. This is the most consequential control on the form: leave it off unless you
            mean it.
          </span>
        </span>
      </label>
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          aria-label="Available to agents"
          checked={enabledForAgents}
          disabled={busy}
          onChange={(event) => setEnabledForAgents(event.target.checked)}
        />
        <span>
          <span>Agents may create this</span>
          <span className="agent-profile-tick-note">
            Lets an agent start this kind of agent. If this profile answers its own permission
            cards, its children run unattended.
          </span>
        </span>
      </label>
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          aria-label="Children cannot message peers or create further agents"
          checked={restrictPeers}
          disabled={busy}
          onChange={(event) => setRestrictPeers(event.target.checked)}
        />
        <span>
          <span>No peer contact and no further agents for children</span>
          <span className="agent-profile-tick-note">
            Children created from this profile cannot message other agents or create further agents.
            They keep the agent roster, their read-only view.
          </span>
        </span>
      </label>
      <div className="device-actions">
        <button
          type="button"
          className="settings-device-action"
          disabled={busy || catalogLoading || providers.length === 0}
          onClick={submit}
        >
          Create profile
        </button>
        <button type="button" className="settings-device-action" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}

/**
 * One row's name/note editor — the only fields editable here on purpose.
 * Provider, model, mode, thinking option, features and the peer-contact
 * restriction are the provider's own vocabulary and the human's saved deny
 * list, stored verbatim, so this editor leaves them exactly as the
 * daemon holds them and the row displays them; the way to different values is
 * a new profile ([`NewAgentProfileForm`], which asks the daemon for the
 * vocabulary), not editing this one. The name is capped in characters, the
 * note in UTF-8 bytes — both refusals name the size, and nothing is ever
 * truncated.
 *
 * The fields are controlled from the panel: the draft lives one level up
 * (`editorDraft`, rule 3 of the write discipline), because the row this
 * editor renders in can be removed by an in-flight delete and restored by
 * that delete's revert — a draft kept in local state would unmount with the
 * row and remount empty.
 *
 * Save sits under the panel's `busy` lock like every other write trigger:
 * while a write is in flight the editor cannot start a second one, so a
 * revert can never land on a document the daemon has just refused.
 */
function AgentProfileEditor({
  name,
  note,
  busy,
  onFieldChange,
  onSave,
  onClose,
}: {
  /** The draft the panel holds for this editor: the fields' values. */
  name: string;
  note: string;
  /** True while a panel write is in flight: Save must not start another. */
  busy: boolean;
  /** Every keystroke, reported up so the draft survives this component. */
  onFieldChange: (name: string, note: string) => void;
  onSave: (name: string, note: string) => void;
  onClose: () => void;
}) {
  const noteBytes = utf8Bytes(note);
  return (
    <div className="agent-inline-editor">
      <label className="device-field">
        Name
        <input value={name} onChange={(event) => onFieldChange(event.target.value, note)} />
      </label>
      <label className="device-field">
        Note — what a creating agent reads to choose this profile. Write it for the agent.
        <textarea
          value={note}
          onChange={(event) => onFieldChange(name, event.target.value)}
          rows={3}
        />
        <span className="agent-byte-counter">
          {noteBytes} / {MAX_PROFILE_NOTE_BYTES} bytes
        </span>
      </label>
      <p className="device-field-hint">
        Provider, model, mode, features and the peer-contact restriction are shown on the row and
        are not editable here. To change them, create a new profile with the values you want and
        delete this one.
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
 * - The new-profile form ([`NewAgentProfileForm`]), which asks the daemon what
 *   a provider offers instead of inventing vocabulary, and falls back to free
 *   text — naming its own reason — when the daemon predates the query. It
 *   saves through the same `persist` path as every other write here.
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
  const [editorDraft, setEditorDraft] = useState<{ id: string; name: string; note: string } | null>(
    null,
  );
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

  function saveProfileFields(id: string, name: string, note: string) {
    const current = documentRef.current;
    if (current === null) return;
    const trimmed = name.trim();
    // The caps are shared with the new-profile form: the name counts Unicode
    // scalar values — the daemon's `chars().count()` — not UTF-16 code units;
    // the note counts UTF-8 bytes. Refuse and name the size; never clip.
    const refusal = profileTextsError(trimmed, note);
    if (refusal !== null) {
      setError({ sentence: refusal, detail: null });
      return;
    }
    const updated = cloneDocument(current);
    const row = updated.profiles.find((profile) => profile.id === id);
    if (row === undefined) return;
    row.name = trimmed;
    row.note = note;
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
    const refusal = profileTextsError(trimmedName, draft.note);
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
      // The vocabulary reply carries no thinking axis, so the form offers
      // none; a new profile starts without one.
      thinkingOptionId: null,
      features: draft.autoAccept ? { autoAccept: true } : {},
      // The human's tick, not an agent's argument: a profile that denies
      // peer contact makes children that cannot message peers or create
      // further agents. Unticked saves nothing, exactly as before.
      toolOverlay: toolOverlayForPeerRestriction(draft.restrictPeers),
      // Default off, always: a profile that becomes agent-reachable the
      // moment it is saved is a profile nobody deliberately ticked.
      enabledForAgents: draft.enabledForAgents,
    };
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
          <NewAgentProfileForm
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
                  {/* Read-only: provider, model, mode and the overlay are the provider's own
                      vocabulary and the human's saved deny list, so they are shown, not edited.
                      To change them, create a new profile and delete this one. */}
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
                      // session. Closing one is the human abandoning it.
                      if (editing) closeEditor();
                      else {
                        setEditingId(profile.id);
                        setEditorDraft(null);
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
                  <AgentProfileEditor
                    name={editorDraft?.id === profile.id ? editorDraft.name : profile.name}
                    note={editorDraft?.id === profile.id ? editorDraft.note : profile.note}
                    busy={busy}
                    onFieldChange={(name, note) => setEditorDraft({ id: profile.id, name, note })}
                    onSave={(name, note) => saveProfileFields(profile.id, name, note)}
                    onClose={closeEditor}
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
                      workspace.path ? (
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
      <NotificationSoundSetting />
      <JournalRetentionPanel />
    </div>
  );
}
