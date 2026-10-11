# Changelog

## [Unreleased]

### Upgrade and rollback notes

- **Journal schema v19** adds nullable `archived_at_ms` columns to the existing
  tables, inside the schema transaction. Existing rows stay unarchived, and the
  forward upgrade drops no data.
- **Rolling back is not safe without a copy.** A build older than v19 refuses a
  journal with a higher `user_version` (`FutureSchema`) and will not start on
  it. Before upgrading, stop the daemon and copy `journal.db` from the daemon's
  data directory. To roll back, restore that copy or restore the v19 build.
- **Protocol 36 and 37** add the archived list and restore, and the paired-host
  workspace operations (create, archive, remove a project). Clients and daemons
  negotiate the `workspace.archive` and `project.remove` names. A paired host
  that does not advertise them shows the workspace operations as unsupported;
  they are not reported as failures, and nothing is sent to it.
- **Forward-only protocol.** A client older than protocol 36 does not know the
  archived list, restore, or remote workspace frames, and will not show them.
