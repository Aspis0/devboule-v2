import { Fragment, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, RefObject } from "react";
import { useAppStore } from "../../store/appStore";
import { DiagnosticsPanel } from "./DiagnosticsPanel";
import { DevicesPanel } from "./DevicesPanel";
import { OraclePanel } from "../oracle/OraclePanel";
import { AppearanceSection } from "./AppearanceSection";
import { CloseBehaviorSetting } from "./CloseBehaviorSetting";
import { SendBehaviorSetting } from "./SendBehaviorSetting";
import { NotificationsSection } from "./NotificationsSection";
import { JournalRetentionPanel } from "./JournalRetentionPanel";
import { UsagePanel } from "./UsagePanel";
import { ProvidersPanel } from "./panels/ProvidersPanel";
import { AgentProfilesPanel } from "./panels/AgentsPanel";
import { ProjectsPanel } from "./panels/ProjectsPanel";
import { AboutPanel } from "./panels/AboutPanel";
import {
  SETTINGS_MENU,
  SETTINGS_PAGE_ORDER,
  settingsPageById,
  type SettingsMenuPage,
  type SettingsPageId,
} from "./settingsMenu";
import { SettingsMenuIcon } from "./menuIcons";
import { HostDot } from "./HostDot";
import "./settings.css";

/** The quiet line under every page with no function yet. */
const EMPTY_PAGE_NOTE = "This page is not available yet.";

function SettingsPageHeader({
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
      {page.unavailable === true ? <p className="settings-page-empty">{EMPTY_PAGE_NOTE}</p> : null}
      {page.note ? <p className="settings-page-empty">{page.note}</p> : null}
    </section>
  );
}

export function SettingsSurface() {
  const [activePage, setActivePage] = useState<SettingsPageId>("providers");
  const selectSurface = useAppStore((state) => state.selectSurface);
  const menuRef = useRef<HTMLElement>(null);
  const titleRef = useRef<HTMLHeadingElement | null>(null);
  const previousPage = useRef<SettingsPageId>(activePage);
  // Clicks move focus to the page title; keyboard activation deliberately
  // does not, so arrow travel through the menu keeps working (F4). The
  // effect reads this, never the event.
  const focusTitleOnChange = useRef(false);
  const [liveMessage, setLiveMessage] = useState("");

  // Focus follows a click navigation to the page title, never the mount.
  // The ref comparison (not a first-render flag) is what survives
  // StrictMode's double effect. A keyboard activation instead announces the
  // new page through the live region and leaves focus on its row.
  useEffect(() => {
    if (previousPage.current === activePage) return;
    previousPage.current = activePage;
    if (focusTitleOnChange.current) {
      focusTitleOnChange.current = false;
      // A mouse click announces through the focused title; drop any
      // keyboard-path message so the status node never names a stale page.
      setLiveMessage("");
      titleRef.current?.focus();
    } else {
      setLiveMessage(`${settingsPageById(activePage).label} page open`);
    }
  }, [activePage]);

  function openPage(id: SettingsPageId, moveFocus: boolean) {
    focusTitleOnChange.current = moveFocus;
    setActivePage(id);
  }

  function handleMenuKeyDown(event: KeyboardEvent<HTMLButtonElement>, id: SettingsPageId) {
    if (event.key === "Enter" || event.key === " ") {
      // preventDefault also suppresses the native click activation, so the
      // row keeps focus and the arrows below stay live.
      event.preventDefault();
      openPage(id, false);
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

  function renderPanel() {
    switch (activePage) {
      case "appearance":
        return <AppearanceSection />;
      case "layout":
        return <CloseBehaviorSetting />;
      case "editing":
        return <SendBehaviorSetting />;
      case "notifications":
        return <NotificationsSection />;
      case "diagnostics":
        return (
          <>
            <DiagnosticsPanel />
            <JournalRetentionPanel />
          </>
        );
      case "providers":
        return <ProvidersPanel />;
      case "profiles":
        return <AgentProfilesPanel />;
      case "usage":
        return <UsagePanel />;
      case "projects":
        return <ProjectsPanel />;
      case "oracle":
        return <OraclePanel />;
      case "paired":
        return <DevicesPanel />;
      case "about":
        return <AboutPanel />;
      default:
        return null;
    }
  }

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
              <div className="settings-host-row">
                <SettingsMenuIcon id="host" />
                <span>This PC</span>
                <HostDot />
              </div>
            ) : null}
            {group.pages.map((page) => (
              <button
                key={page.id}
                type="button"
                className={`settings-menu-row${activePage === page.id ? " settings-menu-row-active" : ""}`}
                data-settings-page={page.id}
                aria-current={activePage === page.id ? "page" : undefined}
                onClick={(event) => openPage(page.id, event.detail !== 0)}
                onKeyDown={(event) => handleMenuKeyDown(event, page.id)}
              >
                <SettingsMenuIcon id={page.id} />
                {page.label}
              </button>
            ))}
          </Fragment>
        ))}
      </nav>
      <span className="sr-only settings-live" role="status">
        {liveMessage}
      </span>

      <div className="settings-main">
        <div className="settings-main-inner" data-settings-content>
          <SettingsPageHeader page={settingsPageById(activePage)} titleRef={titleRef} />
          {renderPanel()}
        </div>
      </div>
    </section>
  );
}

// The two symbols the panel tests import from this path.
export { ALWAYS_ON_REASON, toolPolicyFor } from "./providerStatus";
export { DelegationSetting } from "./panels/AgentsPanel";
