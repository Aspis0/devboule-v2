---
slug: icons
description: Icons that clarify rather than decorate: earn their place, preserve an accessible name, use a coherent family, separate glyph size from hit area, and balance optically. Apply whenever the output introduces an icon, symbol, icon-only control, or icon-and-label pair.
title: Icons
requires: []
---

**Earn the space (CONVENTION).** Primer says, “Where possible use icons to supplement text, rather than replacing it.” ([Primer Octicons usage](https://primer.style/octicons/usage-guidelines/)) Keep one when it clarifies an action, status, or category. Remove it when it adds no recognition; decoration should not compete with the label.

**Name the meaning (STANDARD).** WCAG 2.2 SC 1.1.1 (A) requires an equivalent text alternative for non-text content ([WCAG 2.2 SC 1.1.1](https://www.w3.org/TR/WCAG22/#non-text-content)); SC 4.1.2 (A) requires user interface components to expose name, role, and value ([WCAG 2.2 SC 4.1.2](https://www.w3.org/TR/WCAG22/#name-role-value)). Hide a decorative icon. An icon-only control needs a programmatic name for its action — “Submit,” not “arrow” — and an icon beside visible text should not create a competing name.

**Keep the family coherent (CONVENTION).** Apple requires icons to use “a consistent size, level of detail, stroke thickness (or weight), and perspective.” ([Apple Icons HIG](https://developer.apple.com/design/human-interface-guidelines/icons)) Choose one family for a surface and keep fill/outline treatment, detail, and density consistent. Stroke width is a family token, not a universal number; use its specification.

**Size by system, not folklore (CONVENTION; WCAG floor STANDARD).** Material specifies, “Product icons are 48dp; system icons are 24dp,” while Primer offers `12px`, `16px`, and `24px`. ([Material iconography](https://m1.material.io/style/icons.html), [Primer Octicons usage](https://primer.style/octicons/usage-guidelines/)) These are system conventions, not universal web sizes. Keep the glyph canonical and give the control a separate hit area; WCAG 2.2 SC 2.5.8 (AA) sets a 24×24 CSS px pointer-target floor, with exceptions.

**Balance optically (CONVENTION).** Apple notes, “Some icons — especially asymmetric ones — can look unbalanced when you center them geometrically instead of optically.” ([Apple Icons HIG](https://developer.apple.com/design/human-interface-guidelines/icons)) Beside text, align to the baseline, then adjust padding or the glyph when its mass leans. Do not encode a universal offset; inspect the icon beside its actual label and adjust only that component.

**What is contested.** 24px, 24dp, 16px, and 48dp fit different roles. An icon-only control needs an accessible name; no cross-family stroke number is sourced.
