import type { MouseEvent } from "react";
import type { AgentProfile } from "../../../types/ipc";
import { overlayDenialsDescription } from "../profileOverlay";
import { profileMetaText, profileTileText } from "./profileText";

function PenMark() {
  return (
    <svg className="profile-spawn-pen" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5 3v4M3 5h4M6 17v4M4 19h4M13 3l4 4-9 9H4v-4Z" />
    </svg>
  );
}

function PencilIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6" />
    </svg>
  );
}

/**
 * One agent profile's row: the glyph tile, name, meta, spawn prompt, and the
 * icon actions. The editor lives in the dialog now, so the row holds no form
 * state — only the delete arm, which reuses the app's `device-inline-confirm`
 * pattern (the same one the Devices panel's revoke uses).
 */
export function ProfileRow({
  profile,
  isFirst,
  isLast,
  busy,
  loading,
  dialogHoldsTick,
  deleteArmed,
  onMove,
  onToggle,
  onEdit,
  onDeleteArm,
  onDeleteCancel,
  onDeleteConfirm,
}: {
  profile: AgentProfile;
  isFirst: boolean;
  isLast: boolean;
  busy: boolean;
  loading: boolean;
  /**
   * This row's own dialog is open: the row's tick waits, the dialog holds
   * it. Any other row's tick stays live — the base rule, kept exactly.
   */
  dialogHoldsTick: boolean;
  deleteArmed: boolean;
  onMove: (id: string, delta: -1 | 1) => void;
  onToggle: (id: string, next: boolean) => void;
  onEdit: (id: string, opener: HTMLElement) => void;
  onDeleteArm: (id: string) => void;
  onDeleteCancel: () => void;
  onDeleteConfirm: (id: string) => void;
}) {
  const locked = busy || loading;
  return (
    <>
      <span className="profile-tile" aria-hidden="true">
        {profileTileText(profile)}
      </span>
      <div className="agent-profile-main">
        <span className="profile-name">{profile.name}</span>
        <span className="profile-meta">{profileMetaText(profile)}</span>
        {profile.spawnPrompt ? (
          <span className="profile-spawn">
            <PenMark />
            <span className="profile-spawn-text">{profile.spawnPrompt}</span>
          </span>
        ) : (
          <span className="profile-spawn profile-spawn-empty">
            No spawn prompt — agents created from this profile start with the task alone.
          </span>
        )}
        {overlayDenialsDescription(profile.toolOverlay) === null ? null : (
          <span className="agent-profile-note">
            {overlayDenialsDescription(profile.toolOverlay)}
          </span>
        )}
        {profile.note ? (
          <span className="agent-profile-note">
            <span className="agent-profile-note-label">When to use: </span>
            {profile.note}
          </span>
        ) : (
          <span className="agent-profile-note agent-profile-note-empty">
            No note — agents choosing between profiles will be choosing blind on this one.
          </span>
        )}
      </div>
      <label className="agent-profile-tick">
        <input
          type="checkbox"
          checked={profile.enabledForAgents}
          disabled={locked || dialogHoldsTick}
          onChange={(event) => onToggle(profile.id, event.target.checked)}
        />
        <span>
          <span>Agents may create this</span>
          <span className="agent-profile-tick-note">
            {dialogHoldsTick
              ? "The open dialog holds this setting; save or close it, then use this tick. If this profile answers its own permission cards, its children run unattended."
              : "Lets an agent start this kind of agent. If this profile answers its own permission cards, its children run unattended."}
          </span>
        </span>
      </label>
      <div className="profile-row-actions">
        <button
          type="button"
          className={`profile-icon-btn${isFirst ? " profile-is-dim" : ""}`}
          aria-label={`Move ${profile.name} up`}
          title={`Move ${profile.name} up`}
          disabled={locked || isFirst}
          onClick={() => onMove(profile.id, -1)}
        >
          ↑
        </button>
        <button
          type="button"
          className={`profile-icon-btn${isLast ? " profile-is-dim" : ""}`}
          aria-label={`Move ${profile.name} down`}
          title={`Move ${profile.name} down`}
          disabled={locked || isLast}
          onClick={() => onMove(profile.id, 1)}
        >
          ↓
        </button>
        <button
          type="button"
          className="profile-icon-btn"
          aria-label={`Edit ${profile.name}`}
          title={`Edit ${profile.name}`}
          disabled={locked}
          onClick={(event: MouseEvent<HTMLButtonElement>) =>
            onEdit(profile.id, event.currentTarget)
          }
        >
          <PencilIcon />
        </button>
        <button
          type="button"
          className="profile-icon-btn profile-icon-btn-trash"
          aria-label={`Delete ${profile.name}`}
          title={`Delete ${profile.name}`}
          disabled={locked}
          onClick={() => onDeleteArm(profile.id)}
        >
          <TrashIcon />
        </button>
      </div>
      {deleteArmed ? (
        <div className="device-inline-confirm">
          <p className="device-copy">
            Deletes this profile. Agents are no longer offered it, and a creation naming it is
            refused.
          </p>
          <div className="device-actions">
            <button
              type="button"
              className="settings-device-action"
              disabled={locked}
              onClick={() => onDeleteConfirm(profile.id)}
            >
              Delete now
            </button>
            <button type="button" className="settings-device-action" onClick={onDeleteCancel}>
              Cancel
            </button>
          </div>
        </div>
      ) : null}
    </>
  );
}
