# Oracle pointers panel

The Oracle panel is embedded in Settings and is wired to the local engine.
Oracle returns ranked pointers to source files and line ranges, with the
matching snippet and optional symbol or match type. It is the only Settings tab
that is not fixture data.

The numeric RRF score is intentionally not rendered: it orders
the pointers but is not a confidence measure. Snippets have already passed
Oracle's secret-redaction boundary; the frontend must never be given or render
unredacted source text.
It does not generate or stream an answer: the person or agent reads the cited
spans directly.

The panel stages setup so one action is dominant at a time: choose a folder,
download the local models, index, then ask. Once Oracle is ready, the question
and results lead; typed index status, health checks, model state, file tabs,
and the declared resource cap remain available in the secondary administration
section. All values and actions come through the typed Oracle IPC wrappers.
Progress is derived from the indexed and total file counts because the current
contract does not expose a separate progress command.
