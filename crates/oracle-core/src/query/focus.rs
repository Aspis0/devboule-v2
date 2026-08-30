//! Narrow a retrieved chunk's citation to the lines that answer the query.
//!
//! Retrieval returns a chunk: 22 lines on average for the code that the bench
//! actually retrieves, far more for prose. The citation the caller shows should
//! be smaller than that, because a reader handed a 22-line span to find the one
//! line that matters spends attention on text competing with the answer. This
//! module re-scores fixed-length line windows of an already-selected chunk with
//! the cross-encoder that the query path already loads, and reports the best
//! window as a *focus* alongside — never instead of — the full chunk.
//!
//! Measured on the 160-question bench: when the retrieved chunk contains a
//! ground-truth evidence line, the focus lands on it 85% of the time, while
//! citing 2.32x fewer lines. It is a good first place to look and a bad only
//! place to look, which is exactly why it is additive.
//!
//! Two constraints shape the geometry, both from measured behaviour of MS
//! MARCO cross-encoders rather than from taste:
//!
//! 1. **Every window has the same line count.** These models score longer
//!    passages higher, so comparing spans of different lengths would elect the
//!    longest one by construction and tell us nothing. Equal lengths make the
//!    bias a constant that cancels out of the comparison.
//! 2. **A window is never compared against the whole chunk.** Same reason: the
//!    chunk's own rerank score lives on a different length scale, so "the best
//!    window scores below the chunk" carries no information.
//!
//! The focus is advisory. The full chunk text and its line range stay in the
//! response, so a caller that disagrees with the narrowing keeps everything it
//! had before.

/// Windows scored per chunk, at 50% overlap.
///
/// Swept over 2, 3, 4, 6 and 8 on the 160-question bench, against the 150
/// retrieved target chunks that carry line-level evidence
/// (`recon/bench-citation-focus.md`). Two results decided it, both from a
/// paired bootstrap over those cases rather than from eyeballing the means:
///
/// - **Resolution does not decide whether the focus lands on evidence at all.**
///   No pair of geometries is distinguishable on hit rate; two windows of 16
///   lines and eight of 6 are equally likely to land. So the limit is the
///   cross-encoder's choice, not how finely the chunk is cut.
/// - **Resolution does decide how much evidence survives**, and there the steps
///   are real: 3 to 4 costs nothing measurable, 4 to 6 costs recall at 95%.
///
/// So the criterion is the narrowest geometry reachable without a measurable
/// recall loss from its predecessor, which is four: 10.05 cited lines instead
/// of 23.29. That criterion encodes a preference for a narrow citation. Two
/// windows really does retain more evidence, measurably, while citing 16 lines;
/// someone who would rather read six more lines than miss evidence should
/// change this constant, and the bench supports that reading of the same table.
///
/// Cost is not part of the decision: every geometry measured between 5.5 and
/// 8.5 ms per query, against the 158 ms the reranking pass already spends.
pub const FOCUS_WINDOWS_PER_CHUNK: usize = 4;

/// Below this the chunk is already a precise citation and narrowing it further
/// would only strip context from an answer that is small anyway.
pub const MIN_CHUNK_LINES_TO_NARROW: usize = 8;

/// A window shorter than this is too little text for a cross-encoder trained on
/// natural-language passages to score meaningfully.
pub const MIN_FOCUS_LINES: usize = 3;

/// Hard ceiling on windows scored for one query, across all results. Guards the
/// prose profile, whose chunks are 12,000 characters: without it a query over
/// documentation would submit hundreds of pairs. Sized so the shipped limit of
/// ten results is covered whole (ten chunks plan at most five windows each)
/// rather than silently losing the tail of the list.
pub const MAX_FOCUS_WINDOWS_PER_QUERY: usize = 64;

/// The result limit the Tauri layer ships (`QUERY_LIMIT`). Not imported —
/// `oracle-core` does not depend on its caller — but asserted against, so the
/// budget above stays unreachable in production rather than quietly trimming
/// the focus off the tail of a result list. A caller that raises either
/// constant should fail here, at build time, and not there, in silence.
const SHIPPED_RESULT_LIMIT: usize = 10;
const _: () = assert!(
    MAX_FOCUS_WINDOWS_PER_QUERY >= (FOCUS_WINDOWS_PER_CHUNK + 1) * SHIPPED_RESULT_LIMIT,
    "the shipped result limit no longer fits inside the focus window budget"
);

/// The selected sub-span of a chunk, in lines relative to the chunk's own text.
///
/// Offsets are relative on purpose. Code chunks carry their absolute first line
/// in the index; prose chunks carry only character offsets and the absolute
/// line is derived downstream. Keeping this relative means one representation
/// works for both, and the single place that already knows a chunk's line base
/// stays the single place that maps it.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FocusSpan {
    /// Zero-based index of the window's first line within the chunk text.
    pub line_offset: usize,
    /// Window length in lines. Constant across the windows of one chunk.
    pub line_count: usize,
    /// Raw cross-encoder score of the winning window. Comparable only against
    /// the other windows of the same chunk.
    pub score: f64,
}

/// Plan the equal-length line windows to score for a chunk of `total_lines`.
///
/// Returns an empty plan when the chunk is short enough that its own range is
/// already a precise citation.
pub fn plan_focus_windows(total_lines: usize) -> Vec<(usize, usize)> {
    plan_focus_windows_with(FOCUS_WINDOWS_PER_CHUNK, total_lines)
}

/// The plan above with the window count left open, so a benchmark can sweep the
/// resolution without the production path growing a knob nobody sets.
pub fn plan_focus_windows_with(
    windows_per_chunk: usize,
    total_lines: usize,
) -> Vec<(usize, usize)> {
    let windows_per_chunk = windows_per_chunk.max(1);
    if total_lines < MIN_CHUNK_LINES_TO_NARROW {
        return Vec::new();
    }
    // Windows overlap by half, so n windows of length 2s at stride s span
    // s * (n + 1) lines. Solve for the stride that covers the chunk exactly.
    let stride = total_lines.div_ceil(windows_per_chunk + 1).max(1);
    let window = (stride * 2).max(MIN_FOCUS_LINES).min(total_lines);
    if window >= total_lines {
        // One window would be the whole chunk: nothing to narrow.
        return Vec::new();
    }

    let mut offsets: Vec<usize> = Vec::with_capacity(windows_per_chunk + 1);
    let mut offset = 0usize;
    while offset + window <= total_lines {
        offsets.push(offset);
        offset += stride;
    }
    // Anchor a final window to the end so the chunk's tail is never unscored.
    let last_start = total_lines - window;
    if offsets.last().is_none_or(|&last| last < last_start) {
        offsets.push(last_start);
    }
    offsets.into_iter().map(|start| (start, window)).collect()
}

/// Materialize the planned windows as text, preserving line boundaries.
pub fn window_texts(text: &str, plan: &[(usize, usize)]) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    plan.iter()
        .map(|&(offset, count)| {
            let start = offset.min(lines.len());
            let end = (offset + count).min(lines.len());
            lines[start..end].join("\n")
        })
        .collect()
}

/// Pick the highest-scoring window. Ties resolve to the earliest window, which
/// keeps the choice deterministic and biases towards a definition over its
/// later uses when a chunk repeats a symbol.
pub fn select_focus(plan: &[(usize, usize)], scores: &[f64]) -> Option<FocusSpan> {
    plan.iter()
        .zip(scores)
        .enumerate()
        .max_by(|(left_index, (_, left)), (right_index, (_, right))| {
            left.partial_cmp(right)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(_, (&(line_offset, line_count), &score))| FocusSpan {
            line_offset,
            line_count,
            score,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every invariant the rest of this module depends on, over every geometry
    /// and every chunk height that can occur, rather than the handful a reviewer
    /// would think to try. 12 x 2001 plans is instant, and it is the difference
    /// between believing the arithmetic and knowing it.
    #[test]
    fn the_plan_holds_for_every_geometry_and_every_chunk_height() {
        for windows in 1..=12usize {
            for total in 0..=2000usize {
                let plan = plan_focus_windows_with(windows, total);
                if plan.is_empty() {
                    // Refusing to plan is only allowed when there is nothing to
                    // gain: too short to narrow, or one window would be the
                    // whole chunk.
                    let stride = total.div_ceil(windows.max(1) + 1).max(1);
                    let width = (stride * 2).max(MIN_FOCUS_LINES).min(total);
                    assert!(
                        total < MIN_CHUNK_LINES_TO_NARROW || width >= total,
                        "no plan for {total} lines at {windows} windows, but one was possible"
                    );
                    continue;
                }

                let width = plan[0].1;
                assert!(
                    plan.iter().all(|&(_, count)| count == width),
                    "unequal window lengths at {total}/{windows} would let length bias decide"
                );
                assert!(
                    width < total,
                    "{width} lines is not narrower than {total} at {windows} windows"
                );
                assert!(
                    width >= MIN_FOCUS_LINES,
                    "window of {width} lines is below the floor at {total}/{windows}"
                );
                assert!(
                    plan.len() <= windows + 1,
                    "{total}/{windows} planned {} windows",
                    plan.len()
                );
                assert_eq!(plan[0].0, 0, "first line unscored at {total}/{windows}");
                let (last_offset, last_count) = *plan.last().unwrap();
                assert_eq!(
                    last_offset + last_count,
                    total,
                    "last line unscored at {total}/{windows}"
                );
                for &(offset, count) in &plan {
                    assert!(
                        offset + count <= total,
                        "window {offset}+{count} runs past {total} at {windows} windows"
                    );
                }
                // Offsets are strictly increasing, so no window is scored twice
                // and the random-window control is an average over distinct
                // spans rather than a duplicate-weighted one.
                assert!(
                    plan.windows(2).all(|pair| pair[0].0 < pair[1].0),
                    "repeated or unordered offsets at {total}/{windows}: {plan:?}"
                );
                // Coverage: every line of the chunk falls in some window, so a
                // chunk cannot hide its answer between two of them.
                let mut covered = vec![false; total];
                for &(offset, count) in &plan {
                    for line in covered.iter_mut().skip(offset).take(count) {
                        *line = true;
                    }
                }
                assert!(
                    covered.iter().all(|line| *line),
                    "gap in coverage at {total}/{windows}: {plan:?}"
                );
            }
        }
    }

    #[test]
    fn short_chunks_are_left_alone() {
        for total in 0..MIN_CHUNK_LINES_TO_NARROW {
            assert!(
                plan_focus_windows(total).is_empty(),
                "chunk of {total} lines should not be narrowed"
            );
        }
    }

    #[test]
    fn window_texts_follow_line_boundaries() {
        let text = (1..=20)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let plan = plan_focus_windows(20);
        let texts = window_texts(&text, &plan);
        assert_eq!(texts.len(), plan.len());
        assert!(texts[0].starts_with("line 1"));
        assert!(texts.last().unwrap().ends_with("line 20"));
        for (window, &(_, count)) in texts.iter().zip(&plan) {
            assert_eq!(window.lines().count(), count);
        }
    }

    #[test]
    fn window_texts_tolerate_a_chunk_shorter_than_its_plan() {
        // Redaction rewrites tokens but not line counts; a caller passing a
        // mismatched text should still get bounded slices, never a panic.
        let texts = window_texts("only one line", &[(0, 4), (8, 4)]);
        assert_eq!(texts, vec!["only one line".to_string(), String::new()]);
    }

    #[test]
    fn the_best_window_wins_and_ties_go_to_the_earliest() {
        let plan = vec![(0, 4), (4, 4), (8, 4)];
        let focus = select_focus(&plan, &[0.1, 9.0, 0.2]).unwrap();
        assert_eq!((focus.line_offset, focus.line_count), (4, 4));
        assert_eq!(focus.score, 9.0);

        let tied = select_focus(&plan, &[5.0, 5.0, 5.0]).unwrap();
        assert_eq!(tied.line_offset, 0);
    }

    #[test]
    fn an_empty_plan_selects_nothing() {
        assert!(select_focus(&[], &[]).is_none());
    }
}
