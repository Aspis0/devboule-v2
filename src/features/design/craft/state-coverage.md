---
slug: state-coverage
description: The four states a generated interface usually omits — loading, empty, error and edge — and what each has to contain. Apply whenever the output shows a list, a table, a form, a card, or anything that can be slow, empty, or wrong.
title: State coverage
requires: []
---

The most reliable failure in a generated interface is drawing only the state where everything
worked. Anything that fetches, transforms or accepts data has five states, and four of them
are usually missing.

**Loading.** A skeleton shaped like the content that is coming, not a spinner centred in a
blank panel. Matching the shape is what stops the page jumping when the data lands.

**Empty.** Not a blank, and not the words "No data". It is a composition with a job: a
headline, one plain sentence about why there is nothing, and the action that would fill it.
First use is an onboarding moment. A search that found nothing should repeat what was
searched for and offer somewhere to go next.

**Error.** Three answers, in this order: what happened, why if that is knowable, and what the
person can do now. "Something went wrong" gives none of them. Never fold an error into the
empty state — an error carries recovery information that empty does not. And whatever was
typed survives the failure; a form that clears itself makes the user pay twice for one
mistake. Match the severity to the scope, too: a field that failed validation marks the
field, not the page.

**Populated.** The one the design was drawn for.

**Edge.** The same layout holding a 200-character title, a missing image, an absent optional
field, a number four digits longer than expected, ten times the rows. Draw it with plausible
worst-case content rather than the tidy content that flatters it.

Two rules hold across all five. **State is never carried by colour alone** — a red border
with no icon and no words is invisible to a large number of people, and to everyone outdoors.
And an announcement needs the right role: `role="alert"` for something that interrupts,
`role="status"` for something that can wait. The container has to be in the document before
the message arrives, because adding both together announces nothing.
