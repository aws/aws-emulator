// SPDX-License-Identifier: CC0-1.0

//! The `DetectToxicContent` categories that are about the CONDUCT a segment carries,
//! and the confidence the operation reports for what it placed.
//!
//! RULE: every modelled `ToxicContentType` has a detector — the four about the
//! language a segment uses are classified by the profanity/abuse engine, and the
//! three about conduct (a threat of physical harm, abuse directed at the recipient, a
//! gruesome depiction) by the forms each of those categories is published as covering
//! — and the reported confidences are computed from what was actually placed: a
//! `Score` combines every placement of its type and `Toxicity` every placement of
//! every type, so both rise with how much toxic content the segment carries and
//! neither is a constant of a severity rung.
//!
//! `ToxicLabels$Toxicity` is modelled as "Overall toxicity score for the string.
//! Value range is zero to one, where one is the highest confidence"
//! (`smithy_query(query="shape",shape="ToxicLabels")`), so a moderation client's
//! customary 0.5 gate is a class boundary over that range: a segment carrying an
//! explicit threat sits above it, a benign one at zero.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::TOXIC_CONTENT_TYPE_VALUES;

use crate::comprehend_client::Comprehend;

/// The gate a moderation client reads `Toxicity` against: the midpoint of the
/// modelled zero-to-one range, which is the class boundary between a segment that
/// carries toxic content and one that does not.
const TOXIC_GATE: f64 = 0.5;

/// One row: a segment, the modelled `ToxicContentType` values the analysers must
/// place in it, and whether its overall `Toxicity` has to clear [`TOXIC_GATE`].
///
/// The rows differ along what the rule generalises over: which category the content
/// belongs to, which published form carries it (a stated intention, an imperative,
/// harm aimed at a body part, intimidation, a wish, a demeaning address, a silencing
/// command, vivid imagery), how many categories one segment carries, and benign
/// segments that come near a form without carrying it. `HATE_SPEECH` is the one
/// modelled type no row covers: its detector is the engine's class for slurs and
/// discriminatory language, which this suite does not spell out.
struct Row {
    case: &'static str,
    text: &'static str,
    expected: &'static [&'static str],
    above_gate: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "an explicit threat of physical harm with a demeaning address",
        text: "I will find you and hurt you badly, you worthless piece of garbage, and \
               you deserve every bit of pain coming your way.",
        expected: &["HARASSMENT_OR_ABUSE", "VIOLENCE_OR_THREAT"],
        above_gate: true,
    },
    Row {
        case: "a stated intention to harm the recipient",
        text: "I am going to hurt you after work.",
        expected: &["VIOLENCE_OR_THREAT"],
        above_gate: true,
    },
    Row {
        case: "an act aimed at a person with no other framing",
        text: "Someone should stab him for that.",
        expected: &["VIOLENCE_OR_THREAT"],
        above_gate: true,
    },
    Row {
        case: "harm aimed at a part of the recipient's body",
        text: "Pay up or we break your kneecaps.",
        expected: &["VIOLENCE_OR_THREAT"],
        above_gate: true,
    },
    Row {
        case: "intimidation with no act named",
        text: "Watch your back, I know where you live.",
        expected: &["VIOLENCE_OR_THREAT"],
        above_gate: true,
    },
    Row {
        case: "a wish of harm, with no act of the speaker's own",
        text: "He deserves to die for what he wrote.",
        expected: &["VIOLENCE_OR_THREAT"],
        above_gate: false,
    },
    Row {
        case: "a demeaning epithet addressed to the recipient",
        text: "You are a worthless, clueless nobody.",
        expected: &["HARASSMENT_OR_ABUSE"],
        above_gate: true,
    },
    Row {
        case: "a command silencing the recipient",
        text: "Shut up, nobody likes you.",
        expected: &["HARASSMENT_OR_ABUSE"],
        above_gate: true,
    },
    Row {
        case: "persistent unwanted attention",
        text: "I will keep messaging you, you cannot hide.",
        expected: &["HARASSMENT_OR_ABUSE"],
        // Two forms of it, each alluded to rather than stated: together they clear
        // the gate a single one does not.
        above_gate: true,
    },
    Row {
        case: "vivid bodily imagery with the predicate that depicts it",
        text: "There was blood everywhere, pouring across the floor.",
        expected: &["GRAPHIC"],
        above_gate: true,
    },
    Row {
        case: "a term whose only reading is a gruesome depiction",
        text: "The remains were dismembered.",
        expected: &["GRAPHIC"],
        above_gate: false,
    },
    Row {
        case: "a bodily word with no depiction around it",
        text: "The nurse stopped the bleeding quickly and sent her home.",
        expected: &[],
        above_gate: false,
    },
    Row {
        case: "a verb of harm in its everyday reading",
        text: "We should break for lunch and attack the backlog after.",
        expected: &[],
        above_gate: false,
    },
    Row {
        case: "a friendly message naming the recipient",
        text: "I will call you tomorrow about the schedule.",
        expected: &[],
        above_gate: false,
    },
    Row {
        case: "an emotive topic reported rather than carried",
        text: "The documentary about the war was hard to watch.",
        expected: &[],
        above_gate: false,
    },
];

/// A request carrying `texts` as `TextSegments`, in English.
fn request(texts: &[&str]) -> Value {
    json!({
        "LanguageCode": "en",
        "TextSegments": texts
            .iter()
            .map(|text| json!({ "Text": text }))
            .collect::<Vec<_>>(),
    })
}

/// The `(Name, Score)` pairs of the first `ResultList` entry the analysers PLACED
/// content of: the entries of the per-type score vector scoring above zero. The
/// vector carries every modelled type whatever the segment says, so "placed" is a
/// non-zero score rather than a present member.
fn labels(case: &str, output: &Value) -> Vec<(String, f64)> {
    output["ResultList"][0]["Labels"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Labels array, got {output}"))
        .iter()
        .map(|label| {
            (
                label["Name"].as_str().unwrap_or_default().to_string(),
                label["Score"].as_f64().unwrap_or(f64::NAN),
            )
        })
        .filter(|(_, score)| *score > 0.0)
        .collect()
}

/// The `Toxicity` of the first `ResultList` entry.
fn toxicity(case: &str, output: &Value) -> f64 {
    output["ResultList"][0]["Toxicity"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Toxicity, got {output}"))
}

/// The toxicity verdict for one segment.
async fn verdict(comprehend: &Comprehend, text: &str) -> Value {
    comprehend
        .ok("DetectToxicContent", &request(&[text]))
        .await
}

#[tokio::test]
async fn each_published_form_places_its_own_content_type() {
    let comprehend = Comprehend::new();

    for row in ROWS {
        let output = verdict(&comprehend, row.text).await;
        let placed = labels(row.case, &output);
        let overall = toxicity(row.case, &output);

        for (name, score) in &placed {
            assert!(
                TOXIC_CONTENT_TYPE_VALUES.contains(&name.as_str()),
                "case {:?}: {name:?} is not a modelled ToxicContentType value",
                row.case
            );
            assert!(
                *score > 0.0 && *score <= 1.0,
                "case {:?}: expected a Score in (0, 1] for {name:?}, got {score}",
                row.case
            );
        }

        for expected in row.expected {
            assert!(
                placed.iter().any(|(name, _)| name == expected),
                "case {:?}: expected a {expected:?} label, got {placed:?}",
                row.case
            );
        }
        for absent in row.expected.is_empty().then_some(()).iter().flat_map(|()| {
            ["GRAPHIC", "HARASSMENT_OR_ABUSE", "VIOLENCE_OR_THREAT"]
        }) {
            assert!(
                !placed.iter().any(|(name, _)| name == absent),
                "case {:?}: the segment carries no conduct of any category, so {absent:?} \
                 must not be placed, got {placed:?}",
                row.case
            );
        }

        assert_eq!(
            overall > TOXIC_GATE,
            row.above_gate,
            "case {:?}: expected Toxicity {} {TOXIC_GATE}, got {overall} with {placed:?}",
            row.case,
            if row.above_gate { "above" } else { "at or below" }
        );
        assert!(
            (0.0..=1.0).contains(&overall),
            "case {:?}: expected a Toxicity in [0, 1], got {overall}",
            row.case
        );
        assert_eq!(
            overall > 0.0,
            !placed.is_empty(),
            "case {:?}: a segment nothing was placed in scores zero and carries no label, \
             got {overall} with {placed:?}",
            row.case
        );
    }
}

/// The conduct categories the issue's engine had no detector for are REACHABLE: the
/// table above places each of them, so a moderation client gating on them sees them.
#[tokio::test]
async fn the_conduct_content_types_are_reachable_labels() {
    let comprehend = Comprehend::new();
    let mut reached: Vec<String> = Vec::new();

    for row in ROWS {
        let output = verdict(&comprehend, row.text).await;
        for (name, _) in labels(row.case, &output) {
            if !reached.contains(&name) {
                reached.push(name);
            }
        }
    }

    for expected in ["GRAPHIC", "HARASSMENT_OR_ABUSE", "VIOLENCE_OR_THREAT"] {
        assert!(
            reached.contains(&expected.to_string()),
            "case {expected:?}: expected some row of the table to place this modelled \
             ToxicContentType, reached {reached:?}"
        );
    }
}

/// More of the content, more confidence: a score is computed from the placements, so
/// adding another instance of the same category raises that category's `Score` and the
/// overall `Toxicity`, and a second category raises the `Toxicity` again.
#[tokio::test]
async fn a_confidence_rises_with_what_the_segment_carries() {
    let comprehend = Comprehend::new();

    for (case, less, more, category) in [
        (
            "one threat against two of them",
            "I will hurt you tomorrow.",
            "I will hurt you tomorrow and I will kill you after that.",
            "VIOLENCE_OR_THREAT",
        ),
        (
            "one demeaning address against two of them",
            "You are pathetic.",
            "You are pathetic, and you are a worthless idiot.",
            "HARASSMENT_OR_ABUSE",
        ),
    ] {
        let weaker = verdict(&comprehend, less).await;
        let stronger = verdict(&comprehend, more).await;

        let score_of = |output: &Value, which: &str| -> f64 {
            labels(case, output)
                .into_iter()
                .find(|(name, _)| name == category)
                .map(|(_, score)| score)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a {category} label in the {which} segment, \
                         got {output}"
                    )
                })
        };
        let weaker_score = score_of(&weaker, "weaker");
        let stronger_score = score_of(&stronger, "stronger");
        assert!(
            stronger_score > weaker_score,
            "case {case:?}: expected the {category} Score to rise with a second instance, \
             got {stronger_score} against {weaker_score}"
        );
        assert!(
            toxicity(case, &stronger) > toxicity(case, &weaker),
            "case {case:?}: expected the Toxicity to rise with a second instance, got {} \
             against {}",
            toxicity(case, &stronger),
            toxicity(case, &weaker)
        );
    }

    // A second CATEGORY is further evidence that the segment is toxic, so it raises
    // the overall confidence above what either category reaches on its own.
    let threat = verdict(&comprehend, "I will hurt you tomorrow.").await;
    let abuse = verdict(&comprehend, "You are a worthless idiot.").await;
    let both = verdict(
        &comprehend,
        "I will hurt you tomorrow, you worthless idiot.",
    )
    .await;
    let case = "a second category raises the overall confidence";
    assert!(
        toxicity(case, &both) > toxicity(case, &threat).max(toxicity(case, &abuse)),
        "case {case:?}: expected {} to exceed both {} and {}",
        toxicity(case, &both),
        toxicity(case, &threat),
        toxicity(case, &abuse)
    );
}

/// The reported confidences are not a ladder of three constants: across the table the
/// scores take many distinct values, because each one is combined from the placements
/// of its own segment.
#[tokio::test]
async fn the_reported_confidences_are_not_three_constants() {
    let comprehend = Comprehend::new();
    let mut scores: Vec<f64> = Vec::new();

    for row in ROWS {
        let output = verdict(&comprehend, row.text).await;
        scores.push(toxicity(row.case, &output));
        scores.extend(labels(row.case, &output).into_iter().map(|(_, score)| score));
    }
    scores.retain(|score| *score > 0.0);
    scores.sort_by(f64::total_cmp);
    scores.dedup_by(|left, right| (*left - *right).abs() < 1e-9);

    assert!(
        scores.len() > 3,
        "expected the confidences to follow what each segment carries, got {} distinct \
         values: {scores:?}",
        scores.len()
    );
}
