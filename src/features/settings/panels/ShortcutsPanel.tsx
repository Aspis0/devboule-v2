import { useSyncExternalStore } from "react";
import { getSendBehavior, subscribeSendBehavior } from "../../../lib/sendBehavior";
import { shortcutSections } from "../../../lib/keymap";
import "../general.css";

/**
 * Settings → Shortcuts: the app-level keys, grouped by where they work, read
 * from the keymap module so the page lists nothing the handlers do not match.
 * The two Enter rows follow the Editing page's Default send.
 */
export function ShortcutsPanel() {
  const behavior = useSyncExternalStore(subscribeSendBehavior, getSendBehavior);
  return (
    <div id="settings-panel-shortcuts">
      {shortcutSections(behavior).map((section) => (
        <section className="machine-card" key={section.label} aria-label={section.label}>
          <h3 className="settings-subheading">{section.label}</h3>
          {section.note !== undefined ? <p className="machine-note">{section.note}</p> : null}
          {section.rows.map((row) => (
            <div className="machine-row" key={row.keys}>
              <span className="machine-row-copy">
                <span className="machine-row-title">{row.title}</span>
                {row.detail !== undefined ? (
                  <span className="machine-row-desc">{row.detail}</span>
                ) : null}
              </span>
              <span className="machine-row-title">{row.keys}</span>
            </div>
          ))}
        </section>
      ))}
    </div>
  );
}
