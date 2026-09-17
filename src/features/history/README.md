# History surface

History reads `journal_usage` and joins metadata from `sessions_list`.

`Reopen` is offered when the daemon says the row can be reopened, and only
then: `isResumableSession` tests `Session.resumable` and nothing else. The
persisted columns and the family's own capability are the daemon's to judge
(`Provider::resumable()`, `session_resumable()`), so this surface never
re-derives the answer from kind, state or the presence of a provider id — a
guess here would offer a button that cannot work. A successful resume hands the
session to the workspace's normal attach flow.

Deletion is an explicit, user-confirmed `session_delete`, and a **live** row
cannot be deleted at all: the button is disabled and says why, "Archive the
session before deleting it from history." Deleting a running session would
have to kill it first, and killing is a different act with a different
confirmation.
