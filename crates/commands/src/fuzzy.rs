//! Hand-rolled fuzzy subsequence scorer for the command palette.
//!
//! Case-insensitive: every query character must appear in the candidate in
//! order. Scoring rewards matches at the start of the candidate, at word
//! boundaries, and in contiguous runs; skipped characters between matches
//! cost a small linear gap penalty. The best-scoring alignment is found by
//! dynamic programming (candidates are short command labels, so the
//! quadratic table is a few hundred cells), and the winning positions come
//! back as char indices so frontends can highlight the matched characters.

/// Points per matched character.
const SCORE_MATCH: i32 = 4;
/// Extra points when the match begins at the very start of the candidate
/// (on top of the word-boundary bonus the first character also earns).
const BONUS_PREFIX: i32 = 8;
/// Points for a match at a word boundary (start of the candidate or after
/// a space/dash/underscore/slash/dot).
const BONUS_WORD_BOUNDARY: i32 = 6;
/// Points for a match immediately following the previous matched character.
const BONUS_CONSECUTIVE: i32 = 8;
/// Cost per candidate character skipped before or between matches.
const PENALTY_GAP: i32 = 1;

/// A successful fuzzy match: its score and the matched char indices into
/// the candidate, in ascending order (empty for an empty query).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    pub indices: Vec<usize>,
}

/// Match `query` as a case-insensitive subsequence of `candidate`.
///
/// An empty query matches everything with score 0 (the palette shows the
/// full list). Returns `None` when any query character cannot be placed.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    let needle: Vec<char> = query.chars().map(|c| c.to_ascii_lowercase()).collect();
    let hay: Vec<char> = candidate.chars().collect();
    if needle.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            indices: Vec::new(),
        });
    }
    if needle.len() > hay.len() {
        return None;
    }

    const UNREACHED: i32 = i32::MIN / 2;
    let (m, n) = (needle.len(), hay.len());
    // score[i][j]: best score with needle[i] matched exactly at hay[j].
    let mut score = vec![vec![UNREACHED; n]; m];
    let mut parent = vec![vec![usize::MAX; n]; m];

    for i in 0..m {
        // needle[i] cannot land before position i (its predecessors need room)
        // nor so late that the rest of the needle cannot fit.
        for j in i..(n - (m - 1 - i)) {
            if hay[j].to_ascii_lowercase() != needle[i] {
                continue;
            }
            let bonus = SCORE_MATCH + position_bonus(&hay, j);
            if i == 0 {
                score[0][j] = bonus - PENALTY_GAP * j as i32;
                continue;
            }
            let mut best = UNREACHED;
            let mut best_k = usize::MAX;
            for (k, &prev) in score[i - 1].iter().enumerate().take(j).skip(i - 1) {
                if prev == UNREACHED {
                    continue;
                }
                let step = if k + 1 == j {
                    BONUS_CONSECUTIVE
                } else {
                    -PENALTY_GAP * (j - k - 1) as i32
                };
                let total = prev + bonus + step;
                if total > best {
                    best = total;
                    best_k = k;
                }
            }
            score[i][j] = best;
            parent[i][j] = best_k;
        }
    }

    let (mut end, mut best) = (usize::MAX, UNREACHED);
    for (j, &total) in score[m - 1].iter().enumerate() {
        if total > best {
            best = total;
            end = j;
        }
    }
    if end == usize::MAX {
        return None;
    }

    let mut indices = vec![0usize; m];
    let mut j = end;
    for i in (0..m).rev() {
        indices[i] = j;
        j = parent[i][j];
    }
    Some(FuzzyMatch {
        score: best,
        indices,
    })
}

fn position_bonus(hay: &[char], j: usize) -> i32 {
    if j == 0 {
        return BONUS_WORD_BOUNDARY + BONUS_PREFIX;
    }
    match hay[j - 1] {
        ' ' | '-' | '_' | '/' | '.' => BONUS_WORD_BOUNDARY,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_of(query: &str, candidate: &str) -> i32 {
        fuzzy_match(query, candidate)
            .unwrap_or_else(|| panic!("'{query}' should match '{candidate}'"))
            .score
    }

    #[test]
    fn empty_query_matches_everything_with_zero_score() {
        let hit = fuzzy_match("", "Split pane right").unwrap();
        assert_eq!(hit.score, 0);
        assert!(hit.indices.is_empty());
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(fuzzy_match("xyz", "Split pane right"), None);
        // In-order requirement: characters present but reversed do not match.
        assert_eq!(fuzzy_match("ps", "sp"), None);
        // A query longer than the candidate can never match.
        assert_eq!(fuzzy_match("split pane", "split"), None);
    }

    #[test]
    fn matching_is_case_insensitive_in_both_directions() {
        let lower = fuzzy_match("split", "Split pane right").unwrap();
        let upper = fuzzy_match("SPLIT", "Split pane right").unwrap();
        assert_eq!(lower.indices, vec![0, 1, 2, 3, 4]);
        assert_eq!(lower, upper);
    }

    #[test]
    fn prefix_match_outranks_a_later_match() {
        assert!(score_of("re", "Reload config") > score_of("re", "Enter copy mode"));
    }

    #[test]
    fn word_boundary_match_outranks_a_mid_word_match() {
        // 'c' at the start of "copy" (word boundary) vs inside "Stack".
        assert!(score_of("c", "Enter copy mode") > score_of("c", "Stack panes"));
    }

    #[test]
    fn contiguous_run_outranks_scattered_letters() {
        assert!(score_of("spl", "Split pane") > score_of("spl", "Stop panel"));
    }

    #[test]
    fn best_alignment_wins_and_reports_its_indices() {
        // Greedy-first would take the 'p' inside "Split" and then hunt for a
        // distant 'a'; the DP lands the pair on the word "pane".
        let hit = fuzzy_match("pa", "Split pane").unwrap();
        assert_eq!(hit.indices, vec![6, 7]);
    }

    #[test]
    fn shorter_leading_gap_wins_between_equal_boundary_matches() {
        assert!(score_of("pane", "Zoom pane") > score_of("pane", "Split pane down"));
    }
}
