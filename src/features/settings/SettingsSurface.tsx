import { Fragment, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, RefObject } from "react";
import { useAppStore } from "../../store/appStore";
import { useWorkspaceDaemon } from "../workspace/workspaceDaemon";
import { daemonDotTone, daemonLabel } from "../workspace/sidebar/SidebarFooter";
import { DiagnosticsPanel } from "./DiagnosticsPanel";
import { DevicesPanel } from "./DevicesPanel";
import { OraclePanel } from "../oracle/OraclePanel";
import { AppearanceSection } from "./AppearanceSection";
import { CloseBehaviorSetting } from "./CloseBehaviorSetting";
import { SendBehaviorSetting } from "./SendBehaviorSetting";
import { JournalRetentionPanel } from "./JournalRetentionPanel";
import { ProvidersPanel } from "./panels/ProvidersPanel";
import { AgentProfilesPanel } from "./panels/AgentsPanel";
import { ProjectsPanel } from "./panels/ProjectsPanel";
import {
  SETTINGS_MENU,
  SETTINGS_PAGE_ORDER,
  settingsPageById,
  type SettingsMenuPage,
  type SettingsPageId,
} from "./settingsMenu";
import { SettingsMenuIcon } from "./menuIcons";
import "./settings.css";

/** The quiet line under every page with no function yet. */
const EMPTY_PAGE_NOTE = "This page is not available yet.";

function SettingsEmptyPage({
  page,
  titleRef,
}: {
  page: SettingsMenuPage;
  titleRef: RefObject<HTMLHeadingElement | null>;
}) {
  const titleId = `settings-page-title-${page.id}`;
  return (
    <section aria-labelledby={titleId}>
      <h2 className="settings-page-title" id={titleId} ref={titleRef} tabIndex={-1}>
        {page.label}
      </h2>
      <p className="settings-page-intro">{page.intro}</p>
      <p className="settings-page-empty">{EMPTY_PAGE_NOTE}</p>
      {page.note ? <p className="settings-page-empty">{page.note}</p> : null}
    </section>
  );
}

export function SettingsSurface() {
  const [activePage, setActivePage] = useState<SettingsPageId>("providers");
  const selectSurface = useAppStore((state) => state.selectSurface);
  const daemon = useWorkspaceDaemon();
  const menuRef = useRef<HTMLElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLHeadingElement | null>(null);
  const previousPage = useRef<SettingsPageId>(activePage);

  // Focus follows navigation, never the mount: the title of an empty page,
  // otherwise the content top. The ref comparison (not a first-render flag)
  // is what survives StrictMode's double effect.
  useEffect(() => {
    if (previousPage.current === activePage) return;
    previousPage.current = activePage;
    const content = contentRef.current;
    if (!content) return;
    const title = content.querySelector<HTMLElement>(".settings-page-title");
    (title ?? content).focus();
  }, [activePage]);

  function handleMenuKeyDown(event: KeyboardEvent<HTMLButtonElement>, id: SettingsPageId) {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      setActivePage(id);
      return;
    }
    const index = SETTINGS_PAGE_ORDER.indexOf(id);
    let next: number | null = null;
    if (event.key === "ArrowDown") next = (index + 1) % SETTINGS_PAGE_ORDER.length;
    else if (event.key === "ArrowUp")
      next = (index - 1 + SETTINGS_PAGE_ORDER.length) % SETTINGS_PAGE_ORDER.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = SETTINGS_PAGE_ORDER.length - 1;
    else return;
    event.preventDefault();
    menuRef.current
      ?.querySelector<HTMLElement>(`[data-settings-page="${SETTINGS_PAGE_ORDER[next]}"]`)
      ?.focus();
  }

  function renderContent() {
    switch (activePage) {
      case "appearance":
        return <AppearanceSection />;
      case "diagnostics":
        return (
          <>
            <DiagnosticsPanel />
            <div className="settings-subheading">Journal storage</div>
            <JournalRetentionPanel />
            <div className="settings-subheading">Window</div>
            <CloseBehaviorSetting />
            <div className="settings-subheading">Message sending</div>
            <SendBehaviorSetting />
          </>
        );
      case "providers":
        return <ProvidersPanel />;
      case "profiles":
        return <AgentProfilesPanel />;
      case "projects":
        return <ProjectsPanel />;
      case "oracle":
        return <OraclePanel />;
      case "paired":
        return <DevicesPanel />;
      default:
        return <SettingsEmptyPage page={settingsPageById(activePage)} titleRef={titleRef} />;
    }
  }

  const hostSentence = daemonLabel(daemon);

  return (
    <section className="surface-card settings-surface" aria-label="Settings">
      <nav className="settings-menu" aria-label="Settings pages" ref={menuRef}>
        <button
          type="button"
          className="settings-back-row"
          onClick={() => selectSurface("workspace")}
        >
          <SettingsMenuIcon id="back" />
          Back to workspace
        </button>
        {SETTINGS_MENU.map((group) => (
          <Fragment key={group.label}>
            <div className="settings-menu-group" data-settings-group role="presentation">
              <span className="settings-menu-group-label">{group.label}</span>
            </div>
            {group.host ? (
              <div className="settings-host-row" title={hostSentence}>
                <SettingsMenuIcon id="host" />
                <span>This PC</span>
                <span
                  className={`settings-host-dot settings-host-dot-${daemonDotTone(daemon.state)}`}
                  role="img"
                  aria-label={hostSentence}
                />
              </div>
            ) : null}
            {group.pages.map((page) => (
              <button
                key={page.id}
                type="button"
                className={`settings-menu-row${activePage === page.id ? " settings-menu-row-active" : ""}`}
                data-settings-page={page.id}
                aria-current={activePage === page.id ? "page" : undefined}
                onClick={() => setActivePage(page.id)}
                onKeyDown={(event) => handleMenuKeyDown(event, page.id)}
              >
                <SettingsMenuIcon id={page.id} />
                {page.label}
              </button>
            ))}
          </Fragment>
        ))}
      </nav>

      <div className="settings-main">
        <div className="settings-main-inner" ref={contentRef} tabIndex={-1} data-settings-content>
          {renderContent()}
        </div>
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

// The surface keeps the names its importers use: the heading its sibling
// panels render, and the three symbols the panel tests import from this path.
export { ALWAYS_ON_REASON, toolPolicyFor } from "./providerStatus";
export { DelegationSetting } from "./panels/AgentsPanel";
