// SPDX-License-Identifier: CC0-1.0

//! Comprehend's `DetectToxicContent`.
//!
//! The operation is a pure function of `TextSegments` and `LanguageCode`, so every
//! suite here asserts the RULE over a table of strings rather than one recorded
//! request: one `ToxicLabels` entry per submitted string in input order, each
//! entry's `Labels` carrying the per-type score vector (asserted as a vector in
//! `single_document_conventions.rs`), and its `Toxicity` the confidence that the
//! string carries toxic content of any type — so a string the engine places nothing
//! in scores strictly below one it does. A type the engine PLACED content of is one
//! whose entry scores above zero, which is what the helpers below read.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    LANGUAGE_CODE_VALUES, TOXIC_CONTENT_TYPE_VALUES,
};

use crate::comprehend_client::{error_code, error_message, expected_text_size_message, Comprehend};

// Every modelled `ToxicContentType` has a detector in this service (the four about
// the language a segment uses in the handler's `ENGINE_CATEGORIES`, the three about
// the conduct it carries in its `conduct_rules`), and which forms place which type
// is asserted in `toxic_content_conduct.rs`. What this suite asserts about a
// reported name is only that it is a value of the modelled enum.

/// One row of the table: a string, whether the toxicity engine must place any
/// toxic content in it, and — where the mapping from the engine's own category is
/// the point — the modelled `ToxicContentType` the entry must name.
struct Segment {
    case: &'static str,
    text: &'static str,
    toxic: bool,
    expected_label: Option<&'static str>,
}

/// A toxic row: the engine places content in it, and names `label` when given.
const fn toxic(case: &'static str, text: &'static str, label: Option<&'static str>) -> Segment {
    Segment {
        case,
        text,
        toxic: true,
        expected_label: label,
    }
}

/// A benign row: the engine places nothing in it, so it carries no label.
const fn benign(case: &'static str, text: &'static str) -> Segment {
    Segment {
        case,
        text,
        toxic: false,
        expected_label: None,
    }
}

/// The strings every suite below is asserted over. Each row carries its own
/// expectation, so a row may be inserted anywhere without reclassifying another.
///
/// The rows differ along what the rule generalises over: the kind of toxic content
/// (profanity, insult, sexual), how much of the string carries it, and benign
/// strings of several forms (praise, a factual sentence, one that merely mentions
/// an emotive topic, a single word). No row is the issue's recorded request.
const SEGMENTS: &[Segment] = &[
    toxic(
        "profanity",
        "What the fuck is this piece of shit?",
        Some("PROFANITY"),
    ),
    toxic("insult", "Shut up, you stupid moron.", Some("INSULT")),
    toxic("sexual", "I want to fuck you, send nudes.", Some("SEXUAL")),
    toxic("mild profanity", "damn this is annoying", Some("PROFANITY")),
    toxic(
        "demeaning",
        "You are a worthless idiot and nobody likes you.",
        None,
    ),
    benign(
        "praise",
        "Thank you for the thoughtful review, it was very helpful.",
    ),
    benign(
        "factual",
        "The meeting is scheduled for three in the afternoon.",
    ),
    benign(
        "emotive topic",
        "The documentary about the war was hard to watch.",
    ),
    benign("single word", "hello"),
];

/// The rows the engine must place toxic content in.
fn toxic_rows() -> impl Iterator<Item = &'static Segment> {
    SEGMENTS.iter().filter(|segment| segment.toxic)
}

/// The rows the engine must place nothing in.
fn benign_rows() -> impl Iterator<Item = &'static Segment> {
    SEGMENTS.iter().filter(|segment| !segment.toxic)
}

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

/// Every `(Name, Score)` pair of one `ResultList` entry the analysers PLACED content
/// of: the entries of the per-type score vector scoring above zero. The vector
/// carries every modelled type whatever the string says, so "placed" is a non-zero
/// score rather than a present member.
fn labels_of(case: &str, entry: &Value) -> Vec<(String, f64)> {
    entry["Labels"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Labels array, got {entry}"))
        .iter()
        .map(|label| {
            let name = label["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected Labels[].Name, got {label}"))
                .to_string();
            let score = label["Score"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: expected Labels[].Score, got {label}"));
            (name, score)
        })
        .filter(|(_, score)| *score > 0.0)
        .collect()
}

/// The `Toxicity` of one `ResultList` entry.
fn toxicity_of(case: &str, entry: &Value) -> f64 {
    entry["Toxicity"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Toxicity, got {entry}"))
}

#[tokio::test]
async fn every_result_entry_is_a_well_formed_toxicity_verdict_for_its_string() {
    let comprehend = Comprehend::new();

    for Segment {
        case,
        text,
        toxic,
        expected_label,
    } in SEGMENTS
    {
        let output = comprehend.ok("DetectToxicContent", &request(&[text])).await;
        let results = output["ResultList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResultList, got {output}"))
            .clone();
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: the results list matches the size of the input list, got {output}"
        );

        let labels = labels_of(case, &results[0]);
        let toxicity = toxicity_of(case, &results[0]);

        // Every placed name is a modelled `ToxicContentType`, reported at most once,
        // with a confidence in (0, 1] — a type the engine placed nothing of scores
        // zero in the vector, which the helper above leaves out.
        let mut seen: Vec<&str> = Vec::new();
        for (name, score) in &labels {
            assert!(
                TOXIC_CONTENT_TYPE_VALUES.contains(&name.as_str()),
                "case {case:?}: {name:?} is not a modelled ToxicContentType value"
            );
            assert!(
                !seen.contains(&name.as_str()),
                "case {case:?}: {name:?} is reported twice in {labels:?}"
            );
            seen.push(name.as_str());
            assert!(
                *score > 0.0 && *score <= 1.0,
                "case {case:?}: expected a Score in (0, 1] for {name:?}, got {score}"
            );
        }

        assert!(
            (0.0..=1.0).contains(&toxicity),
            "case {case:?}: expected a Toxicity in [0, 1], got {toxicity}"
        );

        // `Toxicity` is the confidence that the string carries toxic content of ANY
        // type: the complement of the product of the per-type complements.
        let expected_toxicity =
            1.0 - labels.iter().map(|(_, score)| 1.0 - score).product::<f64>();
        assert!(
            (toxicity - expected_toxicity).abs() < 1e-6,
            "case {case:?}: expected Toxicity {expected_toxicity} from the labels \
             {labels:?}, got {toxicity}"
        );

        if let Some(expected_label) = expected_label {
            assert!(
                labels.iter().any(|(name, _)| name == expected_label),
                "case {case:?}: expected a {expected_label:?} label, got {labels:?}"
            );
        }

        // A string the engine places content in carries at least one label and a
        // toxicity above zero; one it places nothing in carries neither.
        assert_eq!(
            !labels.is_empty(),
            *toxic,
            "case {case:?}: expected toxic={toxic}, got labels {labels:?}"
        );
        assert_eq!(
            toxicity > 0.0,
            *toxic,
            "case {case:?}: expected toxic={toxic}, got Toxicity {toxicity}"
        );
    }
}

#[tokio::test]
async fn a_string_the_engine_places_toxic_content_in_scores_above_one_it_does_not() {
    let comprehend = Comprehend::new();

    for toxic_row in toxic_rows() {
        let toxic_case = toxic_row.case;
        let toxic_output = comprehend
            .ok("DetectToxicContent", &request(&[toxic_row.text]))
            .await;
        let toxic_entry = &toxic_output["ResultList"][0];
        let toxic_score = toxicity_of(toxic_case, toxic_entry);
        assert!(
            toxic_score > 0.0,
            "case {toxic_case:?}: expected a Toxicity above zero, got {toxic_score} \
             with {toxic_entry}"
        );
        assert!(
            !labels_of(toxic_case, toxic_entry).is_empty(),
            "case {toxic_case:?}: expected at least one label, got {toxic_entry}"
        );

        for benign_row in benign_rows() {
            let benign_case = benign_row.case;
            let benign_output = comprehend
                .ok("DetectToxicContent", &request(&[benign_row.text]))
                .await;
            let benign_entry = &benign_output["ResultList"][0];
            let benign_score = toxicity_of(benign_case, benign_entry);

            assert_eq!(
                benign_score, 0.0,
                "case {benign_case:?}: the engine places no toxic content, so the \
                 Toxicity is zero, got {benign_score} with {benign_entry}"
            );
            assert!(
                labels_of(benign_case, benign_entry).is_empty(),
                "case {benign_case:?}: the engine places no toxic content, so no type \
                 may score above zero, got {benign_entry}"
            );
            assert!(
                toxic_score > benign_score,
                "case {toxic_case:?} vs {benign_case:?}: expected the toxic string to \
                 score higher, got {toxic_score} and {benign_score}"
            );
        }
    }
}

#[tokio::test]
async fn one_call_scores_each_of_its_strings_independently_and_in_order() {
    let comprehend = Comprehend::new();

    // Every list size the operation accepts, from the modelled floor of 1 to the
    // documented ceiling of 10, built by cycling the table so each size mixes toxic
    // and benign strings in a different arrangement.
    for size in 1..=10usize {
        let texts: Vec<&str> = (0..size)
            .map(|index| SEGMENTS[(index * 3 + size) % SEGMENTS.len()].text)
            .collect();
        let case = format!("{size} segments");

        let batched = comprehend.ok("DetectToxicContent", &request(&texts)).await;
        let results = batched["ResultList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResultList, got {batched}"))
            .clone();
        assert_eq!(
            results.len(),
            size,
            "case {case:?}: the results list matches the size of the input list, got \
             {batched}"
        );

        for (index, text) in texts.iter().enumerate() {
            let alone = comprehend.ok("DetectToxicContent", &request(&[text])).await;
            assert_eq!(
                results[index], alone["ResultList"][0],
                "case {case:?}: entry {index} must be the verdict for its own string, \
                 got {} for the string scored alone as {}",
                results[index], alone["ResultList"][0]
            );
        }
    }
}

#[tokio::test]
async fn repeating_a_call_returns_the_same_scores() {
    let comprehend = Comprehend::new();
    let body = request(&[SEGMENTS[0].text, SEGMENTS[6].text]);

    let first = comprehend.ok("DetectToxicContent", &body).await;
    let second = comprehend.ok("DetectToxicContent", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical call\": expected identical scores, got {first} then \
         {second}"
    );
}

#[tokio::test]
async fn a_language_the_engine_cannot_read_is_refused() {
    let comprehend = Comprehend::new();

    for code in LANGUAGE_CODE_VALUES {
        let body = json!({
            "LanguageCode": code,
            "TextSegments": [{ "Text": SEGMENTS[0].text }],
        });
        let (status, response) = comprehend.call("DetectToxicContent", &body).await;

        // The modelled member documentation says English is the only supported
        // language, and the engine's word lists are English.
        if *code == "en" {
            assert_eq!(
                status, 200,
                "case {code:?}: the engine reads English, expected 200, got {status}: \
                 {response}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {code:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "UnsupportedLanguageException",
                "case {code:?}: expected UnsupportedLanguageException, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_language_code_outside_the_modelled_enum_is_refused() {
    let comprehend = Comprehend::new();

    for code in ["xx", "EN", "english", ""] {
        let body = json!({
            "LanguageCode": code,
            "TextSegments": [{ "Text": SEGMENTS[0].text }],
        });
        let (status, response) = comprehend.call("DetectToxicContent", &body).await;
        assert_eq!(
            status, 400,
            "case {code:?}: expected 400 for a value outside the LanguageCode enum, got \
             {status}: {response}"
        );
    }
}

/// The per-string byte quota this operation enforces.
///
/// Source: `crate::state::TOXIC_CONTENT_SEGMENT_QUOTA_BYTES`, this service's own
/// quota table, which reads the modelled `DetectToxicContentRequest$TextSegments`
/// documentation ("A list of up to 10 text strings. Each string has a maximum size
/// of 1 KB, and the maximum size of the list is 10 KB") as the same decimal count of
/// bytes the service's other real-time quotas are read as — the reading under which
/// ten strings of this quota each are exactly the documented 10 KB ceiling on the
/// whole list.
const MAX_SEGMENT_BYTES: usize = 1_000;

#[tokio::test]
async fn a_string_at_the_documented_size_ceiling_is_accepted_and_one_byte_past_it_is_not() {
    let comprehend = Comprehend::new();

    // The modelled `@length` floor of 1, well inside the quota, the last accepted
    // length, the first refused one, the length a quota misread as a kibibyte would
    // still accept, and far past it.
    for size in [
        1usize,
        MAX_SEGMENT_BYTES / 2,
        MAX_SEGMENT_BYTES - 1,
        MAX_SEGMENT_BYTES,
        MAX_SEGMENT_BYTES + 1,
        1_024,
        MAX_SEGMENT_BYTES * 4,
    ] {
        let (status, response) = comprehend
            .call("DetectToxicContent", &request(&["a".repeat(size).as_str()]))
            .await;

        if size > MAX_SEGMENT_BYTES {
            assert_eq!(
                status, 400,
                "case \"{size} byte string\": expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "TextSizeLimitExceededException",
                "case \"{size} byte string\": expected TextSizeLimitExceededException, \
                 got {response}"
            );
            assert_eq!(
                error_message(&response),
                expected_text_size_message(MAX_SEGMENT_BYTES, size),
                "case \"{size} byte string\": the refusal must report the quota and the \
                 submitted byte length, got {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case \"{size} byte string\": expected 200, got {status}: {response}"
            );
        }
    }
}

#[tokio::test]
async fn the_per_string_quota_counts_utf8_bytes_and_not_characters() {
    let comprehend = Comprehend::new();

    // Characters wider than one byte, with their UTF-8 width: `quota / width` of them
    // sit exactly ON the quota, and one more character is `width` bytes past it —
    // while the CHARACTER count is still far below the quota.
    for (case, character, width) in [("two-byte", "é", 2), ("four-byte", "😀", 4)] {
        let fitting = MAX_SEGMENT_BYTES / width;
        let (status, response) = comprehend
            .call(
                "DetectToxicContent",
                &request(&[character.repeat(fitting).as_str()]),
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?} with {fitting} characters ({} bytes): expected 200, got \
             {status}: {response}",
            fitting * width
        );

        let over_bytes = (fitting + 1) * width;
        let (status, response) = comprehend
            .call(
                "DetectToxicContent",
                &request(&[character.repeat(fitting + 1).as_str()]),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} with {over_bytes} bytes: expected 400 because the quota \
             counts bytes, got {status}: {response}"
        );
        assert_eq!(
            error_message(&response),
            expected_text_size_message(MAX_SEGMENT_BYTES, over_bytes),
            "case {case:?} with {over_bytes} bytes: the refusal must report the \
             submitted UTF-8 byte length, got {response}"
        );
    }
}

#[tokio::test]
async fn one_oversized_string_refuses_the_whole_call_wherever_it_sits_in_the_list() {
    let comprehend = Comprehend::new();

    // The refusal is a property of the list, not of its first entry: an oversized
    // string at any position refuses the call, and a list of strings that all fit is
    // accepted however many of them there are.
    for position in 0..4usize {
        let oversized = "a".repeat(MAX_SEGMENT_BYTES + 1);
        let texts: Vec<&str> = (0..4)
            .map(|index| {
                if index == position {
                    oversized.as_str()
                } else {
                    SEGMENTS[index % SEGMENTS.len()].text
                }
            })
            .collect();
        let (status, response) = comprehend
            .call("DetectToxicContent", &request(&texts))
            .await;
        assert_eq!(
            status, 400,
            "case \"oversized at {position}\": expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "TextSizeLimitExceededException",
            "case \"oversized at {position}\": expected TextSizeLimitExceededException, \
             got {response}"
        );
    }
}

/// The list-length ceiling the modelled `TextSegments` documentation states: "A
/// list of up to 10 text strings."
const MAX_SEGMENTS: usize = 10;

#[tokio::test]
async fn a_list_at_the_documented_length_ceiling_is_accepted_and_one_string_past_it_is_not() {
    let comprehend = Comprehend::new();

    for count in [1usize, MAX_SEGMENTS, MAX_SEGMENTS + 1] {
        let texts: Vec<&str> = (0..count).map(|_| SEGMENTS[0].text).collect();
        let (status, response) = comprehend.call("DetectToxicContent", &request(&texts)).await;

        if count > MAX_SEGMENTS {
            assert_eq!(
                status, 400,
                "case \"{count} strings\": expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidRequestException",
                "case \"{count} strings\": expected InvalidRequestException, got {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case \"{count} strings\": expected 200, got {status}: {response}"
            );
        }
    }
}

#[tokio::test]
async fn an_empty_list_or_an_empty_string_is_refused_by_the_modelled_length_floors() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        (
            "empty TextSegments",
            json!({ "LanguageCode": "en", "TextSegments": [] }),
        ),
        (
            "absent TextSegments",
            json!({ "LanguageCode": "en" }),
        ),
        (
            "empty Text",
            json!({ "LanguageCode": "en", "TextSegments": [{ "Text": "" }] }),
        ),
        (
            "absent Text",
            json!({ "LanguageCode": "en", "TextSegments": [{}] }),
        ),
        (
            "absent LanguageCode",
            json!({ "TextSegments": [{ "Text": "hello" }] }),
        ),
    ] {
        let (status, response) = comprehend.call("DetectToxicContent", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
    }
}
