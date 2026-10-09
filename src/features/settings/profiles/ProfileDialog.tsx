/**
 * The scrim dialog around the profile form, for creating and editing alike.
 * A thin shell over the shared Settings dialog: the card, the focus trap
 * and the dirty check live there, the form owns every field. The discard
 * confirm is the dialog's own step — where the person pressed Cancel, × or
 * Escape — never a banner at the top of a scrolled form.
 */
import type { ReactNode } from "react";
import { SettingsDialog } from "../rows";

export function ProfileDialog({
  open,
  title,
  busy,
  onClose,
  children,
}: {
  /** The dialog is up; the parent owns the open state and says so. */
  open: boolean;
  title: string;
  /** True while the panel's write is in flight: every exit goes dead. */
  busy: boolean;
  onClose: () => void;
  children: (api: { requestClose: () => void; markDirty: () => void }) => ReactNode;
}) {
  return (
    <SettingsDialog
      open={open}
      title={title}
      busy={busy}
      onClose={onClose}
      closeLabel="Close profile dialog"
    >
      {children}
    </SettingsDialog>
  );
}
