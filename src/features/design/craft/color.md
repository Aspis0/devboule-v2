---
slug: color
title: Color
requires: []
---

The palette belongs to the project. These rules hold whatever it is — an all-black site and
an all-green app both obey them.

**Proportion.** Neutrals carry 70–90% of the surface, the accent 5–10%, semantic colors
less. One accent. A second "hero color" is nearly always a decision that was never taken.

**Ration the accent.** At most two visible uses per screen — typically one chip or eyebrow
and one primary action. Links, focus rings and hover states all spend from the same budget.

**Contrast is a gate, not a goal.** 4.5:1 for body text, 3:1 for large text and for the
edges of controls against what surrounds them. Secondary text that fails is the most common
defect and the hardest to notice on a good monitor. If the accent is too light to carry
text, darken it for text and keep the bright variant for fills.

**Neither end of the range.** Pure black on pure white vibrates, and so does the inverse.
Near-black and near-white read as deliberate.

**A dark theme is a second palette, not an inversion.** It needs its own neutrals. Depth
comes from slightly lighter surfaces and hairline borders at low contrast, not from the
shadows that work on light.

**Name by role, never by hue.** `--surface`, `--accent`, `--danger` survive a palette
change; `--blue-500` locks it in.

**Define every token you reference.** A fragment is one `:root` in one `<style>` with no
cascade behind it: `var(--x)` with no `--x` resolves to nothing, the whole declaration is
dropped, and it fails silently and invisibly. Give a fallback — `var(--x, 4px)` — or define
it in the same block.

**States shift, they do not repaint.** Hover and active are the same color, moved. A new
hue on hover reads as a different component.

**Gradients earn their place or leave.** They may separate hierarchies. They must never be
the only thing making an element visible, and text over one has to clear contrast at both
ends.
