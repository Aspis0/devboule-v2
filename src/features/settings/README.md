# Settings surface

Six tabs: General, Projects, Oracle, Providers & models, Devices, and Labs.

Only one of them is real. **Oracle** is wired to the local engine and lives in
[`../oracle`](../oracle); everything else reads from `mockData.ts`.

That boundary is deliberate and kept in one file rather than scattered through the
components, so what is fixture and what is not can be answered by looking at the imports
instead of by tracing a call. When the typed daemon IPC exists for a tab, the change is
visible as an import that stops pointing at `mockData`.

`JournalRetentionPanel` is the exception worth knowing about: it edits a real policy, not
a sample of one.
