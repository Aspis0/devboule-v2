---
slug: motion
description: Purposeful interface motion: scoped durations and easing, feedback and continuity over decoration, cancellation and reduced-motion behavior, and safe handling of automatic movement. Apply whenever the output animates a state, transition, feedback response, or moving piece of content.
title: Motion
requires: []
---

**Animate with a reason (CONVENTION).** Apple advises purposeful motion that supports the experience without overshadowing it ([Apple Motion HIG](https://developer.apple.com/design/human-interface-guidelines/motion)). Animate feedback, continuity, hierarchy, or abrupt change; let people act or cancel, and never make them wait on decoration.

**Use scoped durations (CONVENTION).** Material gives 300ms as typical for mobile, warns that over 400ms may feel slow, and gives 150–200ms for desktop ([Material duration and easing](https://m1.material.io/motion/duration-easing.html)).

**Match the curve to the change (CONVENTION).** Pick easing from the interaction: use opacity and colour for state changes; use a transform or spring only when physical continuity is part of the behaviour. Material’s standard easing is `cubic-bezier(0.4, 0, 0.2, 1)` ([Material duration and easing](https://m1.material.io/motion/duration-easing.html)). Avoid `transition: all`; prefer `transform` and `opacity` when they express the effect (CONVENTION, [MDN](https://developer.mozilla.org/en-US/docs/Web/Performance/Guides/Fundamentals)).

**Honor reduced motion (STANDARD).** WCAG 2.2 SC 2.3.3 (AAA) requires interaction-triggered motion to be disableable unless essential to the functionality or information ([WCAG 2.3.3](https://www.w3.org/WAI/WCAG22/Understanding/animation-from-interactions)). `prefers-reduced-motion` is a CSS media feature, not a WCAG criterion; use `@media (prefers-reduced-motion: reduce)` as the implementation hook ([Media Queries Level 5](https://drafts.csswg.org/mediaqueries-5/#prefers-reduced-motion)). In reduce, remove translate, scale, rotate, and parallax; retain opacity or colour for needed state cues. This is AAA, not AA.

**Control ambient motion (STANDARD).** WCAG 2.2 SC 2.2.2 (A) covers moving, blinking, or scrolling information that starts automatically, lasts over five seconds, and is presented in parallel with other content ([WCAG 2.2 SC 2.2.2](https://www.w3.org/WAI/WCAG22/Understanding/pause-stop-hide.html)). Provide pause, stop, or hide when it applies. It is not a five-second limit on transitions or interaction-triggered animation and does not cover an indeterminate spinner by itself. Auto-updating information is separate, with no five-second threshold.

**What is contested.** Material publishes timings and curves; Apple gives purpose without a universal duration.
