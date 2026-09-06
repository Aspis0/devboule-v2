---
slug: typography
description: The system underneath the sizes: one capped scale, three weights, tracking that varies with size instead of one global value, line length, and the spacing and resize thresholds a layout has to survive rather than set. Apply whenever the output sets type.
title: Typography
requires: []
---

The tell is not an ugly typeface. It is hierarchy assembled from decoration instead of from a
system: every heading at a display size, one tracking value applied to everything, and a
layout that breaks the moment real copy arrives. The WCAG rules below are testable
requirements; the rest are conventions that hold across published systems.

**A scale, and a cap on it.** Derive sizes from one ratio — 1.2 and 1.25 are the common
choices — and hold a single artifact to six or eight of them. Published systems carry more
because they serve many screens; one page does not. A size that exists for one element is not
a scale, it is a decision taken twice.

**Three weights carry almost everything**: one to read, one to emphasise, one to announce.
400 and 600 are the usual ends. Size and weight make the hierarchy — reaching for colour to
carry it means the first two went unused.

**Uppercase needs air.** Caps at body tracking look cramped, and it is one of the most
reliable generated-text tells. Bringhurst gives 5-10% of the type size for strings of caps and
small caps, and says there is no generalised optimum for display capitals — a starting value
to judge by eye, not a constant. It is also why one global letter-spacing is wrong: body wants
0, caps positive, large display often slightly negative. Apple's system font changes tracking
at every point size rather than holding one.

**Line length.** WCAG 2.2 SC 1.4.8 (AAA) asks that a mechanism be available to give blocks of
no more than 80 characters, 40 for CJK; it does not require that authored text always render
that narrow. `max-width: 65ch` is a practical Latin heuristic — `ch` measures the width of
the zero glyph, not characters.

**Some numbers are what the layout must survive, not what to set.** Under WCAG 2.2 SC 1.4.12
(AA), when a reader overrides line height to 1.5x the font size, paragraph spacing to 2x,
letter spacing to 0.12x and word spacing to 0.16x, no content or function may be lost. Setting
0.12em of tracking on body text misreads it. What fails is a text container that cannot grow.
SC 1.4.4 (AA) is a separate test of the same shape: text at 200%.

**Two typefaces at most**, or one variable face at several weights. `font-family: system-ui`
alone on a heading is the default nobody chose. Prefer ragged-right body copy: justifying
without tested hyphenation opens rivers down the paragraph.
