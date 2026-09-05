# History surface

History is a read-only log view for saved journal sessions. It does not attach,
resume, replay, or restore terminal output. The panel reads `journal_usage`,
joins metadata from `sessions_list`, and deletes only after an explicit
user-confirmed `session_delete` action.
