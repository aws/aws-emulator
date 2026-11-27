// SPDX-License-Identifier: CC0-1.0

//! `DetectTargetedSentiment`: the entities a document mentions, each mention with
//! the offsets it occupies in the document and the sentiment of the clause it sits
//! in.
//!
//! The rule these suites assert, over a table of documents rather than over one
//! recorded request: a reported mention is the document's OWN substring at
//! `[BeginOffset, EndOffset)`, counted in characters, every mention of one entity
//! reports one type with confidences in (0, 1] and a four-way sentiment score that
//! sums to 1, and the answer is a function of the document alone — so repeating a
//! call reports the same mentions at the same offsets.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    LANGUAGE_CODE_VALUES, SENTIMENT_TYPE_VALUES, TARGETED_SENTIMENT_ENTITY_TYPE_VALUES,
};

use crate::comprehend_client::{error_code, Comprehend};

/// Documents spanning what the rule generalises over: how many entities a document
/// mentions, how often it mentions one, which rule places the mention (a name, a
/// legal-entity suffix, an honorific, a calendar name, a numeral, a determiner-headed
/// noun phrase, a pronoun), how many sentences it has, which script it is written
/// in, and documents with no mention at all.
const DOCUMENTS: &[(&str, &str)] = &[
    ("one proper noun", "Acme shipped the parcel on time."),
    (
        "one name mentioned twice",
        "Amazon refunded the fee. Amazon apologised for the delay.",
    ),
    (
        "a name and a pronoun",
        "Maria rewrote the parser. She was brilliant about it.",
    ),
    (
        "legal-entity suffix",
        "Initech Inc. shipped a terrible release.",
    ),
    (
        "honorific and name",
        "Dr Okonkwo explained the diagnosis very clearly.",
    ),
    (
        "calendar name and numeral",
        "The invoice arrived in March 2024 and the total was 1,250.",
    ),
    (
        "contrasting clauses",
        "The food was great but the service was terrible.",
    ),
    (
        "multi-byte characters before the mention",
        "The café was lovely and Zoë was wonderful to us.",
    ),
    (
        "four sentences",
        "The driver was late. The parcel was damaged. Acme refunded us. We were still unhappy.",
    ),
    (
        "first and second person",
        "I loved my new phone but you hated yours.",
    ),
    ("no words at all", "!!! ... ???"),
    ("one word", "Wonderful!"),
    ("one character", "a"),
];

/// Assert every invariant a `DetectTargetedSentiment` response carries for `text`.
fn assert_invariants(case: &str, text: &str, output: &Value) {
    let characters: Vec<char> = text.chars().collect();
    let entities = output["Entities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Entities array, got {output}"))
        .clone();

    // Every mention of every entity, so no two mentions of the document may claim
    // overlapping text.
    let mut spans: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut previous_entity_start = -1i64;

    for (entity_index, entity) in entities.iter().enumerate() {
        let mentions = entity["Mentions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: entity {entity_index} has no Mentions array: {entity}")
            })
            .clone();
        assert!(
            !mentions.is_empty(),
            "case {case:?}: entity {entity_index} reports no mention at all: {entity}"
        );

        let mut previous_begin = -1i64;
        let mut types: BTreeSet<&str> = BTreeSet::new();

        for (mention_index, mention) in mentions.iter().enumerate() {
            let at = format!("case {case:?}: entity {entity_index} mention {mention_index}");
            let begin = mention["BeginOffset"].as_i64().unwrap_or(-1);
            let end = mention["EndOffset"].as_i64().unwrap_or(-1);
            assert!(
                begin >= 0 && end > begin && end as usize <= characters.len(),
                "{at}: expected 0 <= BeginOffset < EndOffset <= {} characters, got \
                 [{begin}, {end}) in {mention}",
                characters.len()
            );

            let slice: String = characters[begin as usize..end as usize].iter().collect();
            let reported = mention["Text"].as_str().unwrap_or_default();
            assert_eq!(
                reported, slice,
                "{at}: Text is not the document's substring at [{begin}, {end}), which is \
                 {slice:?}"
            );

            assert!(
                begin > previous_begin,
                "{at}: mentions must ascend by BeginOffset, got {begin} after {previous_begin} \
                 in {entity}"
            );
            previous_begin = begin;

            assert!(
                spans
                    .iter()
                    .all(|(other_begin, other_end)| end <= *other_begin || begin >= *other_end),
                "{at}: [{begin}, {end}) overlaps an already reported mention in {spans:?}"
            );
            spans.insert((begin, end));

            let kind = mention["Type"].as_str().unwrap_or_default();
            assert!(
                TARGETED_SENTIMENT_ENTITY_TYPE_VALUES.contains(&kind),
                "{at}: {kind:?} is not a modelled TargetedSentimentEntityType value"
            );
            types.insert(mention["Type"].as_str().unwrap_or_default());

            for label in ["Score", "GroupScore"] {
                let score = mention[label].as_f64().unwrap_or(f64::NAN);
                assert!(
                    score > 0.0 && score <= 1.0,
                    "{at}: expected {label} in (0, 1], got {score} in {mention}"
                );
            }

            let sentiment = mention["MentionSentiment"]["Sentiment"]
                .as_str()
                .unwrap_or_default();
            assert!(
                SENTIMENT_TYPE_VALUES.contains(&sentiment),
                "{at}: {sentiment:?} is not a modelled SentimentType value"
            );
            let scores = &mention["MentionSentiment"]["SentimentScore"];
            let by_label: Vec<(&str, f64)> = ["Positive", "Negative", "Neutral", "Mixed"]
                .into_iter()
                .map(|label| {
                    let score = scores[label]
                        .as_f64()
                        .unwrap_or_else(|| panic!("{at}: expected SentimentScore.{label} in {scores}"));
                    (label, score)
                })
                .collect();
            let total: f64 = by_label.iter().map(|(_, score)| score).sum();
            assert!(
                (total - 1.0).abs() < 1e-5,
                "{at}: expected the four sentiment scores to sum to 1.0, got {total} from {scores}"
            );
            // The reported sentiment is the arg-max of the four it reports, with the
            // tie-break the scorer documents: the first of (Positive, Negative,
            // Neutral, Mixed), which is the order `by_label` is built in.
            let winner = by_label
                .iter()
                .copied()
                .reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best })
                .map(|(label, _)| label.to_uppercase())
                .unwrap_or_default();
            assert_eq!(
                winner, sentiment,
                "{at}: Sentiment is not the arg-max of SentimentScore, got {sentiment:?} \
                 with {scores}"
            );
        }

        assert_eq!(
            types.len(),
            1,
            "case {case:?}: entity {entity_index} reports more than one Type for one \
             entity: {types:?}"
        );

        let descriptive = entity["DescriptiveMentionIndex"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: entity {entity_index} has no DescriptiveMentionIndex: \
                     {entity}"
                )
            })
            .clone();
        assert!(
            !descriptive.is_empty(),
            "case {case:?}: entity {entity_index} names no descriptive mention: {entity}"
        );
        for index in &descriptive {
            let index = index.as_i64().unwrap_or(-1);
            assert!(
                index >= 0 && (index as usize) < mentions.len(),
                "case {case:?}: entity {entity_index} has DescriptiveMentionIndex {index}, \
                 which is not an index into its {} mentions",
                mentions.len()
            );
        }

        let first = mentions[0]["BeginOffset"].as_i64().unwrap_or(-1);
        assert!(
            first > previous_entity_start,
            "case {case:?}: entities must be ordered by the document's first mention of \
             them, got entity {entity_index} starting at {first} after {previous_entity_start}"
        );
        previous_entity_start = first;
    }
}

/// Analyse `text` as English, expecting success.
async fn analyse(comprehend: &Comprehend, text: &str) -> Value {
    comprehend
        .ok(
            "DetectTargetedSentiment",
            &json!({ "Text": text, "LanguageCode": "en" }),
        )
        .await
}

/// Every mention reported for an entity, as `(Text, Type, BeginOffset)` triples.
fn mentions_of(entity: &Value) -> Vec<(String, String, i64)> {
    entity["Mentions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|mention| {
            (
                mention["Text"].as_str().unwrap_or_default().to_string(),
                mention["Type"].as_str().unwrap_or_default().to_string(),
                mention["BeginOffset"].as_i64().unwrap_or(-1),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_mention_is_the_documents_own_substring_at_its_offsets() {
    let comprehend = Comprehend::new();

    for (case, text) in DOCUMENTS {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);
    }
}

/// A systematic sweep rather than one example: every combination of a prefix that
/// pushes character offsets away from byte offsets, a mention form each recognition
/// rule places, and a sentiment context, is a document whose invariants must hold.
#[tokio::test]
async fn the_invariants_hold_over_every_prefix_form_and_context() {
    let comprehend = Comprehend::new();

    let prefixes = ["", "Привет. ", "Héllo — ", "😀 "];
    let forms = [
        "Acme",
        "Acme Inc.",
        "Dr Okonkwo",
        "the parcel",
        "March 2024",
        "1,250",
        "it",
        "my phone",
    ];
    let contexts = [
        "{} was wonderful.",
        "{} was terrible.",
        "Nobody mentioned {} at all.",
        "I liked {} but my colleague hated {}.",
    ];

    for prefix in prefixes {
        for form in forms {
            for context in contexts {
                let text = format!("{prefix}{}", context.replace("{}", form));
                let case = format!("prefix {prefix:?} + form {form:?} + context {context:?}");
                let output = analyse(&comprehend, &text).await;
                assert_invariants(&case, &text, &output);

                // Each `forms` row is a mention form one of the recognition rules
                // must place, so the sweep cannot pass vacuously on an empty
                // `Entities`: some reported mention has to come out of that form.
                let reported: Vec<String> = output["Entities"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .flat_map(mentions_of)
                    .map(|(text, _, _)| text)
                    .collect();
                assert!(
                    reported
                        .iter()
                        .any(|mention| form.contains(mention.as_str())),
                    "case {case:?}: expected a mention placed in {form:?}, got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn offsets_count_characters_rather_than_bytes() {
    let comprehend = Comprehend::new();

    // Each row carries multi-byte characters BEFORE the mention it names, so the
    // mention's character offset is strictly smaller than its byte offset: a
    // response reporting byte positions cannot pass.
    for (case, text, mention) in [
        ("accents", "The café was lovely and Zoë was wonderful.", "Zoë"),
        ("cyrillic", "Привет! Acme was wonderful.", "Acme"),
        ("emoji", "😀😀😀 Acme was wonderful.", "Acme"),
        ("ideographs", "我们 said Acme was wonderful.", "Acme"),
    ] {
        let text = text.to_string();
        let output = analyse(&comprehend, &text).await;
        assert_invariants(case, &text, &output);

        let expected_characters = text
            .chars()
            .collect::<Vec<char>>()
            .windows(mention.chars().count())
            .position(|window| window.iter().collect::<String>() == mention)
            .unwrap_or_else(|| panic!("case {case:?}: {mention:?} is not in {text:?}"))
            as i64;
        let byte_offset = text
            .find(mention)
            .unwrap_or_else(|| panic!("case {case:?}: {mention:?} is not in {text:?}"))
            as i64;
        assert!(
            byte_offset > expected_characters,
            "case {case:?}: the row must place multi-byte characters before {mention:?}, \
             but its byte offset {byte_offset} is not past its character offset \
             {expected_characters}"
        );

        let found = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(mentions_of)
            .find(|(text, _, _)| text == mention);
        let (_, _, begin) = found.unwrap_or_else(|| {
            panic!("case {case:?}: no mention of {mention:?} in {output}")
        });
        assert_eq!(
            begin, expected_characters,
            "case {case:?}: expected the character offset {expected_characters} for \
             {mention:?}, got {begin} (its byte offset is {byte_offset})"
        );
    }
}

#[tokio::test]
async fn mentions_of_one_entity_are_grouped_into_one_entity() {
    let comprehend = Comprehend::new();

    // Each row: the document, and the mention texts that must land in ONE entity.
    for (case, text, grouped) in [
        (
            "a repeated name",
            "Amazon refunded the fee. Amazon apologised for the delay.",
            vec!["Amazon", "Amazon"],
        ),
        (
            "a name and the pronoun referring to it",
            "Maria rewrote the parser. She was brilliant.",
            vec!["Maria", "She"],
        ),
        (
            "a name repeated after the pronoun that typed it a person's",
            "Maria rewrote the parser. She was brilliant. Maria left early.",
            vec!["Maria", "She", "Maria"],
        ),
        (
            "a full name and its surname",
            "Alice Lennox joined us. Lennox was wonderful.",
            vec!["Alice Lennox", "Lennox"],
        ),
        (
            "every first-person pronoun",
            "I loved it. I said so to the team and I meant it.",
            vec!["I", "I", "I"],
        ),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let entities = output["Entities"].as_array().cloned().unwrap_or_default();
        let found = entities
            .iter()
            .map(|entity| {
                mentions_of(entity)
                    .into_iter()
                    .map(|(text, _, _)| text)
                    .collect::<Vec<String>>()
            })
            .find(|texts| *texts == grouped);
        assert!(
            found.is_some(),
            "case {case:?}: expected one entity mentioned as {grouped:?}, got {entities:?}"
        );
    }
}

#[tokio::test]
async fn a_pronoun_reports_the_type_of_what_it_refers_to() {
    let comprehend = Comprehend::new();

    for (case, text, mention, expected) in [
        (
            "a personal pronoun types its antecedent a person",
            "Maria rewrote the parser. She was brilliant.",
            "She",
            "PERSON",
        ),
        (
            "a first-person pronoun is a person",
            "I was unhappy with the delay.",
            "I",
            "PERSON",
        ),
        (
            "an honorific is a personal title",
            "Dr Okonkwo explained the diagnosis.",
            "Dr",
            "PERSONAL_TITLE",
        ),
        (
            "a legal-entity suffix makes an organization",
            "Initech Inc. shipped a release.",
            "Initech Inc",
            "ORGANIZATION",
        ),
        (
            "a calendar name makes a date",
            "The invoice arrived in March 2024.",
            "March 2024",
            "DATE",
        ),
        (
            "a numeral is a quantity",
            "The total was 1,250 last week.",
            "1,250",
            "QUANTITY",
        ),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let reported = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(mentions_of)
            .find(|(found, _, _)| found == mention)
            .map(|(_, kind, _)| kind);
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected {mention:?} to be typed {expected:?}, got {output}"
        );
    }
}

#[tokio::test]
async fn each_mention_is_scored_on_the_clause_it_sits_in() {
    let comprehend = Comprehend::new();

    // One document carries two mentions with OPPOSITE polarity, so a response
    // scoring the whole document once per mention cannot pass. The rows differ in
    // which clause carries which polarity and in how the clauses are separated.
    // The assertion is on the DIRECTION of the four confidences — which of
    // `Positive` and `Negative` the clause puts the weight on — because the label is
    // their arg-max and `NEUTRAL` can out-score a lone sentiment word in a short
    // clause.
    for (case, text, expected) in [
        (
            "contrast with but",
            "The food was great but the service was terrible.",
            vec![("food", "positive"), ("service", "negative")],
        ),
        (
            "contrast the other way round",
            "Acme was terrible but Globex was wonderful.",
            vec![("Acme", "negative"), ("Globex", "positive")],
        ),
        (
            "two sentences",
            "Acme was wonderful. Globex was terrible.",
            vec![("Acme", "positive"), ("Globex", "negative")],
        ),
        (
            "clauses separated by a semicolon",
            "Acme was wonderful; Globex was terrible.",
            vec![("Acme", "positive"), ("Globex", "negative")],
        ),
        (
            "three clauses",
            "Acme was wonderful but Globex was terrible; Initech was excellent.",
            vec![
                ("Acme", "positive"),
                ("Globex", "negative"),
                ("Initech", "positive"),
            ],
        ),
        (
            "no sentiment anywhere",
            "The invoice arrived on Tuesday.",
            vec![("invoice", "neutral")],
        ),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let reported: Vec<(String, Value)> = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(|entity| entity["Mentions"].as_array().cloned().unwrap_or_default())
            .map(|mention| {
                (
                    mention["Text"].as_str().unwrap_or_default().to_string(),
                    mention["MentionSentiment"].clone(),
                )
            })
            .collect();

        for (mention, direction) in expected {
            let sentiment = reported
                .iter()
                .find(|(text, _)| text == mention)
                .map(|(_, sentiment)| sentiment.clone())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no mention of {mention:?} in {reported:?}")
                });
            let positive = sentiment["SentimentScore"]["Positive"]
                .as_f64()
                .unwrap_or(f64::NAN);
            let negative = sentiment["SentimentScore"]["Negative"]
                .as_f64()
                .unwrap_or(f64::NAN);
            match direction {
                "positive" => assert!(
                    positive > negative,
                    "case {case:?}: the clause around {mention:?} is positive, so expected \
                     Positive > Negative, got {positive} and {negative} in {sentiment}"
                ),
                "negative" => assert!(
                    negative > positive,
                    "case {case:?}: the clause around {mention:?} is negative, so expected \
                     Negative > Positive, got {negative} and {positive} in {sentiment}"
                ),
                _ => assert_eq!(
                    sentiment["Sentiment"].as_str().unwrap_or_default(),
                    "NEUTRAL",
                    "case {case:?}: the clause around {mention:?} carries no sentiment, so \
                     expected NEUTRAL, got {sentiment}"
                ),
            }
        }
    }
}

/// The label is still the arg-max of the clause's four confidences, which a clause
/// with an unambiguous polarity settles: the same document reports a POSITIVE and a
/// NEGATIVE mention.
#[tokio::test]
async fn a_mention_in_an_unambiguous_clause_carries_that_label() {
    let comprehend = Comprehend::new();

    let text = "The food was great but the service was terrible.";
    let output = analyse(&comprehend, text).await;
    assert_invariants("unambiguous clauses", text, &output);

    let labels: Vec<(String, String)> = output["Entities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|entity| entity["Mentions"].as_array().cloned().unwrap_or_default())
        .map(|mention| {
            (
                mention["Text"].as_str().unwrap_or_default().to_string(),
                mention["MentionSentiment"]["Sentiment"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();

    for (mention, expected) in [("food", "POSITIVE"), ("service", "NEGATIVE")] {
        let found = labels
            .iter()
            .find(|(text, _)| text == mention)
            .map(|(_, label)| label.as_str());
        assert_eq!(
            found,
            Some(expected),
            "case {mention:?}: expected {expected:?}, got {labels:?}"
        );
    }
}

/// Each mention's `MentionSentiment` is ONE distribution over the four modelled
/// classes, concentrated on the label it reports: a client aggregating per entity
/// ("how is this entity spoken of") reads a verdict it can act on, and the classes the
/// mention's clause carries no evidence for stay far below any threshold it sets.
///
/// The rows differ along what the clause around the mention carries: plain fact, one
/// evaluative word, several, and a document whose two clauses point opposite ways, so
/// a scorer assigning members from a table of constants would disagree with at least
/// one row.
#[tokio::test]
async fn a_mention_carries_one_distribution_concentrated_on_its_label() {
    let comprehend = Comprehend::new();

    /// How much of a mention's distribution the label it reports has to carry.
    const MENTION_CONCENTRATION: f64 = 0.95;
    /// What a class the mention's clause carries no evidence for may be reported at.
    const UNSUPPORTED_CLASS_CEILING: f64 = 1e-4;

    for (case, text, expected) in [
        (
            "a factual mention",
            "The invoice arrived on Tuesday.",
            vec![("invoice", "NEUTRAL"), ("Tuesday", "NEUTRAL")],
        ),
        (
            "one evaluative word",
            "Acme was wonderful.",
            vec![("Acme", "POSITIVE")],
        ),
        (
            "several evaluative words",
            "Acme was wonderful and the staff were excellent and helpful.",
            vec![("Acme", "POSITIVE")],
        ),
        (
            "two clauses pointing opposite ways",
            "The food was great but the service was terrible.",
            vec![("food", "POSITIVE"), ("service", "NEGATIVE")],
        ),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let mentions: Vec<(String, Value)> = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(|entity| entity["Mentions"].as_array().cloned().unwrap_or_default())
            .map(|mention| {
                (
                    mention["Text"].as_str().unwrap_or_default().to_string(),
                    mention["MentionSentiment"].clone(),
                )
            })
            .collect();

        for (mention, label) in expected {
            let sentiment = mentions
                .iter()
                .find(|(text, _)| text == mention)
                .map(|(_, sentiment)| sentiment.clone())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no mention of {mention:?} in {mentions:?}")
                });
            assert_eq!(
                sentiment["Sentiment"].as_str(),
                Some(label),
                "case {case:?}: expected {mention:?} to be reported {label}, got {sentiment}"
            );
            let member = match label {
                "POSITIVE" => "Positive",
                "NEGATIVE" => "Negative",
                "MIXED" => "Mixed",
                _ => "Neutral",
            };
            let reported = sentiment["SentimentScore"][member]
                .as_f64()
                .unwrap_or(f64::NAN);
            assert!(
                reported > MENTION_CONCENTRATION,
                "case {case:?}: expected {mention:?} to carry more than \
                 {MENTION_CONCENTRATION} on {member}, got {reported} in {sentiment}"
            );
            for other in ["Positive", "Negative", "Neutral", "Mixed"] {
                if other == member {
                    continue;
                }
                let residual = sentiment["SentimentScore"][other]
                    .as_f64()
                    .unwrap_or(f64::NAN);
                // Every member is a reading the clause leaves open — `MIXED` only
                // where both signed readings are open, so it keeps the room they
                // leave together — and the three it does not support stay strictly
                // positive: the lexicon could have missed the word that shows one.
                assert!(
                    (0.0..1.0 - MENTION_CONCENTRATION).contains(&residual),
                    "case {case:?}: expected {other} on {mention:?} inside \
                     [0, {}), got {residual} in {sentiment}",
                    1.0 - MENTION_CONCENTRATION
                );
                assert!(
                    residual > 0.0,
                    "case {case:?}: expected {other} on {mention:?}, a reading the \
                     clause leaves open, above 0, got {residual} in {sentiment}"
                );
            }
        }

        // A mention whose clause is plain fact leaves the signed classes negligible,
        // rather than reporting one of them at a magnitude a client would act on.
        if case == "a factual mention" {
            for (mention, _) in &mentions {
                let sentiment = mentions
                    .iter()
                    .find(|(text, _)| text == mention)
                    .map(|(_, sentiment)| sentiment.clone())
                    .unwrap_or_default();
                for signed in ["Positive", "Negative", "Mixed"] {
                    let score = sentiment["SentimentScore"][signed]
                        .as_f64()
                        .unwrap_or(f64::NAN);
                    assert!(
                        score < UNSUPPORTED_CLASS_CEILING,
                        "case {case:?}: {mention:?} stands in a clause of plain fact, so \
                         expected {signed} below {UNSUPPORTED_CLASS_CEILING}, got {score} \
                         in {sentiment}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Score and GroupScore are the confidences the mention's and the grouping's own
// evidence leaves
// ---------------------------------------------------------------------------

/// `TargetedSentimentMention$Score` is "model confidence that the entity is relevant"
/// and `$GroupScore` "the confidence that all the entities mentioned in the group
/// relate to the same entity" — both about THIS mention and THIS grouping, so both
/// follow what placed them: the cue, how much of the mention's characters it accounts
/// for, and for the grouping how much of the two forms agree and how far apart they
/// stand.
///
/// The rows differ along what the rule generalises over: which cue places the mention
/// (a recognised name, an honorific, bare capitalisation, a determiner-headed common
/// noun, a pronoun, a numeral), the LENGTH of the mention, and what links a second
/// mention to the first (the same form, a shared surname, a pronoun one sentence away
/// and the same pronoun three sentences away). A confidence read off a table of
/// per-rule constants would repeat across rows that carry plainly different evidence.
#[tokio::test]
async fn a_mention_confidence_follows_the_evidence_that_placed_it() {
    let comprehend = Comprehend::new();

    let rows = [
        ("a recognised company name", "Amazon refunded the fee."),
        (
            "a longer recognised company name",
            "Bank of America refunded the fee.",
        ),
        ("an honorific and a name", "Dr Okonkwo explained it."),
        (
            "a capitalised span no lexicon names",
            "Zyrtex Qualmon shipped it.",
        ),
        (
            "a determiner-headed common noun",
            "The service was available.",
        ),
        ("a pronoun", "Maria wrote it. She shipped it."),
        (
            "a pronoun three sentences on",
            "Maria wrote it. The build ran. The parcel arrived. She shipped it.",
        ),
        (
            "the same name written twice",
            "Amazon refunded the fee. Amazon apologised.",
        ),
        (
            "a surname joining a full name",
            "Alice Lennox filed it. Lennox signed it.",
        ),
    ];

    /// One mention, as the members this suite reads of it.
    #[derive(Debug, Clone)]
    struct Read {
        case: &'static str,
        text: String,
        score: f64,
        group_score: f64,
    }

    let mut read: Vec<Read> = Vec::new();
    for (case, text) in rows {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);
        for entity in output["Entities"].as_array().cloned().unwrap_or_default() {
            for mention in entity["Mentions"].as_array().cloned().unwrap_or_default() {
                let score = mention["Score"].as_f64().unwrap_or(f64::NAN);
                let group_score = mention["GroupScore"].as_f64().unwrap_or(f64::NAN);
                assert!(
                    score > 0.0 && score < 1.0,
                    "case {case:?}: expected a Score strictly inside (0, 1), got {score} \
                     in {mention}"
                );
                assert!(
                    group_score > 0.0 && group_score <= 1.0,
                    "case {case:?}: expected a GroupScore in (0, 1], got {group_score} \
                     in {mention}"
                );
                read.push(Read {
                    case,
                    text: mention["Text"].as_str().unwrap_or_default().to_string(),
                    score,
                    group_score,
                });
            }
        }
    }

    // No ladder: four per-rule constants would collapse every mention of the table
    // onto four values, and two more onto every grouping.
    let scores = crate::batch_inference_calibration::distinct(read.iter().map(|one| one.score));
    assert!(
        scores >= 6,
        "expected the mention confidences to follow each mention's own evidence, got \
         {scores} distinct values across {} mentions: {read:?}",
        read.len()
    );

    let mention = |case: &str, wanted: &str| -> Read {
        read.iter()
            .find(|one| one.case == case && one.text == wanted)
            .cloned()
            .unwrap_or_else(|| panic!("no mention {wanted:?} for case {case:?} in {read:?}"))
    };

    // More evidence, more confidence — each pair differs in exactly one thing about
    // the evidence that placed the mention.
    for (stronger, weaker, why) in [
        (
            mention("a longer recognised company name", "Bank of America"),
            mention("a recognised company name", "Amazon"),
            "a lexicon naming more of the mention's characters is less likely to be a \
             coincidence",
        ),
        (
            mention("an honorific and a name", "Okonkwo"),
            mention("a capitalised span no lexicon names", "Zyrtex Qualmon"),
            "an honorific says what kind of thing is named there; a capital alone does \
             not",
        ),
        (
            mention("a pronoun", "She"),
            mention("a determiner-headed common noun", "service"),
            "a closed-class word the lexicon spells out names the whole mention, where \
             a common noun under a determiner is as much the generic reading",
        ),
    ] {
        assert!(
            stronger.score > weaker.score,
            "case {:?} vs {:?}: {why}, so expected a higher Score, got {} against {}",
            stronger.case,
            weaker.case,
            stronger.score,
            weaker.score
        );
    }

    // The grouping link follows ITS own evidence: an identical form agrees character
    // for character, a shared surname leaves the given name unaccounted for, and a
    // pronoun's link decays with the sentences between it and its antecedent.
    let same_form = mention("the same name written twice", "Amazon").group_score;
    let partial_name = mention("a surname joining a full name", "Lennox").group_score;
    let near_pronoun = mention("a pronoun", "She").group_score;
    let far_pronoun = mention("a pronoun three sentences on", "She").group_score;
    assert!(
        same_form > partial_name,
        "expected an identical form to group more confidently than a shared surname, \
         got {same_form} against {partial_name}"
    );
    assert!(
        near_pronoun > far_pronoun,
        "expected a pronoun's link to its antecedent to weaken with the sentences \
         between them, got {near_pronoun} against {far_pronoun}"
    );
    assert!(
        far_pronoun > 0.0,
        "expected a distant antecedent to stay a possible reading, got {far_pronoun}"
    );

    // One recognition, one confidence: a mention this service's entity recogniser
    // placed is reported with the very confidence `DetectEntities` reports for the
    // same span, so the two operations cannot disagree about how sure the service is.
    for (case, text, name) in [
        ("a recognised company name", "Amazon refunded the fee.", "Amazon"),
        (
            "a longer recognised company name",
            "Bank of America refunded the fee.",
            "Bank of America",
        ),
        (
            "a surname joining a full name",
            "Alice Lennox filed it. Lennox signed it.",
            "Alice Lennox",
        ),
    ] {
        let entities = comprehend
            .ok(
                "DetectEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let recognised = entities["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .find(|entity| entity["Text"].as_str() == Some(name))
            .and_then(|entity| entity["Score"].as_f64())
            .unwrap_or_else(|| panic!("case {case:?}: no {name:?} entity in {entities}"));
        let targeted = mention(case, name).score;
        assert!(
            (recognised - targeted).abs() < 1e-9,
            "case {case:?}: expected the mention of {name:?} to carry the confidence the \
             recogniser read it with ({recognised}), got {targeted}"
        );
    }
}

#[tokio::test]
async fn a_document_with_no_word_reports_no_entity() {
    let comprehend = Comprehend::new();

    for (case, text) in [
        ("punctuation only", "!!! ... ???"),
        ("whitespace and punctuation", " - , . "),
        ("one function word", "the"),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);
        let entities = output["Entities"].as_array().cloned().unwrap_or_default();
        assert!(
            entities.is_empty(),
            "case {case:?}: no rule places a mention in {text:?}, so no entity may be \
             reported, got {entities:?}"
        );
    }
}

#[tokio::test]
async fn the_same_document_is_analysed_the_same_way_twice() {
    let comprehend = Comprehend::new();

    for (case, text) in DOCUMENTS {
        let body = json!({ "Text": text, "LanguageCode": "en" });
        let first = comprehend.ok("DetectTargetedSentiment", &body).await;
        let second = comprehend.ok("DetectTargetedSentiment", &body).await;
        assert_eq!(
            first, second,
            "case {case:?}: expected the same entities and offsets twice, got {first} then \
             {second}"
        );
    }
}

#[tokio::test]
async fn a_language_the_operation_does_not_analyse_is_refused() {
    let comprehend = Comprehend::new();

    // `DetectTargetedSentimentRequest$LanguageCode` documents "Currently, English is
    // the only supported language", so every other modelled value is refused — as an
    // input-validation constraint over the operation's own set, which is what
    // `sync_inference_refusals.rs` asserts the member path, the value and the set of.
    for code in LANGUAGE_CODE_VALUES {
        let (status, body) = comprehend
            .call(
                "DetectTargetedSentiment",
                &json!({ "Text": "Acme was wonderful.", "LanguageCode": code }),
            )
            .await;

        if *code == "en" {
            assert_eq!(
                status, 200,
                "case {code:?}: English is analysed, expected 200, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {code:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {code:?}: expected ValidationException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn text_at_the_documented_ceiling_is_accepted_and_one_byte_past_it_is_not() {
    let comprehend = Comprehend::new();

    // `DetectTargetedSentimentRequest$Text` documents "The maximum string length is
    // 5 KB": the modelled `@length` floor of 1, the ceiling exactly, one byte past.
    let ceiling = 5 * 1024;
    for size in [1usize, ceiling, ceiling + 1] {
        let (status, body) = comprehend
            .call(
                "DetectTargetedSentiment",
                &json!({ "Text": "a".repeat(size), "LanguageCode": "en" }),
            )
            .await;

        if size > ceiling {
            assert_eq!(
                status, 400,
                "case {size} bytes: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "TextSizeLimitExceededException",
                "case {size} bytes: expected TextSizeLimitExceededException, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {size} bytes: expected 200, got {status}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_empty_absent_or_unmodelled_input_is_refused() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        ("empty Text", json!({ "Text": "", "LanguageCode": "en" })),
        ("absent Text", json!({ "LanguageCode": "en" })),
        ("absent LanguageCode", json!({ "Text": "Acme was wonderful." })),
        (
            "LanguageCode outside the modelled enum",
            json!({ "Text": "Acme was wonderful.", "LanguageCode": "EN" }),
        ),
    ] {
        let (status, response) = comprehend.call("DetectTargetedSentiment", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
    }
}

/// Documents whose names the entity recogniser types in PARTS, or types over fewer
/// words than the document capitalises, with the whole named mention each one writes.
///
/// The rows differ along how the name is written and what the recogniser can say
/// about it: a company name carrying a product name of its own, a three-word name, a
/// name with an ordinary capitalised word in front of it (a sentence-opening verb),
/// a legal-entity suffix the recogniser types from, a name repeated, a name written
/// in a non-ASCII script, and a recognition reaching past the run of name words.
const COMPOUND_NAMES: &[(&str, &str, &str, &[&str])] = &[
    (
        "a company name carrying a product name of its own",
        "Amazon Comprehend returned the analysis.",
        "Amazon Comprehend",
        &["Amazon", "Comprehend"],
    ),
    (
        "the same compound name twice",
        "Amazon Comprehend shipped. Amazon Comprehend improved.",
        "Amazon Comprehend",
        &["Amazon", "Comprehend"],
    ),
    (
        "a capitalised ordinary word in front of a person name",
        "Contact John Smith about the invoice.",
        "John Smith",
        &["Contact John Smith", "John", "Smith"],
    ),
    (
        "a legal-entity suffix",
        "Initech Holdings Inc. filed the report.",
        "Initech Holdings Inc",
        &["Initech", "Holdings", "Initech Holdings"],
    ),
    (
        "a name in a non-ASCII script",
        "Zoë Müller signed the contract.",
        "Zoë Müller",
        &["Zoë", "Müller"],
    ),
    (
        // The other half of rule 1: a recognition that reaches PAST the run of name
        // words takes the mention with it, rather than the run cutting it short.
        "a recognition reaching past the run of name words",
        "The audit closed in March 2024 and nobody objected.",
        "March 2024",
        &["March", "2024"],
    ),
];

#[tokio::test]
async fn a_named_mention_is_reported_whole_however_the_recogniser_types_its_parts() {
    let comprehend = Comprehend::new();

    for (case, text, whole, fragments) in COMPOUND_NAMES {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let mentions: Vec<(String, String, i64)> = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(mentions_of)
            .collect();

        assert!(
            mentions.iter().any(|(mention, _, _)| mention == whole),
            "case {case:?}: expected the whole named mention {whole:?}, got {mentions:?}"
        );
        for fragment in *fragments {
            assert!(
                !mentions.iter().any(|(mention, _, _)| mention == fragment),
                "case {case:?}: {fragment:?} is not the whole named mention {whole:?}, so \
                 no mention may report it, got {mentions:?}"
            );
        }
        // One entity per entity: every mention of the name belongs to the one group,
        // so no half of it opens a second one.
        let holders = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|entity| {
                mentions_of(entity)
                    .iter()
                    .any(|(mention, _, _)| mention == whole)
            })
            .count();
        assert_eq!(
            holders, 1,
            "case {case:?}: expected {whole:?} to be mentioned by exactly one entity, got \
             {holders} in {output}"
        );
    }
}

#[tokio::test]
async fn no_two_mentions_are_the_halves_of_one_name() {
    let comprehend = Comprehend::new();

    // Over every document either suite analyses: two mentions with nothing but
    // whitespace between them, both opening with a capital, are one name reported
    // twice. A `PERSONAL_TITLE` is exempt — an honorific IS its own mention, beside
    // the name it titles.
    let documents = DOCUMENTS
        .iter()
        .map(|(case, text)| (*case, *text))
        .chain(
            COMPOUND_NAMES
                .iter()
                .map(|(case, text, _, _)| (*case, *text)),
        );
    for (case, text) in documents {
        let output = analyse(&comprehend, text).await;
        let characters: Vec<char> = text.chars().collect();
        let mut mentions: Vec<(i64, i64, String, String)> = Vec::new();
        for entity in output["Entities"].as_array().cloned().unwrap_or_default() {
            for mention in entity["Mentions"].as_array().cloned().unwrap_or_default() {
                mentions.push((
                    mention["BeginOffset"].as_i64().unwrap_or(-1),
                    mention["EndOffset"].as_i64().unwrap_or(-1),
                    mention["Text"].as_str().unwrap_or_default().to_string(),
                    mention["Type"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }
        mentions.sort_by_key(|(begin, _, _, _)| *begin);

        for pair in mentions.windows(2) {
            let (_, first_end, first_text, first_type) = &pair[0];
            let (second_begin, _, second_text, second_type) = &pair[1];
            if first_type == "PERSONAL_TITLE" || second_type == "PERSONAL_TITLE" {
                continue;
            }
            let between: String = characters
                [(*first_end).max(0) as usize..(*second_begin).max(0) as usize]
                .iter()
                .collect();
            let adjacent_words = !between.is_empty()
                && between.chars().all(char::is_whitespace)
                && first_text
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase)
                && second_text
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase);
            assert!(
                !adjacent_words,
                "case {case:?}: {first_text:?} and {second_text:?} are adjacent words of \
                 one name, so they are one mention, got {mentions:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_mention_carries_the_recognisers_own_confidence_for_it() {
    let comprehend = Comprehend::new();

    // Documents naming things the recogniser evidences differently: a lexicon entry, a
    // name shape in a person-bearing position, a closed-form address, a figure, a
    // two-letter abbreviation nothing corroborates.
    for (case, text) in [
        (
            "a company, a person and an e-mail address",
            "Amazon Comprehend reads the document. Contact John Smith at \
             john.smith@example.com.",
        ),
        (
            "an address, a figure and an abbreviation",
            "Deliver 1,250 units to 123 Main Street, Seattle, WA 98101.",
        ),
    ] {
        let body = json!({ "Text": text, "LanguageCode": "en" });
        let recognised = comprehend.ok("DetectEntities", &body).await;
        let entities: Vec<(i64, f64, String)> = recognised["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entity| {
                (
                    entity["BeginOffset"].as_i64().unwrap_or(-1),
                    entity["Score"].as_f64().unwrap_or(f64::NAN),
                    entity["Text"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert!(
            entities.len() > 1,
            "case {case:?}: the document names several things, so the recogniser must \
             place several entities, got {entities:?}"
        );

        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);
        let mut matched = 0usize;
        let mut scores: Vec<f64> = Vec::new();
        for entity in output["Entities"].as_array().cloned().unwrap_or_default() {
            for mention in entity["Mentions"].as_array().cloned().unwrap_or_default() {
                let begin = mention["BeginOffset"].as_i64().unwrap_or(-1);
                let score = mention["Score"].as_f64().unwrap_or(f64::NAN);
                scores.push(score);
                // Where the recogniser placed a recognition that opens this mention,
                // the mention's confidence is that recognition's own — the two
                // operations read one recogniser, so they cannot report two
                // confidences for one piece of evidence.
                let Some((_, recognised_score, recognised_text)) =
                    entities.iter().find(|(at, _, _)| *at == begin)
                else {
                    continue;
                };
                matched += 1;
                assert!(
                    (score - recognised_score).abs() < 1e-6,
                    "case {case:?}: the mention at {begin} carries Score {score}, where \
                     the recogniser reports {recognised_score} for {recognised_text:?}"
                );
            }
        }
        assert!(
            matched > 1,
            "case {case:?}: expected several mentions the recogniser typed, got {matched}"
        );
        // Not one value per shape: two mentions the document evidences differently are
        // reported differently.
        let distinct = scores
            .iter()
            .map(|score| format!("{score:.9}"))
            .collect::<BTreeSet<String>>();
        assert!(
            distinct.len() > 1,
            "case {case:?}: every mention is reported at the same confidence {scores:?}, \
             which is a constant of the shape rather than evidence for the mention"
        );
    }
}

/// `MIXED` is the confidence a clause carries BOTH polarities, so it is reported as
/// measured: the leading reading where the clause carries both in balance, and — where
/// it carries at most one — the room the two signed readings leave TOGETHER, which is
/// strictly positive, since the lexicon can miss the word that shows either, and
/// strictly below the room each of them keeps on its own. No member of the
/// distribution is a flat impossibility.
///
/// The rows differ along how many polarities the clause the mention sits in carries:
/// none, one of each, and both at once.
#[tokio::test]
async fn mention_sentiment_reports_mixed_as_measured() {
    let comprehend = Comprehend::new();

    for (case, text, mention, carries_both) in [
        ("plain fact", "The invoice arrived on Tuesday.", "invoice", false),
        ("one positive word", "Acme was wonderful.", "Acme", false),
        ("one negative word", "Acme was terrible.", "Acme", false),
        (
            "both polarities in one clause",
            "The Acme release was excellent and awful.",
            "Acme",
            true,
        ),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let scores = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(|entity| entity["Mentions"].as_array().cloned().unwrap_or_default())
            .find(|reported| reported["Text"].as_str() == Some(mention))
            .map(|reported| reported["MentionSentiment"]["SentimentScore"].clone())
            .unwrap_or_else(|| panic!("case {case:?}: no mention of {mention:?} in {output}"));

        let mixed = scores["Mixed"].as_f64().unwrap_or(f64::NAN);
        if carries_both {
            assert!(
                mixed > 0.0,
                "case {case:?}: the clause carries both polarities, so MIXED is \
                 supported, got {mixed} in {scores}"
            );
        } else {
            // MIXED needs a missed word of EACH polarity, so the room it keeps is the
            // two signed rooms multiplied: above zero, below either of them.
            let positive = scores["Positive"].as_f64().unwrap_or(f64::NAN);
            let negative = scores["Negative"].as_f64().unwrap_or(f64::NAN);
            assert!(
                mixed > 0.0 && mixed < positive.min(negative),
                "case {case:?}: the clause carries at most one polarity, so MIXED keeps \
                 the room the two signed readings leave together — above 0 and below \
                 both — got {mixed} against {positive} and {negative} in {scores}"
            );
        }
        for open in ["Positive", "Negative", "Neutral", "Mixed"] {
            let reported = scores[open].as_f64().unwrap_or(f64::NAN);
            assert!(
                reported > 0.0 && reported < 1.0,
                "case {case:?}: {open} is a reading the clause leaves open, so it is \
                 reported strictly inside (0, 1), got {reported} in {scores}"
            );
        }
        let total: f64 = ["Positive", "Negative", "Neutral", "Mixed"]
            .into_iter()
            .map(|label| scores[label].as_f64().unwrap_or(f64::NAN))
            .sum();
        assert!(
            (total - 1.0).abs() < 1e-5,
            "case {case:?}: the four confidences are a distribution, so they sum to 1, \
             got {total} from {scores}"
        );
    }
}
