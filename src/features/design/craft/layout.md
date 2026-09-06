---
slug: layout
description: Alignment as hierarchy: visible keylines, optical rather than box-perfect balance, grouping by proximity before enclosure, and grid breaks that have a reason. Apply whenever the output arranges content on a grid or aligns related interface elements.
title: Layout
requires: []
---

Generated layout tends to center everything and give every rectangle equal weight. It can be tidy while reading order, optical balance, and the primary action remain unclear.

**Align for hierarchy (CONVENTION).** Apple recommends aligning components to improve scanning and communicate organization and hierarchy ([Apple Layout HIG](https://developer.apple.com/design/human-interface-guidelines/layout)). Set keylines for headings, copy, controls, and content edges; share them across related items and use proximity before borders or backgrounds. Carbon calls repeated edges visible key lines ([Carbon 2x Grid](https://v10.carbondesignsystem.com/guidelines/2x-grid/overview/)).

**Correct optically (CONVENTION).** Apple notes that asymmetric icons can look unbalanced when centered geometrically instead of optically ([Apple Icons HIG](https://developer.apple.com/design/human-interface-guidelines/icons)). Treat the box midpoint as a starting point; balance a glyph, illustration, or wordmark by visual mass, and align an icon beside text to the baseline when that reads better. No universal optical offset is sourced.

**Break the grid on purpose (CONVENTION).** Carbon’s narrow-grid exception lets headings and copy outside containers align with the copy within ([Carbon 2x Grid implementation](https://v10.carbondesignsystem.com/guidelines/2x-grid/implementation/)). Use a hang, bleed, or offset only when it improves hierarchy, reading path, or alignment. If cards break differently, remove the drift; no universal inset exists.

**Test the relationship (CONVENTION; WCAG web checks STANDARD).** Apple advises previewing across devices, orientations, localizations, and text sizes ([Apple Layout HIG](https://developer.apple.com/design/human-interface-guidelines/layout)). For web output, verify text resize under SC 1.4.4 (AA) and reflow under SC 1.4.10 (AA) ([WCAG 1.4.4](https://www.w3.org/WAI/WCAG22/Understanding/resize-text), [WCAG 1.4.10](https://www.w3.org/WAI/WCAG22/Understanding/reflow)). Test long labels, translations, large text, and narrow widths. Keep a break only when it preserves the intended alignment and DOM reading and focus order at the supported widths. Centering is a focal choice (OPINION), not a substitute for choosing the leading edge.

**What is contested.** Alignment and optical correction lack a formula. Grid structure is a Carbon convention, not WCAG; carry over the purpose, not a fixed pixel hang.
