# History surface

History reads `journal_usage`, joins metadata from `sessions_list`, and offers
`Reopen` for ACP rows with persisted provider and peer-session metadata. A
successful resume hands the session to the workspace's normal attach flow.
Deletion still requires an explicit user-confirmed `session_delete` action.
