#![allow(missing_docs)]

//! TDD tests for issue #263: calibrated confidence floor and abstention for
//! `eg query semantic`.
//!
//! RED PHASE: these tests fail until `src/semantic_confidence.rs`,
//! `corpus/semantic_confidence_fixture.json`, and the CLI wiring exist.
//!
//! The fixture pins the calibrated floor and two real measured cases (a
//! strong in-corpus match and a below-floor abstention); the tests assert the
//! code agrees with the fixture, so the floor cannot drift silently.

use std::path::PathBuf;

use aletheia_egregore::semantic_confidence::{
    ConfidenceBand, SEMANTIC_CONFIDENCE_FLOOR, SEMANTIC_SELECTION_BASIS, SemanticAbstention,
    should_abstain,
};
use serde::Deserialize;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/semantic_confidence_fixture.json")
}

#[derive(Deserialize)]
struct FixtureHit {
    record_id: String,
    score: f32,
}

#[derive(Deserialize)]
struct FixtureCase {
    query: String,
    ranked_hits: Vec<FixtureHit>,
    expected_abstention: bool,
}

#[derive(Deserialize)]
struct Fixture {
    confidence_floor: f32,
    selection_basis: String,
    strong_case: FixtureCase,
    abstention_case: FixtureCase,
}

fn load_fixture() -> Fixture {
    let text = std::fs::read_to_string(fixture_path()).expect("read confidence fixture");
    serde_json::from_str(&text).expect("confidence fixture must be valid JSON")
}

fn best_score(case: &FixtureCase) -> f32 {
    case.ranked_hits
        .iter()
        .map(|h| h.score)
        .fold(f32::NEG_INFINITY, f32::max)
}

// ---------------------------------------------------------------------------
// Floor pinning: the code constant must equal the documented fixture value.
// ---------------------------------------------------------------------------

#[test]
fn floor_matches_documented_fixture_value() {
    let fixture = load_fixture();
    // Float equality via epsilon: the fixture pins the exact calibrated value,
    // and the constant must match it bit-for-bit in practice.
    assert!(
        (SEMANTIC_CONFIDENCE_FLOOR - fixture.confidence_floor).abs() < f32::EPSILON,
        "SEMANTIC_CONFIDENCE_FLOOR ({}) must equal the calibrated value pinned in corpus/semantic_confidence_fixture.json ({})",
        SEMANTIC_CONFIDENCE_FLOOR,
        fixture.confidence_floor,
    );
}

#[test]
fn selection_basis_matches_documented_fixture_value() {
    let fixture = load_fixture();
    assert_eq!(
        SEMANTIC_SELECTION_BASIS, fixture.selection_basis,
        "selection basis tag must match the fixture"
    );
}

// ---------------------------------------------------------------------------
// Strong case: no abstention, every row strong.
// ---------------------------------------------------------------------------

#[test]
fn strong_case_does_not_abstain() {
    let fixture = load_fixture();
    assert!(
        !fixture.strong_case.expected_abstention,
        "fixture strong case must expect no abstention"
    );
    let best = best_score(&fixture.strong_case);
    assert!(
        !should_abstain(best),
        "strong case best score {best} must clear the floor {SEMANTIC_CONFIDENCE_FLOOR}"
    );
}

#[test]
fn strong_case_rows_all_classify_strong() {
    let fixture = load_fixture();
    for hit in &fixture.strong_case.ranked_hits {
        assert_eq!(
            ConfidenceBand::of(hit.score),
            ConfidenceBand::Strong,
            "strong-case row {} score {} must classify strong",
            hit.record_id,
            hit.score
        );
    }
}

// ---------------------------------------------------------------------------
// Abstention case: best below floor, envelope is explicit and stable.
// ---------------------------------------------------------------------------

#[test]
fn abstention_case_abstains() {
    let fixture = load_fixture();
    assert!(
        fixture.abstention_case.expected_abstention,
        "fixture abstention case must expect abstention"
    );
    let best = best_score(&fixture.abstention_case);
    assert!(
        should_abstain(best),
        "abstention case best score {best} must be below the floor {SEMANTIC_CONFIDENCE_FLOOR}"
    );
}

#[test]
fn abstention_envelope_carries_required_fields() {
    let fixture = load_fixture();
    let case = &fixture.abstention_case;
    let best = best_score(case);
    let verdict = SemanticAbstention::new(&case.query, best, case.ranked_hits.len());
    let json = serde_json::to_value(&verdict).expect("abstention must serialize");

    assert_eq!(json["ok"], true, "abstention is a completed answer");
    assert_eq!(
        json["no_confident_match"], true,
        "explicit abstention flag must be true"
    );
    assert_eq!(json["query"], case.query.as_str());
    assert_eq!(json["selection_threshold"], SEMANTIC_CONFIDENCE_FLOOR);
    assert_eq!(json["selection_basis"], SEMANTIC_SELECTION_BASIS);
    assert_eq!(json["total_candidates"], case.ranked_hits.len());
    assert_eq!(json["highest_score"], best);
}

#[test]
fn abstention_envelope_is_byte_identical_across_repeats() {
    let fixture = load_fixture();
    let case = &fixture.abstention_case;
    let best = best_score(case);
    let first = serde_json::to_string(&SemanticAbstention::new(
        &case.query,
        best,
        case.ranked_hits.len(),
    ))
    .expect("serialize");
    for _ in 0..4 {
        let again = serde_json::to_string(&SemanticAbstention::new(
            &case.query,
            best,
            case.ranked_hits.len(),
        ))
        .expect("serialize");
        assert_eq!(
            first, again,
            "five repeated queries must produce byte-identical confidence fields"
        );
    }
}

// ---------------------------------------------------------------------------
// Boundary and tag stability.
// ---------------------------------------------------------------------------

#[test]
fn floor_boundary_is_inclusive_for_abstention() {
    assert!(
        !should_abstain(SEMANTIC_CONFIDENCE_FLOOR),
        "a score exactly at the floor must not abstain"
    );
    assert_eq!(
        ConfidenceBand::of(SEMANTIC_CONFIDENCE_FLOOR),
        ConfidenceBand::Strong
    );
}

#[test]
fn confidence_band_tags_are_stable() {
    assert_eq!(ConfidenceBand::Strong.as_str(), "strong");
    assert_eq!(ConfidenceBand::Weak.as_str(), "weak");
}
