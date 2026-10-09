import type { MouseEvent } from "react";
import type { AgentProfile } from "../../../types/ipc";
import { profileMetaText, profileTileText } from "./profileText";

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
 * One agent profile's row: the glyph tile, the name, the provider · model ·
 * effort line, the one-line note, and the icon actions. Nothing else reads
 * here — the editor behind the pencil holds the rest. The delete arm reuses
 * the app's `device-inline-confirm` pattern, beside the trash that armed it.
 */
export function ProfileRow({
  profile,
  isFirst,
  isLast,
  busy,
  loading,
  deleteArmed,
  onMove,
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
  deleteArmed: boolean;
  onMove: (id: string, delta: -1 | 1) => void;
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
        {profile.note ? <span className="agent-profile-note">{profile.note}</span> : null}
      </div>
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
          <p className="device-copy">Deletes this profile.</p>
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
