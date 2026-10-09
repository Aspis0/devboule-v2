import { useSyncExternalStore } from "react";
import { getSendBehavior, subscribeSendBehavior } from "../../../lib/sendBehavior";
import { shortcutSections } from "../../../lib/keymap";
import { SettingsAdvanced, SettingsRow, SettingsSection } from "../rows";

/**
 * Settings → Shortcuts: the app-level keys, grouped by where they work, read
 * from the keymap module so the page lists nothing the handlers do not match.
 * The two Enter rows follow the Editing page's Default send. Each row's
 * detail and each group's note sit under one collapsed Advanced disclosure.
 */
export function ShortcutsPanel() {
  const behavior = useSyncExternalStore(subscribeSendBehavior, getSendBehavior);
  const sections = shortcutSections(behavior);
  return (
    <div id="settings-panel-shortcuts">
      {sections.map((section) => (
        <SettingsSection key={section.label} label={section.label}>
          {section.rows.map((row) => (
            <SettingsRow key={row.keys} title={row.title} control={<span>{row.keys}</span>} />
          ))}
        </SettingsSection>
      ))}
      <SettingsAdvanced>
        {sections.map((section) => (
          <div key={section.label}>
            {section.note !== undefined ? <p>{section.note}</p> : null}
            {section.rows.map((row) =>
              row.detail !== undefined ? (
                <p key={row.keys}>
                  {row.title}: {row.detail}
                </p>
              ) : null,
            )}
          </div>
        ))}
      </SettingsAdvanced>
    </div>
  );
}
