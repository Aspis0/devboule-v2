import { useSyncExternalStore } from "react";
import { getSendBehavior, subscribeSendBehavior } from "../../../lib/sendBehavior";
import { shortcutSections } from "../../../lib/keymap";
import { SettingsRow, SettingsSection } from "../rows";

/**
 * Settings → Shortcuts: the app-level keys, grouped by where they work, read
 * from the keymap module so the page lists nothing the handlers do not match.
 * The two Enter rows follow the Editing page's Default send. A row's condition
 * is its one visible line; a group's note sits under its label.
 */
export function ShortcutsPanel() {
  const behavior = useSyncExternalStore(subscribeSendBehavior, getSendBehavior);
  const sections = shortcutSections(behavior);
  return (
    <div id="settings-panel-shortcuts">
      {sections.map((section) => (
        <SettingsSection key={section.label} label={section.label}>
          {section.note !== undefined ? <p className="settings-status">{section.note}</p> : null}
          {section.rows.map((row) => (
            <SettingsRow
              key={row.keys}
              title={row.title}
              description={row.detail}
              control={<span>{row.keys}</span>}
            />
          ))}
        </SettingsSection>
      ))}
    </div>
  );
}
