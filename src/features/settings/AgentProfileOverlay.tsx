/**
 * The profile form's tool-overlay editor: the peer-restriction tick, the
 * stored denials beyond the peer tools, and the add control. It edits one
 * list — the profile's whole overlay, carried verbatim — so an ordinary edit
 * can never drop a restriction that shares the list with something else. The
 * daemon serves a fixed set of tool names and refuses the save naming any
 * other; no daemon-published list of them exists for the app to offer, so
 * the add control is free text and says so.
 */
import { useState } from "react";
import { PEER_TOOLS } from "./profileOverlay";
import { rustTrim, utf8Bytes } from "./AgentProfileDraft";

const MAX_TOOL_OVERLAY_NAME_BYTES = 128;

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
  const [tool, setTool] = useState("");
  const [error, setError] = useState<string | null>(null);
  const restricted = PEER_TOOLS.every((peer) => overlay.includes(peer));
  const extras = overlay.filter((name) => !PEER_TOOLS.includes(name));

  function togglePeers(on: boolean) {
    const next = on
      ? [...overlay, ...PEER_TOOLS.filter((peer) => !overlay.includes(peer))]
      : overlay.filter((name) => !PEER_TOOLS.includes(name));
    setError(null);
    onChange(next);
  }

  function addDenial() {
    const name = rustTrim(tool);
    if (name === "" || overlay.includes(name) || PEER_TOOLS.includes(name)) {
      setTool("");
      return;
    }
    const bytes = utf8Bytes(name);
    if (bytes > MAX_TOOL_OVERLAY_NAME_BYTES) {
      setError(
        `A denied tool name is ${bytes} bytes, over the ${MAX_TOOL_OVERLAY_NAME_BYTES}-byte cap. Nothing was added.`,
      );
      return;
    }
    setError(null);
    onChange([...overlay, name]);
    setTool("");
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
            They keep the agent roster, their read-only view.
          </span>
        </span>
      </label>
      {extras.length > 0 ? (
        <div className="device-field">
          <span className="settings-subheading">Other stored tool denials</span>
          {extras.map((name) => (
            <div className="agent-profile-create-row" key={name}>
              <span className="device-copy">{name}</span>
              <button
                type="button"
                className="settings-device-action"
                disabled={busy}
                aria-label={`Remove denied tool ${name}`}
                onClick={() => onChange(overlay.filter((kept) => kept !== name))}
              >
                Remove denial
              </button>
            </div>
          ))}
        </div>
      ) : null}
      <div className="agent-profile-create-row">
        <label className="device-field">
          Deny a tool by name
          <input
            aria-label="New denied tool name"
            value={tool}
            disabled={busy}
            onChange={(event) => setTool(event.target.value)}
          />
          <span className="device-field-hint">
            The daemon serves a fixed set of tools and refuses the save naming any other; its
            refusal names the tool. No list of the served names is published here.
          </span>
          {error === null ? null : (
            <span className="device-field-hint" role="alert">
              {error}
            </span>
          )}
        </label>
        <button
          type="button"
          className="settings-device-action"
          disabled={busy || rustTrim(tool) === ""}
          onClick={addDenial}
        >
          Add denial
        </button>
      </div>
    </>
  );
}
