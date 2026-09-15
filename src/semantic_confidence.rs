//! Calibrated confidence floor and abstention for `eg query semantic`
//! (issue #263).
//!
//! ## Problem
//!
//! Semantic search ranks candidates by cosine similarity and returns the top
//! rows unconditionally. A best score of 0.41 looks exactly like a best score
//! of 0.92 on the wire: the caller cannot tell a confident, citation-backed
//! result from nearest-neighbor noise. For an agent deciding whether to act
//! on a result, that ambiguity is a correctness hazard.
//!
//! ## What this module provides
//!
//! * [`SEMANTIC_CONFIDENCE_FLOOR`] — the corpus-calibrated cosine-similarity
//!   floor. Documented, not magic: see the derivation note below and the full
//!   write-up in `docs/cli/query.md` ("Confidence calibration and
//!   abstention").
//! * [`ConfidenceBand`] — the per-row verdict (`strong` / `weak`) every
//!   semantic result row now carries.
//! * [`should_abstain`] — the answer-level predicate: when the best
//!   candidate scores below the floor, the query abstains instead of
//!   presenting a weak top hit as an authoritative answer.
//! * [`SemanticAbstention`] — the explicit abstention envelope printed in
//!   place of result rows. It is a deliberate, successful answer (exit 0),
//!   distinct from the exit-2 "no match" path used when the store has no
//!   semantic index at all or the scope matches zero candidates.
//!
//! ## Floor derivation (summary)
//!
//! The floor was measured, not chosen: `calibration/calibrate.py` built an
//! embedded store from this repository, ran the 32 queries in
//! `corpus/semantic_relevance_corpus.json` plus 15 held-out nonsense and
//! out-of-domain queries through `eg query semantic`, and recorded the
//! top-1 score distribution per query class. The floor sits in the gap
//! between the weakest reviewed in-corpus top-1 score and the strongest
//! negative/ambiguous top-1 score, so reviewed in-corpus queries keep their
//! answers while nonsense and out-of-domain queries abstain. The exact
//! numbers, the held-out abstention rate, and the recall check are documented
//! in `docs/cli/query.md`; `corpus/semantic_confidence_fixture.json` pins
//! the floor value for regression.
//!
//! ## Determinism
//!
//! Classification is a pure function of the score: no timestamps, no random
//! IDs, no HashMap iteration. Serialization field order is the struct field
//! order, so repeated queries produce byte-identical confidence fields.

use serde::Serialize;

/// Cosine-similarity floor below which the best semantic candidate is too
/// weak to present as a confident, citation-backed result (issue #263).
///
/// Calibrated from `corpus/semantic_relevance_corpus.json`; see the module
/// docs and `docs/cli/query.md` for the derivation. The value is a
/// documented constant — not a magic number — and
/// `corpus/semantic_confidence_fixture.json` pins it for regression.
pub const SEMANTIC_CONFIDENCE_FLOOR: f32 = 0.55; // PROVISIONAL: set from calibration before GREEN.

/// Stable tag naming how [`SEMANTIC_CONFIDENCE_FLOOR`] was set.
///
/// Mirrors the drift answer's `selection_basis`: every semantic row carries
/// the basis so a consumer knows what selected it.
pub const SEMANTIC_SELECTION_BASIS: &str = "corpus_calibrated_confidence_floor";

/// Confidence verdict for one returned semantic row (issue #263).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceBand {
    /// `score >= SEMANTIC_CONFIDENCE_FLOOR`: safe to cite as a confident lead.
    Strong,
    /// `score < SEMANTIC_CONFIDENCE_FLOOR`: nearest-neighbor noise risk; the
    /// agent must confirm before acting on this row.
    Weak,
}

impl ConfidenceBand {
    /// Stable lowercase tag used in serialized output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Weak => "weak",
        }
    }

    /// Classifies `score` against [`SEMANTIC_CONFIDENCE_FLOOR`].
    ///
    /// The boundary is inclusive: a score exactly at the floor is `Strong`.
    /// Fail-open the other way would silently weaken the documented floor.
    #[must_use]
    pub fn of(score: f32) -> Self {
        if score >= SEMANTIC_CONFIDENCE_FLOOR {
            Self::Strong
        } else {
            Self::Weak
        }
    }
}

/// Returns `true` when the best candidate's score is below the calibrated
/// floor — the abstention condition (issue #263).
#[must_use]
pub const fn should_abstain(best_score: f32) -> bool {
    best_score < SEMANTIC_CONFIDENCE_FLOOR
}

/// Answer-level abstention verdict for `eg query semantic` (issue #263).
///
/// Emitted on stdout as a single JSON object when [`should_abstain`] fires,
/// in place of result rows. `ok` is `true`: abstention is a completed,
/// deliberate answer (exit 0), not a failure — distinct from the exit-2
/// "no match" path used when the store has no semantic index or the scope
/// matches zero candidates.
#[derive(Debug, Clone, Serialize)]
pub struct SemanticAbstention<'a> {
    /// `true`: the query ran and was answered; the answer is "no confident
    /// match".
    pub ok: bool,
    /// Always `true` on this envelope; the machine-readable abstention flag.
    pub no_confident_match: bool,
    /// The query text, echoed for log correlation.
    pub query: &'a str,
    /// The calibrated floor the best candidate failed to clear.
    pub selection_threshold: f32,
    /// How the floor was set; see [`SEMANTIC_SELECTION_BASIS`].
    pub selection_basis: &'static str,
    /// Number of ranked candidates the verdict was computed over.
    pub total_candidates: usize,
    /// The highest similarity score observed among the candidates.
    pub highest_score: f32,
}

impl<'a> SemanticAbstention<'a> {
    /// Builds the verdict for a ranked candidate list whose best score is
    /// below the floor. Callers must check [`should_abstain`] first.
    #[must_use]
    pub const fn new(query: &'a str, highest_score: f32, total_candidates: usize) -> Self {
        Self {
            ok: true,
            no_confident_match: true,
            query,
            selection_threshold: SEMANTIC_CONFIDENCE_FLOOR,
            selection_basis: SEMANTIC_SELECTION_BASIS,
            total_candidates,
            highest_score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_boundary_is_inclusive() {
        assert_eq!(
            ConfidenceBand::of(SEMANTIC_CONFIDENCE_FLOOR),
            ConfidenceBand::Strong
        );
        assert!(!should_abstain(SEMANTIC_CONFIDENCE_FLOOR));
    }

    #[test]
    fn below_floor_is_weak_and_abstains() {
        assert_eq!(ConfidenceBand::of(0.0), ConfidenceBand::Weak);
        assert_eq!(
            ConfidenceBand::of(SEMANTIC_CONFIDENCE_FLOOR - 0.000_1),
            ConfidenceBand::Weak
        );
        assert!(should_abstain(0.0));
        assert!(should_abstain(SEMANTIC_CONFIDENCE_FLOOR - 0.000_1));
    }

    #[test]
    fn above_floor_is_strong_and_does_not_abstain() {
        assert_eq!(ConfidenceBand::of(1.0), ConfidenceBand::Strong);
        assert!(!should_abstain(1.0));
    }

    #[test]
    fn band_tags_are_stable() {
        assert_eq!(ConfidenceBand::Strong.as_str(), "strong");
        assert_eq!(ConfidenceBand::Weak.as_str(), "weak");
    }

    #[test]
    fn abstention_envelope_serializes_deterministically() {
        let verdict = SemanticAbstention::new("some query", 0.4123, 7);
        let first = serde_json::to_string(&verdict).expect("serialization cannot fail");
        let second = serde_json::to_string(&verdict).expect("serialization cannot fail");
        assert_eq!(first, second, "confidence fields must be byte-identical");
        let parsed: serde_json::Value = serde_json::from_str(&first).expect("valid JSON");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["no_confident_match"], true);
        assert_eq!(parsed["query"], "some query");
        assert_eq!(parsed["selection_threshold"], SEMANTIC_CONFIDENCE_FLOOR);
        assert_eq!(parsed["selection_basis"], SEMANTIC_SELECTION_BASIS);
        assert_eq!(parsed["total_candidates"], 7);
        assert_eq!(parsed["highest_score"], 0.4123);
    }
}
