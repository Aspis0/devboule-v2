---
slug: spacing
description: Spacing as structure rather than leftover: one scale, taken from the project when it has one, gaps between groups larger than gaps inside them, space before borders and cards, and rhythm that changes with the viewport. Apply whenever the output arranges more than one element.
title: Spacing
requires: []
---

The failure is not ugly spacing, it is arbitrary spacing: twelve pixels here, fourteen there,
and a gap between two sections that looks the same as a gap inside one. Every value is
defensible on its own and together they say nothing.

**Use a scale and stay inside it**, keeping exceptions deliberate rather than accidental.
There is no universal base, and the published systems are not even the same kind of artifact:
Carbon's spacing tokens start 2/4/8/12/16 and it keeps separate component and layout scales,
Primer's padding utilities run 4/8/16/24/32, GOV.UK publishes a static scale of 5/10/15/20/25
beside a responsive one. What matters is that a scale exists and holds, not where it starts.
Values invented per element are the tell.

**If the project already defines spacing tokens, those are the scale.** Introducing a second
one is worse than having picked the wrong base to begin with.

**Space is the first tool for grouping, not the last.** Related things sit closer than
unrelated things, and the difference has to be obvious rather than technically present. No
authoritative source fixes the ratio — Carbon says only that the gap between groups should be
adjusted in relation to the gap between items — so the test is whether the grouping survives
when the words are ignored. Uniform spacing does not destroy hierarchy by itself, but it takes
space out of the tools that can carry it: if every gap is 16px, structure has to come from
somewhere else.

**Reach for space before chrome.** A border, a tinted panel or a card is a heavier move than
distance. Separation that space can carry should be carried by space, and the container saved
for when there is something to contain.

**Proximity is not enough when the actions differ.** Controls that do different things need
room enough that neither is pressed by mistake: WCAG 2.2 SC 2.5.8 (AA) governs pointer target
size on the web, and Material asks for at least 8dp between touch targets. Giving a
destructive action more room than the default step is judgement, not a requirement.

**Vertical rhythm is tokenised, not a single number.** Section separation runs larger than
local separation, and often changes with the viewport — GOV.UK publishes responsive and static
scales side by side. Identical padding at every breakpoint is not automatically wrong, but
when it makes a phone read as a stacked desktop, that is the smell.
