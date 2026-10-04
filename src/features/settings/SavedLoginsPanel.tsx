import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import {
  savedLoginCreate,
  savedLoginDelete,
  savedLoginsList,
  savedLoginUpdate,
} from "../../lib/tauri";
import { useTrackedRequest } from "../../lib/trackedRequest";
import type { SavedLogin } from "../../types/ipc";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import "./savedLogins.css";

/**
 * Saved logins: what this machine may fill in for an agent, and nothing else.
 *
 * A row is the entry's metadata and the words "Password saved". The password
 * itself is not held here: it is read out of the field, handed to the command
 * that writes it to the OS credential store, and the field is emptied on the
 * way out — whatever the answer. What the person typed stays only as long as
 * the call that carries it.
 *
 * The form is keyed by what it is editing (`SavedLoginsPanel.formKey`), because
 * it holds its values in the DOM rather than in state: a form that stayed
 * mounted across two rows would send the first row's values under the second
 * row's id.
 */

interface Draft {
  label: string;
  origins: string;
  username: string;
}

const EMPTY_DRAFT: Draft = { label: "", origins: "", username: "" };

/** Which entry the open form belongs to, or `new` for one being written. */
type Target = SavedLogin | "new";

function draftOf(login: SavedLogin): Draft {
  return {
    label: login.label,
    // Sites are listed one per line in the field and shown back the same way,
    // so the person editing sees exactly what was saved.
    origins: login.origins.join("\n"),
    username: login.username,
  };
}

/**
 * The vault writes its refusals as sentences for the person who reads them, so
 * they are shown as they arrive rather than through the daemon's own table,
 * which would answer "Something went wrong inside the agent daemon".
 */
function refusal(cause: unknown): ErrorSentence {
  const mapped = errorSentence(cause);
  return mapped.detail === null ? mapped : { sentence: mapped.detail, detail: null };
}

/** The key a form for `target` is mounted under. */
function formKey(target: Target): string {
  return target === "new" ? "new" : target.id;
}

export function SavedLoginsPanel() {
  const listed = useTrackedRequest<SavedLogin[]>(savedLoginsList, { status: "loading" }, true);
  const [editing, setEditing] = useState<Target | null>(null);
  const [armed, setArmed] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<ErrorSentence | null>(null);
  const [done, setDone] = useState("");

  // A command that answers after this panel is gone must not write to it, and
  // must not keep the payload it carried alive.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const logins = listed.state.status === "ready" ? listed.state.value : null;
  const listError: ErrorSentence | null =
    listed.state.status === "error"
      ? { sentence: listed.state.detail ?? listed.state.message, detail: null }
      : null;
  const error = actionError ?? listError;

  function begin(target: Target) {
    setArmed(null);
    setActionError(null);
    setEditing(target);
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy || editing === null) return;
    const form = event.currentTarget;
    const fields = Object.fromEntries(new FormData(form));
    const origins = String(fields.origins ?? "")
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "");
    const label = String(fields.label ?? "");
    const username = String(fields.username ?? "");
    const password = String(fields.password ?? "");
    // The field is emptied here, before the call that carries it: from here on
    // the password exists only in the argument of one command.
    const control = form.elements.namedItem("password");
    if (control instanceof HTMLInputElement) control.value = "";
    setBusy(true);
    setActionError(null);
    setDone("");
    try {
      if (editing === "new") {
        await savedLoginCreate({ label, username, origins, password });
      } else {
        await savedLoginUpdate({
          id: editing.id,
          label,
          username,
          origins,
          // Absent, not empty: an empty field means keep the stored password,
          // and an empty password is not one the vault will take.
          ...(password === "" ? {} : { password }),
        });
      }
      if (!mounted.current) return;
      setEditing(null);
      setDone(editing === "new" ? "Login saved." : "Login changed.");
      listed.run(false);
    } catch (cause: unknown) {
      if (!mounted.current) return;
      setActionError(refusal(cause));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }

  async function remove(login: SavedLogin) {
    if (busy) return;
    setBusy(true);
    setActionError(null);
    setDone("");
    try {
      await savedLoginDelete(login.id);
      if (!mounted.current) return;
      setArmed(null);
      // The row this form was editing is gone; a form that still named it
      // would offer to save a change to an entry that no longer exists.
      setEditing((open) => (open !== null && open !== "new" && open.id === login.id ? null : open));
      setDone(`Deleted ${login.label}.`);
      listed.run(false);
    } catch (cause: unknown) {
      if (!mounted.current) return;
      setActionError(refusal(cause));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }

  return (
    <div id="settings-panel-saved-logins">
      {error === null ? null : (
        <p role="alert" className="saved-logins-error">
          <ErrorText sentence={error.sentence} detail={error.detail} id="saved-logins-error" />
        </p>
      )}
      <p className="saved-logins-notice" role="status">
        {done}
      </p>

      {listError === null && logins === null ? <div role="status">Loading…</div> : null}

      {editing === null ? null : (
        <LoginForm
          key={formKey(editing)}
          draft={editing === "new" ? EMPTY_DRAFT : draftOf(editing)}
          editing={editing !== "new"}
          busy={busy}
          onSubmit={(event) => void submit(event)}
          onCancel={() => setEditing(null)}
        />
      )}

      {logins !== null && logins.length === 0 && editing === null ? (
        <p className="saved-logins-empty">No saved logins yet.</p>
      ) : null}

      {logins?.map((login) => (
        <LoginRow
          key={login.id}
          login={login}
          busy={busy}
          armed={armed === login.id}
          onEdit={() => begin(login)}
          onArm={() => setArmed(armed === login.id ? null : login.id)}
          onDisarm={() => setArmed(null)}
          onConfirm={() => void remove(login)}
        />
      ))}

      {editing === null ? (
        <button
          type="button"
          className="settings-device-action"
          disabled={busy}
          onClick={() => begin("new")}
        >
          Add a login
        </button>
      ) : null}
    </div>
  );
}

function LoginRow({
  login,
  busy,
  armed,
  onEdit,
  onArm,
  onDisarm,
  onConfirm,
}: {
  login: SavedLogin;
  busy: boolean;
  armed: boolean;
  onEdit: () => void;
  onArm: () => void;
  onDisarm: () => void;
  onConfirm: () => void;
}) {
  const confirmRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (armed) confirmRef.current?.focus();
  }, [armed]);
  return (
    <section className="saved-login-row">
      <div className="saved-login-head">
        <span className="saved-login-label">{login.label}</span>
        <span className="saved-login-meta">Password saved</span>
      </div>
      <span className="saved-login-meta">{login.origins.join(", ")}</span>
      {login.username === "" ? null : <span className="saved-login-meta">{login.username}</span>}
      <div className="saved-login-actions">
        <button type="button" className="settings-device-action" disabled={busy} onClick={onEdit}>
          Edit
        </button>
        <button type="button" className="settings-device-action" disabled={busy} onClick={onArm}>
          Delete
        </button>
      </div>
      {armed ? (
        <div className="saved-login-confirm" role="alert" tabIndex={-1} ref={confirmRef}>
          <span>
            Deleting removes the password from this machine&apos;s credential store. It cannot be
            undone.
          </span>
          <div className="saved-login-actions">
            <button
              type="button"
              className="settings-device-action"
              disabled={busy}
              onClick={onConfirm}
            >
              Delete now
            </button>
            <button type="button" className="settings-device-action" onClick={onDisarm}>
              Cancel
            </button>
          </div>
        </div>
      ) : null}
    </section>
  );
}

function LoginForm({
  draft,
  editing,
  busy,
  onSubmit,
  onCancel,
}: {
  draft: Draft;
  editing: boolean;
  busy: boolean;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onCancel: () => void;
}) {
  const firstField = useRef<HTMLInputElement>(null);
  useEffect(() => {
    firstField.current?.focus();
  }, []);
  return (
    <form className="saved-login-form" onSubmit={onSubmit}>
      <label className="saved-login-field">
        <span>Name</span>
        <input
          ref={firstField}
          name="label"
          type="text"
          defaultValue={draft.label}
          autoComplete="off"
          required
        />
      </label>
      <label className="saved-login-field">
        <span>Sites</span>
        <textarea
          name="origins"
          rows={2}
          defaultValue={draft.origins}
          placeholder="https://example.com"
          spellCheck={false}
          required
        />
        <span className="saved-login-hint">
          One exact site per line, written as https://example.com. A password is only ever used on
          the site it was saved for.
        </span>
      </label>
      <label className="saved-login-field">
        <span>Username</span>
        <input name="username" type="text" defaultValue={draft.username} autoComplete="off" />
      </label>
      <label className="saved-login-field">
        <span>Password</span>
        {/* A browser must not fill this in: a password the app does not know
            about is one it cannot offer, so it has to be typed here. */}
        <input name="password" type="password" autoComplete="new-password" />
        {editing ? (
          <span className="saved-login-hint">Leave empty to keep the saved password.</span>
        ) : null}
      </label>
      <div className="saved-login-actions">
        <button type="submit" className="settings-device-action" disabled={busy}>
          {busy ? "Saving…" : "Save"}
        </button>
        <button type="button" className="settings-device-action" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
