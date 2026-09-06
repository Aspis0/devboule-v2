import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { providerUpdate, providersList, providersRefresh, reasonFromCause } from "../../lib/tauri";
import type { ProviderCatalog, ProviderInfo } from "../../types/ipc";
import { OraclePanel } from "../oracle/OraclePanel";
import { JournalRetentionPanel } from "./JournalRetentionPanel";
import {
  MOCK_DEVICES,
  MOCK_GENERAL_SETTINGS,
  MOCK_LABS,
  MOCK_PROJECTS,
  MOCK_SETTINGS_TABS,
  MOCK_WORKTREE_DEFAULTS,
  type SettingsTab,
} from "./mockData";
import "./settings.css";

export function SettingsSurface() {
  const [activeTab, setActiveTab] = useState<SettingsTab>("providers");

  const settingsTabs = MOCK_SETTINGS_TABS.map((tab) => ({
    ...tab,
    active: activeTab === tab.id,
  }));
  function handleTabKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    const currentIndex = MOCK_SETTINGS_TABS.findIndex((tab) => tab.id === activeTab);
    let nextIndex = currentIndex;

    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      nextIndex = (currentIndex + 1) % MOCK_SETTINGS_TABS.length;
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      nextIndex = (currentIndex - 1 + MOCK_SETTINGS_TABS.length) % MOCK_SETTINGS_TABS.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = MOCK_SETTINGS_TABS.length - 1;
    } else {
      return;
    }

    event.preventDefault();
    setActiveTab(MOCK_SETTINGS_TABS[nextIndex].id);
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
      case "devices":
        return <DevicesPanel />;
      case "general":
        return <GeneralPanel />;
      case "labs":
        return <LabsPanel />;
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
        <button className="settings-lock-button" type="button">
          Lock app
        </button>
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

function SettingsHeading({ title, description }: SettingsHeadingProps) {
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
  return (
    <div id="settings-panel-projects" role="tabpanel" aria-label="Projects">
      <SettingsHeading
        title="Projects"
        description="A project is a git repository or any directory this daemon can reach. Workspaces live inside it."
      />
      <div className="settings-stack settings-stack-spaced">
        {MOCK_PROJECTS.map((project) => (
          <div className="settings-card settings-project-card" key={project.name}>
            <span className="settings-card-copy">
              <span className="settings-card-title">{project.name}</span>
              <span className="settings-card-meta">{project.path}</span>
            </span>
            <span className="settings-card-value">{project.workspaces}</span>
          </div>
        ))}
        <button className="settings-dashed-action" type="button">
          <span aria-hidden="true">+</span>Add project
        </button>
      </div>

      <div className="settings-subheading">Worktree defaults</div>
      <div className="settings-stack settings-stack-tight">
        {MOCK_WORKTREE_DEFAULTS.map((setting) => (
          <SettingValue
            key={setting.label}
            label={setting.label}
            value={setting.value}
            tone={setting.tone}
          />
        ))}
      </div>
    </div>
  );
}

function DevicesPanel() {
  return (
    <div id="settings-panel-devices" role="tabpanel" aria-label="Devices">
      <SettingsHeading
        title="Devices"
        description="Paired clients that may drive this daemon. Pairing is per-device and revocable."
      />
      <div className="settings-stack settings-stack-tight settings-devices-list">
        {MOCK_DEVICES.map((device) => (
          <div className="settings-card settings-device-card" key={device.name}>
            <span className={`device-dot device-dot-${device.tone}`} aria-hidden="true" />
            <span className="settings-device-name">{device.name}</span>
            <span className="settings-card-value">{device.state}</span>
          </div>
        ))}
        <button className="settings-dashed-action" type="button">
          <span aria-hidden="true">+</span>Pair a device
        </button>
      </div>
    </div>
  );
}

function GeneralPanel() {
  return (
    <div id="settings-panel-general" role="tabpanel" aria-label="General">
      <JournalRetentionPanel />
      <div className="settings-stack settings-stack-tight settings-general-list">
        {MOCK_GENERAL_SETTINGS.map((setting) => (
          <SettingValue
            key={setting.label}
            label={setting.label}
            value={setting.value}
            tone={setting.tone}
          />
        ))}
      </div>
    </div>
  );
}

function LabsPanel() {
  return (
    <div id="settings-panel-labs" role="tabpanel" aria-label="Labs">
      <SettingsHeading
        title="Labs"
        description="Unfinished surfaces. Turning one on adds its globe to the crescent."
      />
      <div className="settings-stack settings-stack-tight">
        {MOCK_LABS.map((lab) => (
          <LabRow key={lab.title} title={lab.title} description={lab.description} />
        ))}
      </div>
    </div>
  );
}

function SettingValue({
  label,
  value,
  tone = "default",
}: {
  label: string;
  value: string;
  tone?: "default" | "green" | "danger" | "muted";
}) {
  return (
    <div className="settings-card settings-value-row">
      <span>{label}</span>
      <span className={`settings-card-value settings-value-${tone}`}>{value}</span>
    </div>
  );
}

function LabRow({ title, description }: { title: string; description: string }) {
  return (
    <div className="settings-card settings-lab-row">
      <span className="settings-card-copy">
        <span className="settings-card-title">{title}</span>
        <span className="settings-card-meta settings-lab-description">{description}</span>
      </span>
      <span className="settings-card-value settings-value-green">on</span>
    </div>
  );
}
