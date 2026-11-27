// SPDX-License-Identifier: CC0-1.0

//! Where a confidence the SINGLE-DOCUMENT `Detect*` family reports comes FROM: the
//! material of the detection it is reported for, in the document the request carried.
//!
//! One rule, asserted per operation over a table whose rows differ along what the rule
//! generalises over rather than along one recorded document:
//!
//! 1. `DetectDominantLanguage`: the leading `Score` follows the identification's own
//!    evidence — it rises with the material the engine read, falls when two languages
//!    share that material, and never reaches a certainty.
//! 2. `DetectEntities`: one confidence per detected span, measured from that span in
//!    that document, so the spans of one answer are not reported with one value and the
//!    same span reads differently in two documents.
//! 3. `DetectPiiEntities`: the same, per matched value, and measured by its OWN
//!    recognizer — no confidence it reports recurs in the entity answer for the same
//!    document.
//! 4. `DetectSentiment`: the four members are one normalised distribution concentrated
//!    on the reported `Sentiment`, with the residual members at MUTUALLY DISTINCT
//!    magnitudes and none of them a flat zero.
//! 5. `DetectToxicContent`: `Labels` carries one room per content type, measured from
//!    what that type's own detectors read in the segment, so the vector is not one
//!    literal repeated, and `Toxicity` is derived from the vector rather than copied
//!    from a member of it.
//!
//! Numbers are never compared with a recorded answer here: every assertion is a
//! relation between two answers this emulator computed, or between the members of one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::comprehend_client::Comprehend;

/// An English single-document request carrying `text`.
fn document(text: &str) -> Value {
    json!({ "Text": text, "LanguageCode": "en" })
}

/// How a confidence is compared with another for IDENTITY: two detections scored off
/// one shared constant report the same value to every digit the wire carries, while two
/// confidences measured from different material differ far above this.
fn digits(score: f64) -> String {
    format!("{score:.12}")
}

/// Every `Score` of `path` in `answer`, in report order.
fn scores_at(case: &str, answer: &Value, path: &str) -> Vec<f64> {
    answer[path]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {path} array, got {answer}"))
        .iter()
        .map(|entry| {
            entry["Score"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Score, got {entry}"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. DetectDominantLanguage: the leading Score is the identification's evidence
// ---------------------------------------------------------------------------

/// The leading candidate's `(LanguageCode, Score)` for `text`.
async fn leading_language(comprehend: &Comprehend, case: &str, text: &str) -> (String, f64) {
    let output = comprehend
        .ok("DetectDominantLanguage", &json!({ "Text": text }))
        .await;
    let leading = output["Languages"]
        .as_array()
        .and_then(|languages| languages.first().cloned())
        .unwrap_or_else(|| panic!("case {case:?}: expected a leading candidate, got {output}"));
    (
        leading["LanguageCode"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        leading["Score"].as_f64().unwrap_or(f64::NAN),
    )
}

#[tokio::test]
async fn the_dominant_language_confidence_is_the_identification_the_text_supports() {
    let comprehend = Comprehend::new();

    // The rows differ along what the rule generalises over: how much material the
    // identification was made from, and how much of that material one language has to
    // itself. No row is the issue's recorded request.
    let fragment = "Good morning.";
    let sentence = "Good morning, the weather in London is rather pleasant today.";
    let paragraph = format!("{sentence} {sentence} {sentence} {sentence}");
    let shared = "Hola Ciao Hola Ciao Hola Ciao Hola Ciao Hola Ciao Hola Ciao Hola Ciao.";
    let mixed = "Good morning Hola Ciao Guten Tag Bonjour, the Zorblax K9 build 4711.";

    let mut rising = Vec::new();
    for (case, text) in [
        ("a fragment", fragment),
        ("a sentence", sentence),
        ("a paragraph", paragraph.as_str()),
    ] {
        let (code, score) = leading_language(&comprehend, case, text).await;
        assert_eq!(
            code, "en",
            "case {case:?}: expected the document's own language, got {code:?}"
        );
        assert!(
            score > 0.0 && score < 1.0,
            "case {case:?}: an identification is a confidence and never a certainty, \
             got {score}"
        );
        rising.push((case, score));
    }
    for window in rising.windows(2) {
        let [(shorter, lower), (longer, higher)] = [window[0], window[1]];
        assert!(
            higher > lower,
            "case {longer:?}: the identification rests on more of the document than \
             {shorter:?}, so it is reported above it, got {higher} against {lower}"
        );
    }

    // Material two or more languages share is the other half of the same rule: a
    // confidence that followed the amount of text alone would report these at the
    // sentence's value, since they are of the same length.
    let (_, sentence_score) = leading_language(&comprehend, "a sentence", sentence).await;
    for (case, text) in [("two languages share it", shared), ("four of them", mixed)] {
        let (_, score) = leading_language(&comprehend, case, text).await;
        assert!(
            score > 0.0 && score < sentence_score,
            "case {case:?}: the engine's margin over the same amount of material is \
             narrower than on a document plainly in one language, so the confidence is \
             reported below it, got {score} against {sentence_score}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2 and 3. DetectEntities / DetectPiiEntities: one confidence per detection
// ---------------------------------------------------------------------------

/// `(case, document)` rows for the two recognizers. They differ along what the rule
/// generalises over: how many detections the document carries, of how many types, how
/// much of each span its own form accounts for, and whether the document writes a
/// detection once or corroborates it.
const DETECTION_DOCUMENTS: &[(&str, &str)] = &[
    (
        "names, a place and a date",
        "Dr. Jane Doe joined Example Corporation in Berlin on 14 March 2023.",
    ),
    (
        "structured values beside a bare one",
        "Write to john.smith@example.com or call 555-0143 about invoice 4711.",
    ),
    (
        "an address, a name and a mail address",
        "Contact John Smith at 123 Main Street, Seattle, WA 98101 or \
         john.smith@example.com for more details today.",
    ),
];

#[tokio::test]
async fn every_entity_carries_the_confidence_its_own_span_leaves() {
    let comprehend = Comprehend::new();

    for (case, text) in DETECTION_DOCUMENTS {
        let output = comprehend.ok("DetectEntities", &document(text)).await;
        let scores = scores_at(case, &output, "Entities");
        assert!(
            scores.len() > 1,
            "case {case:?}: expected several entities to compare, got {output}"
        );
        for score in &scores {
            assert!(
                *score > 0.0 && *score <= 1.0,
                "case {case:?}: expected every Score inside (0, 1], got {score} in \
                 {output}"
            );
        }
        // One confidence per detection, measured from that detection: a scorer reading
        // off a ladder attached to the response shape reports the same few values for
        // unrelated spans.
        let distinct: BTreeSet<String> = scores.iter().copied().map(digits).collect();
        assert!(
            distinct.len() > 1,
            "case {case:?}: every entity is reported at the same confidence {scores:?}, \
             which is a constant of the shape rather than evidence for the span"
        );
    }

    // The same span, in two documents that say different amounts about it: the
    // confidence follows the document, so a name the document writes again is reported
    // above the same name written once.
    let once = comprehend
        .ok("DetectEntities", &document("Zorblax Initiative filed the report."))
        .await;
    let repeated = comprehend
        .ok(
            "DetectEntities",
            &document(
                "Zorblax Initiative filed the report. Zorblax Initiative filed it on \
                 time.",
            ),
        )
        .await;
    let score_of = |case: &str, answer: &Value| -> f64 {
        answer["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .find(|entity| entity["Text"].as_str() == Some("Zorblax Initiative"))
            .and_then(|entity| entity["Score"].as_f64())
            .unwrap_or_else(|| panic!("case {case:?}: no Zorblax Initiative in {answer}"))
    };
    let (single, corroborated) = (
        score_of("written once", &once),
        score_of("written twice", &repeated),
    );
    assert!(
        corroborated > single,
        "case \"corroborated mention\": a document that writes the name again \
         corroborates the reading, so it is reported above the same name written once, \
         got {corroborated} against {single}"
    );
}

/// One detection as an answer reports it: which operation reported it, the characters
/// it covers, what the document evidences it with — its type and how many characters
/// the evidence had to account for — and the confidence reported, compared by
/// [`digits`].
struct Reported {
    operation: &'static str,
    span: (u64, u64),
    evidence: (String, u64),
    score: String,
}

#[tokio::test]
async fn every_pii_detection_carries_its_own_confidence_from_its_own_recognizer() {    let comprehend = Comprehend::new();

    for (case, text) in DETECTION_DOCUMENTS {
        let pii = comprehend.ok("DetectPiiEntities", &document(text)).await;
        let pii_scores = scores_at(case, &pii, "Entities");
        if pii_scores.len() > 1 {
            let distinct: BTreeSet<String> = pii_scores.iter().copied().map(digits).collect();
            assert!(
                distinct.len() > 1,
                "case {case:?}: every PII detection is reported at the same confidence \
                 {pii_scores:?}, which is a constant of the shape rather than evidence \
                 for the matched value"
            );
        }
        for score in &pii_scores {
            assert!(
                *score > 0.0 && *score <= 1.0,
                "case {case:?}: expected every Score inside (0, 1], got {score} in {pii}"
            );
        }

        // No confidence is shared between detections the document gives DIFFERENT
        // evidence for — a span of another type, or of another length, whether the
        // same operation or two operations report it. Two detections whose evidence is
        // the same in kind and amount (two four-digit quantities of one document) are
        // reported alike because their evidence is alike, and two operations reporting
        // the same characters report one recognition twice; what a shared constant pool
        // looks like is one value standing for two readings the document supports
        // differently.
        let entities = comprehend.ok("DetectEntities", &document(text)).await;
        let mut seen: Vec<Reported> = Vec::new();
        for (operation, answer) in [("DetectEntities", &entities), ("DetectPiiEntities", &pii)] {
            for entry in answer["Entities"].as_array().cloned().unwrap_or_default() {
                let span = (
                    entry["BeginOffset"].as_u64().unwrap_or_default(),
                    entry["EndOffset"].as_u64().unwrap_or_default(),
                );
                let current = Reported {
                    operation,
                    span,
                    evidence: (
                        entry["Type"].as_str().unwrap_or_default().to_string(),
                        span.1.saturating_sub(span.0),
                    ),
                    score: digits(entry["Score"].as_f64().unwrap_or(f64::NAN)),
                };
                if let Some(previous) = seen.iter().find(|previous| {
                    previous.score == current.score
                        && previous.span != current.span
                        && previous.evidence != current.evidence
                }) {
                    panic!(
                        "case {case:?}: {} reports {} for {:?}, which {} already reports \
                         for {:?} — two detections the document evidences differently \
                         carrying one literal is a shared pool rather than each \
                         detection's own evidence: {entities} against {pii}",
                        current.operation,
                        current.score,
                        current.span,
                        previous.operation,
                        previous.span
                    );
                }
                seen.push(current);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. DetectSentiment: one distribution, with distinct residual members
// ---------------------------------------------------------------------------

/// The four modelled members of `SentimentScore`.
const SENTIMENT_MEMBERS: [&str; 4] = ["Positive", "Negative", "Neutral", "Mixed"];

/// `(case, document, whether the document carries material of a polarity)`. The rows
/// differ along what the rule generalises over: which polarity the document carries,
/// how much of it does, and whether it carries both.
const SENTIMENT_DOCUMENTS: &[(&str, &str, bool)] = &[
    (
        "plain fact",
        "The quarterly report is published on the investor relations page.",
        false,
    ),
    ("one positive word", "The release notes were helpful.", true),
    (
        "wholly positive",
        "Wonderful, excellent work — the release was delightful and the team was superb.",
        true,
    ),
    (
        "one negative word",
        "The release notes were confusing.",
        true,
    ),
    (
        "wholly negative",
        "Terrible, awful work — the release was dreadful and the outage was appalling.",
        true,
    ),
    (
        "both polarities",
        "The release was excellent but the outage was awful.",
        true,
    ),
];

#[tokio::test]
async fn the_sentiment_distribution_is_the_documents_own_with_distinct_residuals() {
    let comprehend = Comprehend::new();
    let mut residual_of: Vec<(&str, Vec<String>)> = Vec::new();

    for (case, text, polar) in SENTIMENT_DOCUMENTS {
        let output = comprehend.ok("DetectSentiment", &document(text)).await;
        let members: Vec<(&str, f64)> = SENTIMENT_MEMBERS
            .into_iter()
            .map(|member| {
                let score = output["SentimentScore"][member].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: expected SentimentScore.{member}, got {output}")
                });
                (member, score)
            })
            .collect();
        let member = |name: &str| -> f64 {
            members
                .iter()
                .find(|(reported, _)| *reported == name)
                .map_or(f64::NAN, |(_, score)| *score)
        };

        let total: f64 = members.iter().map(|(_, score)| score).sum();
        assert!(
            (total - 1.0).abs() < 1e-5,
            "case {case:?}: the four confidences are one distribution, so they sum to \
             1, got {total} from {output}"
        );

        // A confidence is never a flat impossibility: a client comparing two documents
        // on the log scale of a residual member divides by it.
        for (name, score) in &members {
            assert!(
                *score > 0.0 && *score < 1.0,
                "case {case:?}: expected SentimentScore.{name} strictly inside \
                 (0, 1), got {score} in {output}"
            );
        }

        // The distribution is concentrated on the reported verdict, which is its
        // arg-max.
        let verdict = output["Sentiment"].as_str().unwrap_or_default().to_string();
        let leading = members
            .iter()
            .copied()
            .reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best })
            .map(|(name, _)| name.to_uppercase())
            .unwrap_or_default();
        assert_eq!(
            verdict, leading,
            "case {case:?}: the verdict is the arg-max of the four confidences, got \
             {output}"
        );

        // MIXED is the confidence the document carries BOTH polarities, so it is open
        // only where both signed readings are: a document carrying at most one keeps
        // the room the two of them leave together, which is below either of them.
        if verdict != "MIXED" {
            assert!(
                member("Mixed") < member("Positive").min(member("Negative")),
                "case {case:?}: the document carries at most one polarity, so MIXED is \
                 reported below both signed readings, got {output}"
            );
        }

        let residuals: Vec<(&str, f64)> = members
            .iter()
            .copied()
            .filter(|(name, _)| !verdict.eq_ignore_ascii_case(name))
            .collect();
        let distinct: BTreeSet<String> = residuals.iter().map(|(_, score)| digits(*score)).collect();
        if *polar {
            // The members the verdict leaves are MUTUALLY DISTINCT: each keeps the room
            // THIS document leaves that reading, so a client reading them apart reads
            // what the document says and not one shared floor.
            assert_eq!(
                distinct.len(),
                residuals.len(),
                "case {case:?}: the residual members are each the room the document \
                 leaves that reading, so no two of them are one value, got {residuals:?}"
            );
        } else {
            // A document with no material of either polarity leaves the two signed
            // readings open to exactly the same extent, so they are reported alike —
            // equal evidence, equal confidence — while MIXED and the verdict's own
            // residual still differ from them.
            assert_eq!(
                digits(member("Positive")),
                digits(member("Negative")),
                "case {case:?}: nothing in the document favours either polarity, so the \
                 two signed readings keep the same room, got {output}"
            );
            assert!(
                distinct.len() > 1,
                "case {case:?}: the residual members are not one single value, got \
                 {residuals:?}"
            );
        }
        residual_of.push((
            case,
            residuals
                .iter()
                .map(|(_, score)| digits(*score))
                .collect::<Vec<String>>(),
        ));
    }

    // And the residual members of two DOCUMENTS are not one shared literal either: a
    // floor attached to the response shape says the same thing about every document.
    for window in residual_of.windows(2) {
        let (first, second) = (&window[0], &window[1]);
        assert_ne!(
            first.1, second.1,
            "cases {:?} and {:?}: the residual members are the room each document \
             leaves, so two documents do not report one set of floors",
            first.0, second.0
        );
    }
}

// ---------------------------------------------------------------------------
// 5. DetectToxicContent: a per-type vector, and a Toxicity derived from it
// ---------------------------------------------------------------------------

/// `(case, segment, whether the analysers place any toxic content in it)`. The rows
/// differ along what the rule generalises over: how much the segment gives the
/// detectors to read, and whether anything is placed in it.
const TOXIC_SEGMENTS: &[(&str, &str, bool)] = &[
    ("a single benign word", "hello", false),
    (
        "a benign sentence",
        "The quarterly report is published on the investor relations page.",
        false,
    ),
    (
        "a benign paragraph",
        "The quarterly report is published on the investor relations page. Maria Lopez \
         moved to Lisbon in March and joined the research team, where she now reviews \
         the migration plan every Tuesday afternoon.",
        false,
    ),
    ("abuse directed at the recipient", "You are worthless.", true),
    (
        "a threat and abuse in one segment",
        "You are pathetic and I will kill you for it.",
        true,
    ),
];

#[tokio::test]
async fn toxicity_reports_a_per_type_vector_and_an_overall_score_derived_from_it() {
    let comprehend = Comprehend::new();
    let mut room_of: Vec<(&str, f64)> = Vec::new();

    for (case, text, placed) in TOXIC_SEGMENTS {
        let output = comprehend
            .ok(
                "DetectToxicContent",
                &json!({ "LanguageCode": "en", "TextSegments": [{ "Text": text }] }),
            )
            .await;
        let entry = &output["ResultList"][0];
        let labels = entry["Labels"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Labels array, got {output}"));
        let scores: Vec<f64> = labels
            .iter()
            .map(|label| label["Score"].as_f64().unwrap_or(f64::NAN))
            .collect();
        let toxicity = entry["Toxicity"].as_f64().unwrap_or(f64::NAN);

        for (label, score) in labels.iter().zip(&scores) {
            assert!(
                *score > 0.0 && *score <= 1.0,
                "case {case:?}: expected a Score inside (0, 1] for {}, got {score}",
                label["Name"]
            );
        }

        // One room per content type, measured from what THAT type's detectors read: a
        // single constant for the whole result ties every label, so the top-label
        // choice a moderation client makes is arbitrary.
        let distinct: BTreeSet<String> = scores.iter().copied().map(digits).collect();
        assert!(
            distinct.len() > 1,
            "case {case:?}: every content type is reported at the same confidence \
             {scores:?}, so no type can be distinguished from another"
        );

        // `Toxicity` is derived from the vector: at least as large as the strongest
        // type, never larger than the whole vector combined.
        let strongest = scores.iter().copied().fold(0.0f64, f64::max);
        let combined = 1.0 - scores.iter().map(|score| 1.0 - score).product::<f64>();
        assert!(
            toxicity + 1e-6 >= strongest && toxicity <= combined + 1e-6,
            "case {case:?}: expected a Toxicity between the strongest type {strongest} \
             and the whole vector combined {combined}, got {toxicity}"
        );

        if !*placed {
            // Nothing was placed, so the overall score is what the rooms the WHOLE
            // vector keeps add up to — above every one of them, and a copy of none, so
            // a client comparing it against a policy threshold is not reading one of
            // the label values back.
            assert!(
                (toxicity - combined).abs() < 1e-6
                    && scores.iter().all(|score| toxicity > *score),
                "case {case:?}: with nothing placed, the overall Toxicity is the \
                 vector's rooms combined ({combined}), got {toxicity} with {scores:?}"
            );
            room_of.push((case, strongest));
        }
    }

    // Nothing was placed in those rows, so each type keeps the room its detectors'
    // observations leave — and that room FALLS as the segment gives them more to read,
    // which is what makes it a reading of the segment rather than a constant.
    for window in room_of.windows(2) {
        let [(shorter, roomier), (longer, tighter)] = [window[0], window[1]];
        assert!(
            tighter < roomier,
            "case {longer:?}: the detectors read more of it without placing anything \
             than of {shorter:?}, so every type keeps less room, got {tighter} against \
             {roomier}"
        );
    }
}

#[tokio::test]
async fn reading_a_document_twice_reports_the_same_confidences() {
    let comprehend = Comprehend::new();
    let text = DETECTION_DOCUMENTS[2].1;

    for (operation, request) in [
        ("DetectDominantLanguage", json!({ "Text": text })),
        ("DetectEntities", document(text)),
        ("DetectPiiEntities", document(text)),
        ("DetectSentiment", document(text)),
        (
            "DetectToxicContent",
            json!({ "LanguageCode": "en", "TextSegments": [{ "Text": text }] }),
        ),
    ] {
        let first = comprehend.ok(operation, &request).await;
        let second = comprehend.ok(operation, &request).await;
        assert_eq!(
            first, second,
            "case {operation:?}: inference holds no state, so a repeated call reports \
             the same confidences, got {first} then {second}"
        );
    }
}
