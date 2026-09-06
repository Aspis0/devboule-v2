---
slug: accessibility
description: The floor, with its clause numbers and conformance levels: visible focus, the four different target sizes people collapse into one, accessible names for icon-only controls, text alternatives that carry purpose, and contrast. Apply whenever the output has a control, an image, or text.
title: Accessibility
requires: []
---

The generated defaults are specific and repeatable: the focus outline removed and not
replaced, icon-only buttons with no name, hit areas smaller than a fingertip, and alternative
text that describes the picture instead of its job.

**Never remove focus without replacing it.** WCAG 2.2 SC 2.4.7 (AA) requires a keyboard
operable interface to have a mode where the focus indicator is visible; `outline: none` with
nothing in its place fails it outright. `:focus-visible` styles the indicator when the browser
judges one is warranted, which is usually keyboard navigation rather than a pointer click.

**The indicator has a size.** SC 2.4.13 (AAA) asks for at least the area of a 2 CSS px
perimeter of the unfocused component, and a 3:1 change between the focused and unfocused
states. A ring is one shape that passes, not the required one.

**"44 by 44" is four different rules.** SC 2.5.8 (AA) sets 24 by 24 CSS px for pointer
targets, with exceptions — spacing, an equivalent control elsewhere, inline targets,
unmodified user-agent controls, essential presentation. SC 2.5.5 (AAA) sets 44 by 44 CSS px.
Apple recommends a hit region of 44 by 44 pt, 60 in visionOS; Material recommends 48 by 48 dp.
Points and dp are not CSS pixels, and the hit area may be larger than the glyph inside it.

**Every control needs an accessible name.** An icon-only button without one is nameless, and
`aria-label` is only one way to supply it — visible text, visually hidden text and
`aria-labelledby` all count. SC 1.1.1 (A) asks for a text alternative serving an equivalent
purpose: the purpose, not the picture, so an arrow that submits is "Submit". Decorative
imagery has to be ignorable by assistive technology, which for an `<img>` means `alt=""`, and
an icon carrying the only indication of an action is not decorative.

**Contrast.** SC 1.4.3 (AA) is 4.5:1 for text and 3:1 for large-scale text, which means 18pt
or 14pt bold. SC 1.4.11 (AA) is 3:1 for the visual information that identifies a control and
its states. SC 1.4.1 (A): colour must not be the only means of conveying information,
indicating an action, prompting a response or distinguishing an element. Both contrast
criteria exempt inactive components — which removes a measurement, not the need for a disabled
control to look unavailable.
