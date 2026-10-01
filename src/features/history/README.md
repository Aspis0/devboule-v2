# History surface

History reads `journal_usage` and joins metadata from `sessions_list`. Both
are read when the panel opens, and an open panel is a snapshot: an agent
started or ended while History stays open appears or changes only after a
successful delete or a failed resume re-reads the roster, or when History is
reopened.

`Reopen` is offered when the daemon says the row can be reopened, and only
then: `isResumableSession` tests `Session.resumable` and nothing else. The
persisted columns and the family's own capability are the daemon's to judge
(`Provider::resumable()`, `session_resumable()`), so this surface never
re-derives the answer from kind, state or the presence of a provider id — a
guess here would offer a button that cannot work. A successful resume hands the
session to the workspace's normal attach flow.

Deletion is an explicit, user-confirmed `session_delete`, and a running row
(live or silent) cannot be deleted at all: the Delete button is marked
`aria-disabled` while the session runs and refuses the click, and the reason
("Archive the session before deleting it from history.") is its tooltip and
its accessible description. The button stays focusable so that description is
announced. Deleting a running session would have to kill it first, and killing
is a different act with a different confirmation.
