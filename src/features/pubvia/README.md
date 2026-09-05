# Pubvia

A tool for academic writing — document model, citation handling, DOI and Crossref lookups,
Word export. It has nothing to do with writing software, which is the point: Devboule is
meant to host genuinely different kinds of work in one shell, not variations on a model
picker.

**This directory holds no code, and that is not an oversight.** Pubvia will arrive as an
out-of-process plugin, the same way [Polis](../polis) does, so there is nothing to compile
in here. The surface is registered in `types/surface.ts` and drawn by the shared
`SurfacePlaceholder`; this file exists so that an empty folder in `src/features` has a
stated reason rather than looking like abandoned work.
