---
slug: microcopy
description: Useful interface language: verb-led buttons, recoverable errors, purposeful empty states, and consistent casing without pretending one style is universal. Apply whenever the output labels an action, reports a problem, or explains an empty result.
title: Microcopy
requires: []
---

Generated UI reaches for vague actions, failure-only errors, and empty panels that say “No data.”

**Name the action (CONVENTION).** Carbon recommends a {verb} + {noun} button except for common actions like Done, Close, Cancel, Add, or Delete ([Carbon Button guidelines](https://preview.carbondesignsystem.com/building-blocks/core/components/button/guidelines)). Prefer “Start tracking” to “Get started” and “Save draft” to “Submit” when the object matters. A short label is fine when context supplies the noun.

**Make errors recoverable (CONVENTION).** GOV.UK says an error message should explain what went wrong and how to fix it ([GOV.UK Error message](https://design-system.service.gov.uk/components/error-message/)). Put validation beside its field, name the expected value or format, preserve the input, and give the correction. Replace “An error occurred” with a specific instruction whenever knowable; a field failure should not become a vague page alarm.

**Give empty states a job (CONVENTION).** Carbon says empty states should provide constructive next steps ([Carbon empty states](https://carbondesignsystem.com/patterns/empty-states-pattern/)). Name the situation, explain why it is empty when useful, and offer the action that fills it. For no results, repeat the query and offer a useful change of course.

**Choose a casing system (CONVENTION).** Carbon recommends sentence case for UI text ([Carbon writing style](https://carbondesignsystem.com/guidelines/content/writing-style/)); Apple says to choose a style per element type and use it consistently ([Apple Writing HIG](https://developer.apple.com/design/human-interface-guidelines/writing)). Default to sentence case: capitalize the first word and proper nouns. Title case can be correct when the product uses it consistently; do not mix styles by whim.

**Write for the next decision (OPINION).** Use concrete verbs, name objects, and cut filler. Keep obvious notification actions to one or two words; this is a Carbon convention, not a universal button limit. Translation can require longer copy. WCAG 2.2 requires labels or instructions when input needs them under SC 3.3.2 (A), and headings and labels that describe topic or purpose under SC 2.4.6 (AA) ([WCAG 3.3.2](https://www.w3.org/WAI/WCAG22/Understanding/labels-or-instructions), [WCAG 2.4.6](https://www.w3.org/WAI/WCAG22/Understanding/headings-and-labels)); it prescribes neither casing nor a word maximum.
