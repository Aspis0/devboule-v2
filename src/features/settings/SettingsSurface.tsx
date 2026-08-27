import { useState } from "react";
import type { KeyboardEvent } from "react";
import { OraclePanel } from "../oracle/OraclePanel";
import {
  MOCK_DEFAULT_MODELS,
  MOCK_DEVICES,
  MOCK_GENERAL_SETTINGS,
  MOCK_LABS,
  MOCK_PROJECTS,
  MOCK_PROVIDERS,
  MOCK_PROVIDER_ENABLED,
  MOCK_SETTINGS_TABS,
  MOCK_WORKTREE_DEFAULTS,
  type ProviderId,
  type SettingsTab,
} from "./mockData";
import "./settings.css";

export function SettingsSurface() {
  const [activeTab, setActiveTab] = useState<SettingsTab>("providers");
  const [providerEnabled, setProviderEnabled] = useState(MOCK_PROVIDER_ENABLED);

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

  function toggleProvider(providerId: ProviderId) {
    setProviderEnabled((current) => ({
      ...current,
      [providerId]: !current[providerId],
    }));
  }

  const providers = MOCK_PROVIDERS.map((provider) => {
    const on = providerEnabled[provider.id];
    // PATH discovery and authentication are separate mock facts by design.
    const status = !provider.installed
      ? "not installed"
      : on && provider.authenticated
        ? "ready"
        : "disabled";
    const statusTone =
      status === "ready" ? "ready" : status === "not installed" ? "missing" : "disabled";

    return {
      ...provider,
      on,
      status,
      statusTone,
    };
  });

  function renderActivePanel() {
    switch (activeTab) {
      case "providers":
        return (
          <div id="settings-panel-providers" role="tabpanel" aria-label="Providers and models">
            <SettingsHeading
              title="Providers & models"
              description="Devboule wraps the CLIs already installed on this machine. Enable one and it appears in the workspace composer."
            />
            <div className="provider-list">
              {providers.map((provider) => (
                <div className="provider-card" key={provider.id}>
                  <span
                    className={`provider-mark provider-mark-${provider.tone}`}
                    aria-hidden="true"
                  >
                    {provider.initial}
                  </span>
                  <span className="provider-copy">
                    <span className="provider-name">{provider.name}</span>
                    <span className="provider-detail">{provider.detail}</span>
                  </span>
                  <span className="provider-controls">
                    <span className={`provider-status provider-status-${provider.statusTone}`}>
                      {provider.status}
                    </span>
                    <button
                      className={`provider-switch${provider.on ? " provider-switch-on" : ""}`}
                      type="button"
                      role="switch"
                      aria-checked={provider.on}
                      aria-label={`Enable ${provider.name}`}
                      onClick={() => toggleProvider(provider.id)}
                    >
                      <span className="provider-switch-knob" aria-hidden="true" />
                    </button>
                  </span>
                </div>
              ))}
            </div>

            <div className="settings-subheading">Default model per surface</div>
            <div className="default-model-grid">
              {MOCK_DEFAULT_MODELS.map((model) => (
                <ModelChoice key={model.title} title={model.title} value={model.value} />
              ))}
            </div>
          </div>
        );
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

function ModelChoice({ title, value }: { title: string; value: string }) {
  return (
    <div className="model-choice">
      <div className="model-choice-title">{title}</div>
      <button
        className="model-choice-control"
        type="button"
        aria-label={`Choose default model for ${title}`}
      >
        {value}
        <span aria-hidden="true">▾</span>
      </button>
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
      <SettingsHeading title="General" />
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
