import { useEffect, useRef } from "react";
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
export function PanelStub({ side, onShow }: { side: PanelSide; onShow: () => void }) {
  const { name, chord, icon } = SIDE_TEXT[side];
  const label = `Show ${name}`;
  const buttonRef = useRef<HTMLButtonElement>(null);
  // A header toggle that collapsed the panel was focused, and unmounting it drops
  // focus to the body; a chord leaves focus where the person typed, and keeps it.
  useEffect(() => {
    const active = document.activeElement;
    if (active === null || active === document.body) buttonRef.current?.focus();
  }, []);
  return (
    <button
      ref={buttonRef}
      type="button"
      className={`workspace-collapsed-panel panel-stub panel-stub-${side}`}
      aria-label={label}
      aria-expanded="false"
      onClick={onShow}
    >
      <PanelIcon name={icon} size={16} strokeWidth={1.6} />
      <span className="workspace-vertical-label">{side === "left" ? "workspaces" : "panel"}</span>
      <KeyTip label={label} chord={chord} />
    </button>
  );
}
