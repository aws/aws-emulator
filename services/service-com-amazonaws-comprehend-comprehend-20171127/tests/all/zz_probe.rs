// SPDX-License-Identifier: CC0-1.0

//! One document written from scratch for the SINGLE-DOCUMENT inference family, and a
//! dump of each analyser's answer for it, pinned as a baseline.
//!
//! `single_document_conventions.rs` asserts the rule each analyser follows, one rule at
//! a time; this one pins what those rules add up to on ONE document read by all six, so
//! a change to one rule that quietly changes another analyser's answer — a tag that
//! moves a name's boundary, a name span that swallows the word in front of it, a type
//! that retypes a targeted mention — fails here rather than silently. It is the
//! single-document counterpart of `zz_scratch.rs`, which pins the same thing for the
//! batch analysers.
//!
//! Only the DISCRETE parts of each answer are pinned: the tags, the spans and their
//! types, the verdict, the label set. The confidences beside them follow each
//! analyser's evidence and are asserted as the rules they obey, in the suite above,
//! rather than pinned as literals here.

use serde_json::json;

use crate::comprehend_client::Comprehend;
use crate::toxic_content::PLACEMENT_FLOOR;

/// One document carrying a company name followed by a product of its own, a nominal
/// compound, a relative clause, a personal name standing in running text after an
/// ordinary word, a postal address, an e-mail address and a date.
const DOCUMENT: &str = "Amazon Comprehend is a natural language processing service that \
                        uses machine learning to find insights and relationships in \
                        text. Contact John Smith at 123 Main Street, Seattle, WA 98101 \
                        or john.smith@example.com for more details today.";

/// An English single-document request carrying [`DOCUMENT`].
fn request() -> serde_json::Value {
    json!({ "Text": DOCUMENT, "LanguageCode": "en" })
}

/// Every entity of [`DOCUMENT`], as `(Text, Type)`.
const ENTITIES: &[(&str, &str)] = &[
    ("Amazon", "ORGANIZATION"),
    ("Comprehend", "COMMERCIAL_ITEM"),
    ("John Smith", "PERSON"),
    ("123", "QUANTITY"),
    ("Main Street", "LOCATION"),
    ("Seattle", "LOCATION"),
    ("WA", "OTHER"),
    ("98101", "QUANTITY"),
    ("john.smith@example.com", "OTHER"),
    ("today", "DATE"),
];

/// Every PII entity of [`DOCUMENT`], as `(Type, the characters its offsets delimit)`.
const PII_ENTITIES: &[(&str, &str)] = &[
    ("NAME", "John Smith"),
    ("ADDRESS", "123 Main Street, Seattle, WA 98101"),
    ("EMAIL", "john.smith@example.com"),
];

/// Every targeted-sentiment mention of [`DOCUMENT`], as `(Text, Type)`.
///
/// Not the same spans as [`ENTITIES`], and that is the point: the entity recogniser
/// answers which CHARACTERS it typed and types the company name and the product name
/// beside it separately, while a targeted-sentiment mention is the whole name the
/// document writes, typed by the part that says what is named.
///
/// The predicate of the first clause (`is a natural language processing service`) is
/// no mention: it says what KIND of thing the document's subject is rather than naming
/// a further thing, so no sentiment is directed at it.
const TARGETS: &[(&str, &str)] = &[
    ("Amazon Comprehend", "ORGANIZATION"),
    ("John Smith", "PERSON"),
    ("123", "QUANTITY"),
    ("Main Street", "LOCATION"),
    ("Seattle", "LOCATION"),
    ("WA", "OTHER"),
    ("98101", "QUANTITY"),
    ("john.smith@example.com", "OTHER"),
    ("today", "DATE"),
];

/// The characters of [`DOCUMENT`] in `[begin, end)`, read off a response's offsets.
fn slice(begin: usize, end: usize) -> String {
    DOCUMENT.chars().skip(begin).take(end - begin).collect()
}

#[tokio::test]
async fn the_whole_answer_for_one_document_is_pinned_across_the_six_analysers() {
    let comprehend = Comprehend::new();

    let entities = comprehend.ok("DetectEntities", &request()).await;
    let read: Vec<(String, String)> = entities["Entities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entity| {
            (
                entity["Text"].as_str().unwrap_or_default().to_string(),
                entity["Type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = ENTITIES
        .iter()
        .map(|(text, entity_type)| ((*text).to_string(), (*entity_type).to_string()))
        .collect();
    assert_eq!(
        read, expected,
        "case \"entity baseline\": expected {expected:?}, got {read:?}"
    );

    let pii = comprehend.ok("DetectPiiEntities", &request()).await;
    let placed: Vec<(String, String)> = pii["Entities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entity| {
            let begin = entity["BeginOffset"].as_u64().unwrap_or_default() as usize;
            let end = entity["EndOffset"].as_u64().unwrap_or_default() as usize;
            (
                entity["Type"].as_str().unwrap_or_default().to_string(),
                slice(begin, end),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = PII_ENTITIES
        .iter()
        .map(|(entity_type, value)| ((*entity_type).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        placed, expected,
        "case \"pii baseline\": expected {expected:?}, got {placed:?}"
    );

    let labelled = comprehend.ok("ContainsPiiEntities", &request()).await;
    let mut labels: Vec<String> = labelled["Labels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|label| label["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    labels.sort();
    let mut expected: Vec<String> = PII_ENTITIES
        .iter()
        .map(|(entity_type, _)| (*entity_type).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        labels, expected,
        "case \"pii label baseline\": expected {expected:?}, got {labels:?}"
    );

    let targeted = comprehend.ok("DetectTargetedSentiment", &request()).await;
    let mut mentions: Vec<(usize, String, String)> = Vec::new();
    for entity in targeted["Entities"].as_array().cloned().unwrap_or_default() {
        for mention in entity["Mentions"].as_array().cloned().unwrap_or_default() {
            mentions.push((
                mention["BeginOffset"].as_u64().unwrap_or_default() as usize,
                mention["Text"].as_str().unwrap_or_default().to_string(),
                mention["Type"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    mentions.sort_by_key(|(begin, _, _)| *begin);
    let read: Vec<(String, String)> = mentions
        .into_iter()
        .map(|(_, text, entity_type)| (text, entity_type))
        .collect();
    let expected: Vec<(String, String)> = TARGETS
        .iter()
        .map(|(text, entity_type)| ((*text).to_string(), (*entity_type).to_string()))
        .collect();
    assert_eq!(
        read, expected,
        "case \"targeted-sentiment baseline\": expected {expected:?}, got {read:?}"
    );

    let sentiment = comprehend.ok("DetectSentiment", &request()).await;
    assert_eq!(
        sentiment["Sentiment"].as_str(),
        Some("NEUTRAL"),
        "case \"sentiment baseline\": a document of plain fact is NEUTRAL, got {sentiment}"
    );

    let toxicity = comprehend
        .ok(
            "DetectToxicContent",
            &json!({ "LanguageCode": "en", "TextSegments": [{ "Text": DOCUMENT }] }),
        )
        .await;
    let scores: Vec<f64> = toxicity["ResultList"][0]["Labels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|label| label["Score"].as_f64().unwrap_or(f64::NAN))
        .collect();
    // The document carries no toxic content, so no content type is PLACED: each keeps
    // only the room its OWN detectors' observations leave, which is not one value for
    // the whole vector — the engine's four categories are read by one detector over the
    // segment and its words, while each conduct category is read by its own published
    // forms over each sentence — and every one of them is strictly inside (0, the gate
    // a moderation client reads the vector against).
    assert_eq!(
        scores.len(),
        7,
        "case \"toxicity baseline\": expected one score per modelled content type, got \
         {scores:?}"
    );
    for score in &scores {
        assert!(
            *score > 0.0 && *score < PLACEMENT_FLOOR,
            "case \"toxicity baseline\": the document carries no toxic content, so every \
             content type keeps only the residual a finite reading leaves, got {scores:?}"
        );
    }
    let distinct: std::collections::BTreeSet<String> =
        scores.iter().map(|score| format!("{score:.12}")).collect();
    assert!(
        distinct.len() > 1,
        "case \"toxicity baseline\": the vector reports one room per content type, \
         measured from what that type's own detectors read, so it is not a single \
         literal repeated, got {scores:?}"
    );
    let overall = toxicity["ResultList"][0]["Toxicity"]
        .as_f64()
        .unwrap_or(f64::NAN);
    // With nothing placed, the overall Toxicity is what the whole vector's rooms
    // combine to: above every one of them, and a copy of none.
    let combined = 1.0 - scores.iter().map(|score| 1.0 - score).product::<f64>();
    assert!(
        (overall - combined).abs() < 1e-6 && scores.iter().all(|score| overall > *score),
        "case \"toxicity baseline\": with nothing placed, the overall Toxicity is the \
         vector's rooms combined ({combined}), got {overall} with {scores:?}"
    );

    let syntax = comprehend.ok("DetectSyntax", &request()).await;
    let tokens = syntax["SyntaxTokens"].as_array().cloned().unwrap_or_default();
    // Every token's Text is the characters its own offsets delimit, and its tag is the
    // reading of the position it stands in.
    for (position, token) in tokens.iter().enumerate() {
        let begin = token["BeginOffset"].as_u64().unwrap_or_default() as usize;
        let end = token["EndOffset"].as_u64().unwrap_or_default() as usize;
        assert_eq!(
            token["Text"].as_str(),
            Some(slice(begin, end).as_str()),
            "case \"syntax baseline\": token {position} must carry the characters its \
             offsets delimit, got {token}"
        );
        assert_eq!(
            token["TokenId"].as_u64(),
            Some(position as u64 + 1),
            "case \"syntax baseline\": TokenId is the token's 1-based position, got {token}"
        );
    }
    let tagged: Vec<(String, String)> = tokens
        .iter()
        .map(|token| {
            (
                token["Text"].as_str().unwrap_or_default().to_string(),
                token["PartOfSpeech"]["Tag"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = TOKENS
        .iter()
        .map(|(text, tag)| ((*text).to_string(), (*tag).to_string()))
        .collect();
    assert_eq!(
        tagged, expected,
        "case \"syntax baseline\": expected {expected:?}, got {tagged:?}"
    );
}

/// Every syntax token of [`DOCUMENT`], as `(Text, Tag)`.
const TOKENS: &[(&str, &str)] = &[
    ("Amazon", "PROPN"),
    ("Comprehend", "PROPN"),
    ("is", "VERB"),
    ("a", "DET"),
    ("natural", "ADJ"),
    ("language", "NOUN"),
    ("processing", "NOUN"),
    ("service", "NOUN"),
    ("that", "PRON"),
    ("uses", "VERB"),
    ("machine", "NOUN"),
    ("learning", "NOUN"),
    ("to", "ADP"),
    ("find", "VERB"),
    ("insights", "NOUN"),
    ("and", "CCONJ"),
    ("relationships", "NOUN"),
    ("in", "ADP"),
    ("text", "NOUN"),
    (".", "PUNCT"),
    ("Contact", "NOUN"),
    ("John", "PROPN"),
    ("Smith", "PROPN"),
    ("at", "ADP"),
    ("123", "NUM"),
    ("Main", "PROPN"),
    ("Street", "PROPN"),
    (",", "PUNCT"),
    ("Seattle", "PROPN"),
    (",", "PUNCT"),
    ("WA", "PROPN"),
    ("98101", "NUM"),
    ("or", "CCONJ"),
    // A mail address is one word of the document, and no word class claims it.
    ("john.smith@example.com", "O"),
    ("for", "ADP"),
    ("more", "NOUN"),
    ("details", "NOUN"),
    ("today", "ADV"),
    (".", "PUNCT"),
];
