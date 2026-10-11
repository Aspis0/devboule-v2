import { commandKeycap } from "../../lib/keymap";
import { PanelIcon } from "./panel/PanelIcon";
import "./PanelToggle.css";

type PanelSide = "left" | "right";

const SIDE_TEXT: Record<
  PanelSide,
  { name: string; chord: string; icon: "panel-left" | "panel-right" }
> = {
  left: { name: "sidebar", chord: "B", icon: "panel-left" },
  right: { name: "panel", chord: "E", icon: "panel-right" },
};

/** The key tip: the label, then the command key and the letter. Shown on hover and focus by CSS. */
function KeyTip({ label, chord }: { label: string; chord: string }) {
  return (
    <span className="panel-key-tip" aria-hidden="true">
      <span className="panel-key-tip-label">{label}</span>
      <kbd className="panel-key-tip-key">{commandKeycap()}</kbd>
      <kbd className="panel-key-tip-key">{chord}</kbd>
    </span>
  );
}

/** The header button that collapses a side panel, with its key tip. */
export function PanelToggle({ side, onCollapse }: { side: PanelSide; onCollapse: () => void }) {
  const { name, chord, icon } = SIDE_TEXT[side];
  const label = `Collapse ${name}`;
  return (
    <button
      type="button"
      className="panel-toggle"
      aria-label={label}
      aria-expanded="true"
      onClick={onCollapse}
    >
      <PanelIcon name={icon} size={16} strokeWidth={1.6} />
      <KeyTip label={label} chord={chord} />
    </button>
  );
}

/** The collapsed side: a whole-height button that shows the panel again. */
export function PanelStub({
  side,
  onShow,
  autoFocus = false,
}: {
  side: PanelSide;
  onShow: () => void;
  /** The right panel's stub takes focus when it replaces the open panel. */
  autoFocus?: boolean;
}) {
  const { name, chord, icon } = SIDE_TEXT[side];
  const label = `Show ${name}`;
  return (
    <button
      type="button"
      className={`workspace-collapsed-panel panel-stub panel-stub-${side}`}
      aria-label={label}
      aria-expanded="false"
      autoFocus={autoFocus}
      onClick={onShow}
    >
      <PanelIcon name={icon} size={16} strokeWidth={1.6} />
      <span className="workspace-vertical-label">{side === "left" ? "workspaces" : "panel"}</span>
      <KeyTip label={label} chord={chord} />
    </button>
  );
}
