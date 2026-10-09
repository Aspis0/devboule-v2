/**
 * The profile form's tool-overlay control: the peer-restriction tick, and one
 * honest line for denials an older setting left behind. There is no per-tool
 * choice here — the owner ruled it out — so stored denials beyond the peer
 * tools are shown, never edited: the tick adds or removes only the peer pair
 * inside the stored list, and "Allow all tools" drops only the non-peer
 * entries. The draft always carries the whole list, so a save writes back
 * exactly what is stored and can never silently drop a restriction.
 */
import { PEER_TOOLS, overlayDenialsDescription } from "./profileOverlay";

export function AgentProfileOverlayEditor({
  overlay,
  busy,
  onChange,
}: {
  /** The profile's whole tool overlay, as currently drafted. */
  overlay: readonly string[];
  busy: boolean;
  /** Every change, as the next whole overlay. */
  onChange: (overlay: string[]) => void;
}) {
  const restricted = PEER_TOOLS.every((peer) => overlay.includes(peer));
  const extras = overlay.filter((name) => !PEER_TOOLS.includes(name));

  function togglePeers(on: boolean) {
    const next = on
      ? [...overlay, ...PEER_TOOLS.filter((peer) => !overlay.includes(peer))]
      : overlay.filter((name) => !PEER_TOOLS.includes(name));
    onChange(next);
  }

  function allowAll() {
    onChange(overlay.filter((name) => PEER_TOOLS.includes(name)));
  }

  return (
    <>
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          aria-label="Children cannot message peers or create further agents"
          checked={restricted}
          disabled={busy}
          onChange={(event) => togglePeers(event.target.checked)}
        />
        <span>
          <span>No peer contact and no further agents for children</span>
          <span className="agent-profile-tick-note">
            Children created from this profile cannot message other agents or create further agents.
          </span>
        </span>
      </label>
      {extras.length > 0 ? (
        <div className="agent-profile-legacy-denials">
          <span className="device-field-hint">
            {overlayDenialsDescription(extras) ?? "This profile blocks some tools."}
          </span>
          <button
            type="button"
            className="settings-device-action"
            disabled={busy}
            onClick={allowAll}
          >
            Allow all tools
          </button>
        </div>
      ) : null}
    </>
  );
}
