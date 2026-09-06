---
slug: rtl
description: RTL and bidi as layout semantics: set direction and language, use logical CSS, mirror directional relationships while preserving logos and real-world imagery, and isolate mixed-direction content. Apply whenever the output may render Arabic, Hebrew, or another right-to-left language.
title: RTL and bidi
requires: []
---

**Set direction and language (STANDARD).** Use `<html dir="rtl" lang="ar">`; use `dir="auto"` for unknown content. MDN recommends root `dir` ([MDN `dir`](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Global_attributes/dir)); `lang` informs localization.

**Use logical CSS (STANDARD).** Logical properties are direction-relative ([MDN logical properties](https://developer.mozilla.org/en-US/docs/Web/CSS/Guides/Logical_properties_and_values)). Physical `left`/`right` do not automatically break RTL; use them for genuinely physical relationships. Use `margin-inline`, `padding-inline`, and `text-align: start/end` for direction-relative ones.

**Mirror directional meaning (CONVENTION).** Mirror controls when meaning depends on reading order or progress: next/previous, sliders, progress. Preserve controls referring to a real-world direction. Apple supports RTL flipped icons ([Apple Icons HIG](https://developer.apple.com/design/human-interface-guidelines/icons)); its clause excludes checkbox-label relationships or all reading-order illustrations. Keep DOM order meaningful and verify keyboard focus follows the same task sequence in LTR and RTL. Do not flip logos/universal signs ([Apple Right to left HIG](https://developer.apple.com/design/human-interface-guidelines/right-to-left)); preserve photos, artwork, charts, clocks, real-world objects; decide illustrations by meaning.

**Isolate mixed content (STANDARD).** Use `<bdi>` for unknown/user-generated values and `dir="auto"` for uncertain inputs; it isolates text direction ([MDN `<bdi>`](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/bdi)). Set phone numbers and country codes to `dir="ltr"`; Apple says they are always left-to-right ([Apple RTL internationalization](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPInternational/SupportingRight-To-LeftLanguages/SupportingRight-To-LeftLanguages.html)). For email, URLs, IBAN, and card values, isolate and test in RTL context; do not force one direction universally.

**Arabic type (CONVENTION).** Use project’s Arabic type scale; verify shaping and readability at used sizes. Apple says Arabic or Hebrew beside uppercase Latin often reads better about two points larger ([Apple Right to left HIG](https://developer.apple.com/design/human-interface-guidelines/right-to-left)). Do not apply a global tracking value across joined Arabic letters by default; if spacing is adjusted, preserve joining and test diacritics.
