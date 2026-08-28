# Oracle pointers panel

M1b contains the Settings-embedded Oracle panel. Oracle returns ranked pointers
to source files and line ranges, with the matching snippet, score, and optional
symbol or match type. Snippets have already passed Oracle's secret-redaction
boundary; the frontend must never be given or render unredacted source text.
It does not generate or stream an answer: the person or agent reads the cited
spans directly.

The panel also keeps the typed index status, health checks, progress, file tabs,
and declared resource cap visible. Values currently come from `mockData.ts`;
replacing that boundary with the typed Oracle IPC is intentionally mechanical.
