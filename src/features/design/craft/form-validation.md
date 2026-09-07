---
slug: form-validation
description: Validation timing and field-error wiring: blur before error, input to clear, submit focus, live checks that do not freeze editing, and aria-describedby attachment. Apply whenever the output includes a form that can reject input.
title: Form validation
requires: []
---

Generated forms usually get one of two things wrong: they speak before a person has
finished, or they report an error without attaching it to the field that needs repair.
`state-coverage` owns which states exist; `microcopy` owns the words; `accessibility` owns
the conformance floor.

**Wait for the first edited blur (OPINION).** Do not show an error on focus or ordinary
typing. Validate after the first blur that follows editing. Once an error is visible,
revalidate on `input` so it clears as soon as the value is valid; do not make the person blur
again.

**Submit is the complete check (CONVENTION).** GOV.UK's error-summary pattern moves focus to a
heading-led summary whose links target the erroneous fields; the WAI forms tutorial recommends
the same top-of-form list and adds that focusing the first invalid input is convenient. WCAG
mandates neither — Understanding SC 3.3.1 (A) says so outright. Render the summary before
focusing it, and do not move focus on each keystroke. A debounced uniqueness
or address preflight may report a live result politely, but it must not freeze editing or disable
submit indefinitely. Start it only when the value is plausibly complete, and server validation
still belongs to submit. Apply a result only to the value that was checked, never to a newer
edit. If the preflight is unavailable, leave the field editable and let submit perform the
server check. Keep pending and failure distinct: a delayed result is not an error.

**Keep one error attached to one field (CONVENTION).** GOV.UK and the WAI forms tutorial both
wire field errors through a programmatic description: give each field a stable `id`, keep its
label associated, give its error a unique `id`, and reference that error with
`aria-describedby`. Set `aria-invalid="true"` only while that field has a current error —
technique ARIA21 for SC 3.3.1 (A); GOV.UK's examples omit it, so that rule rests on the
technique alone. A
summary that receives focus should not also be an assertive alert, or the same failure is
announced twice; inline errors that appear without focus movement use the existing
accessibility wiring.

**Map server failures back to fields (OPINION).** A rejected field must return to the same
field and error node rather than becoming a route-level exception. Keep the field's identity
stable across the response so focus, description and correction remain attached.
