//! The budget one tool row's text is held to.
//!
//! Whatever provider served the call, a row that carries a tool's answer is
//! cut at [`MAX_TEXT_BYTES`]: the encoded event then stays far under the 1 MiB
//! frame cap even when every byte JSON-escapes to six (64 KiB becomes at most
//! 384 KiB), and the journal keeps an answer a reader can still scroll. The cut
//! lands on a character boundary and the row says it was cut.

/// How much of a tool's answer one row keeps.
pub(crate) const MAX_TEXT_BYTES: usize = 64 * 1024;

/// What a row says in place of the text it does not keep.
pub(crate) const TRUNCATION_MARKER: &str = "\n[output truncated]";

/// The first `max` bytes of `text`, backed off to a character boundary.
pub(crate) fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `body` as a row shows it: cut at the budget, marked when the row shows less
/// than the source held. `dropped` says the caller already left text out while
/// it was assembling `body`, so `body` alone cannot say it.
pub(crate) fn shown(body: &str, dropped: bool) -> String {
    let kept = clip(body, MAX_TEXT_BYTES);
    let mut text = kept.to_string();
    if dropped || kept.len() < body.len() {
        text.push_str(TRUNCATION_MARKER);
    }
    text
}

/// A whole answer read at once, as a row shows it.
pub(crate) fn capped(text: &str) -> String {
    shown(text, false)
}

#[cfg(test)]
#[path = "text_cap_tests.rs"]
mod tests;
