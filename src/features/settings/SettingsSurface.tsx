import { useState } from "react";
import type { KeyboardEvent } from "react";
import { DiagnosticsPanel } from "./DiagnosticsPanel";
import { DevicesPanel } from "./DevicesPanel";
import { OraclePanel } from "../oracle/OraclePanel";
import { ProvidersPanel } from "./panels/ProvidersPanel";
import { AgentProfilesPanel } from "./panels/AgentsPanel";
import { ProjectsPanel } from "./panels/ProjectsPanel";
import { GeneralPanel } from "./panels/GeneralPanel";
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

// The surface keeps the names its importers use: the shell above, the
// heading its sibling panels render, and the three symbols the panel tests
// import from this path.
export { ALWAYS_ON_REASON, toolPolicyFor } from "./providerStatus";
export { DelegationSetting } from "./panels/AgentsPanel";
