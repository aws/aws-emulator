// SPDX-License-Identifier: CC0-1.0

//! What Comprehend's SINGLE-DOCUMENT synchronous inference family reports, as the
//! CONVENTIONS each analyser follows rather than as one recorded answer.
//!
//! Five rules, one suite each, every one driven over a table whose rows differ along
//! what the rule generalises over rather than along the one document that showed the
//! defect:
//!
//! 1. `DetectToxicContent`: `Labels` is a per-type score VECTOR — one entry for every
//!    modelled `ToxicContentType`, in the order Comprehend's own toxicity reference
//!    prints them, each with its own confidence — so a moderation client reads a
//!    category's score by looking the type up, and a benign segment is reported as
//!    every category scoring zero rather than as a response carrying no toxicity
//!    information at all.
//! 2. `DetectSentiment`: the verdict is the arg-max of a distribution in which
//!    `Neutral` holds whatever the document's sentiment-bearing material does not
//!    outweigh, so a long document of plain fact carrying one evaluative word is
//!    NEUTRAL while a short evaluative one is not.
//! 3. `DetectSyntax`: a word's tag is the Universal Dependencies reading of the
//!    position it stands in — a name's words are `PROPN`, a copula standing as its
//!    clause's only verb is `VERB`, a relativizer standing for the noun its clause
//!    modifies is `PRON`, and a gerund naming a thing inside a nominal compound is
//!    `NOUN`.
//! 4. `DetectPiiEntities` / `ContainsPiiEntities`: a modelled PII type with no closed
//!    form of its own is still placed — a personal NAME the document does not
//!    introduce with a courtesy title, and an ADDRESS over the whole address the
//!    document writes — and the two operations answer the same set of types.
//! 5. `DetectEntities` / `DetectTargetedSentiment`: a company's name followed by a
//!    name of its own is TWO entities (the company, then its product), and targeted
//!    sentiment types every target from that same recogniser, including a named thing
//!    the document writes without a capital.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    ENTITY_TYPE_VALUES, PART_OF_SPEECH_TAG_TYPE_VALUES, TARGETED_SENTIMENT_ENTITY_TYPE_VALUES,
    TOXIC_CONTENT_TYPE_VALUES,
};

use crate::comprehend_client::Comprehend;

/// An English single-document request carrying `text`.
fn document(text: &str) -> Value {
    json!({ "Text": text, "LanguageCode": "en" })
}

// ---------------------------------------------------------------------------
// 1. DetectToxicContent: Labels is a per-type score vector
// ---------------------------------------------------------------------------

/// The order one `ToxicLabels` entry reports the modelled content types in.
///
/// Source: the response Comprehend's toxicity-detection reference prints for a text
/// segment, which carries one `ToxicContent` entry per content type in this order.
/// The SET is asserted against the modelled `ToxicContentType` enum below, so a model
/// that gains a type fails here rather than silently dropping it from the vector.
const REPORTED_CONTENT_TYPES: &[&str] = &[
    "PROFANITY",
    "HATE_SPEECH",
    "INSULT",
    "GRAPHIC",
    "HARASSMENT_OR_ABUSE",
    "SEXUAL",
    "VIOLENCE_OR_THREAT",
];

/// `(case, segment, whether the analysers place any toxic content in it)`.
///
/// The rows differ along what the rule generalises over: how much toxic content a
/// segment carries (none, one category, several) and what KIND — the language it uses
/// and the conduct it states — since the vector has to carry every category for all of
/// them.
const SEGMENTS: &[(&str, &str, bool)] = &[
    (
        "a benign factual segment",
        "The quarterly report is published on the investor relations page.",
        false,
    ),
    (
        "a benign segment naming people and places",
        "Maria Lopez moved to Lisbon in March and joined the research team.",
        false,
    ),
    ("abuse directed at the recipient", "You are worthless.", true),
    (
        "a stated threat of violence",
        "I will kill you tomorrow.",
        true,
    ),
    (
        "abuse and a threat in one segment",
        "You are pathetic and I will hurt you for it.",
        true,
    ),
];

#[tokio::test]
async fn toxic_content_reports_one_score_for_every_modelled_content_type() {
    let comprehend = Comprehend::new();

    // The vector's set is the model's own enum, whatever order it is reported in.
    let mut reported = REPORTED_CONTENT_TYPES.to_vec();
    reported.sort_unstable();
    let mut modelled = TOXIC_CONTENT_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        reported, modelled,
        "case \"the reported vector\": expected every modelled ToxicContentType exactly \
         once, got {reported:?} against {modelled:?}"
    );

    for (case, text, toxic) in SEGMENTS {
        let output = comprehend
            .ok(
                "DetectToxicContent",
                &json!({ "LanguageCode": "en", "TextSegments": [{ "Text": text }] }),
            )
            .await;
        let entry = &output["ResultList"][0];
        let labels = entry["Labels"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Labels array, got {output}"))
            .clone();

        let names: Vec<&str> = labels
            .iter()
            .map(|label| label["Name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names, REPORTED_CONTENT_TYPES,
            "case {case:?}: Labels is the per-type score vector, so it carries every \
             modelled content type in the reported order, got {names:?}"
        );

        let scores: Vec<f64> = labels
            .iter()
            .map(|label| label["Score"].as_f64().unwrap_or(f64::NAN))
            .collect();
        for (name, score) in names.iter().zip(&scores) {
            assert!(
                (0.0..=1.0).contains(score),
                "case {case:?}: expected a Score in [0, 1] for {name:?}, got {score}"
            );
        }

        let toxicity = entry["Toxicity"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Toxicity, got {entry}"));
        let combined = 1.0 - scores.iter().map(|score| 1.0 - score).product::<f64>();
        assert!(
            (toxicity - combined).abs() < 1e-6,
            "case {case:?}: Toxicity is the confidence that the segment carries toxic \
             content of ANY type, so it combines the vector: expected {combined}, got \
             {toxicity} from {scores:?}"
        );
        assert_eq!(
            scores.iter().any(|score| *score > 0.0),
            *toxic,
            "case {case:?}: expected toxic={toxic}, got the vector {scores:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. DetectSentiment: neutral holds what the document's sentiment does not outweigh
// ---------------------------------------------------------------------------

/// `(case, document, the modelled SentimentType the four confidences must make the
/// arg-max)`.
///
/// The rows differ along polarity, along the DENSITY of the document's
/// sentiment-bearing material — the same evaluative words in a short document and in a
/// long factual one — and along how the two polarities balance.
const SENTIMENT_DOCUMENTS: &[(&str, &str, &str)] = &[
    (
        "a long factual document carrying one evaluative word",
        "The service reads the document, finds the entities in it, and writes the \
         results to the bucket named in the request, where the next stage of the \
         pipeline reads them in a nice order for the operator on duty.",
        "NEUTRAL",
    ),
    (
        "the same evaluative word in a short document",
        "The order was nice.",
        "POSITIVE",
    ),
    ("plain fact", "The meeting starts at three.", "NEUTRAL"),
    ("short praise", "I love this product.", "POSITIVE"),
    (
        "sustained praise",
        "I love this product, it is wonderful and the battery is excellent.",
        "POSITIVE",
    ),
    ("short complaint", "The seat was uncomfortable.", "NEGATIVE"),
    (
        "both polarities in balance",
        "The food was delicious but the service was terrible.",
        "MIXED",
    ),
];

#[tokio::test]
async fn sentiment_follows_how_much_of_the_document_carries_sentiment() {
    let comprehend = Comprehend::new();
    let mut neutral_of: Vec<(&str, f64)> = Vec::new();

    for (case, text, expected) in SENTIMENT_DOCUMENTS {
        let output = comprehend.ok("DetectSentiment", &document(text)).await;
        let scores = &output["SentimentScore"];
        let by_label: Vec<(&str, f64)> = ["Positive", "Negative", "Neutral", "Mixed"]
            .into_iter()
            .map(|label| {
                let score = scores[label].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: expected SentimentScore.{label}, got {output}")
                });
                (label, score)
            })
            .collect();

        let total: f64 = by_label.iter().map(|(_, score)| score).sum();
        assert!(
            (total - 1.0).abs() < 1e-5,
            "case {case:?}: the four confidences are a distribution, so they sum to 1, \
             got {total} from {output}"
        );
        for (label, score) in &by_label {
            assert!(
                *score > 0.0 && *score < 1.0,
                "case {case:?}: expected SentimentScore.{label} strictly inside (0, 1), \
                 got {score}"
            );
        }
        let winner = by_label
            .iter()
            .copied()
            .reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best })
            .map(|(label, _)| label.to_uppercase())
            .unwrap_or_default();
        assert_eq!(
            output["Sentiment"].as_str(),
            Some(winner.as_str()),
            "case {case:?}: Sentiment must be the arg-max of the four confidences, got \
             {output}"
        );
        assert_eq!(
            output["Sentiment"].as_str(),
            Some(*expected),
            "case {case:?}: expected {expected:?}, got {output}"
        );

        let neutral = by_label
            .iter()
            .find(|(label, _)| *label == "Neutral")
            .map_or(f64::NAN, |(_, score)| *score);
        neutral_of.push((case, neutral));
    }

    // The same evaluative word says less about a long document of fact than about a
    // short document that is only that word's clause: the confidence follows the SHARE
    // of the document carrying sentiment, not the count of words that do.
    let neutral = |case: &str| -> f64 {
        neutral_of
            .iter()
            .find(|(name, _)| *name == case)
            .map_or(f64::NAN, |(_, score)| *score)
    };
    let long = neutral("a long factual document carrying one evaluative word");
    let short = neutral("the same evaluative word in a short document");
    assert!(
        long > short,
        "expected the long factual document to be reported more neutral than the short \
         evaluative one, got {long} against {short}"
    );
}

// ---------------------------------------------------------------------------
// 3. DetectSyntax: the tag is the reading of the position
// ---------------------------------------------------------------------------

/// `(case, document, the (token, tag) pairs the reading must contain)`.
type SyntaxRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

/// The rows differ along the construction each word stands in rather than along the
/// word: a name opening a sentence against an ordinary word opening one, a copula with
/// and without a verb behind it, a relativizer standing for a noun against one
/// determining a noun, and a gerund naming a thing against one in a progressive.
///
/// The tag set is Universal Dependencies v2 (https://universaldependencies.org/u/pos/),
/// which is what the modelled `PartOfSpeechTagType` values name; every expected tag is
/// asserted to be one of them.
const SYNTAX_READINGS: &[SyntaxRow] = &[
    (
        "a name in subject position opening a sentence",
        "Amazon Comprehend is a natural language processing service that uses machine \
         learning to find insights in text.",
        &[
            ("Amazon", "PROPN"),
            ("Comprehend", "PROPN"),
            // A copula with no verb behind it IS its clause's verb.
            ("is", "VERB"),
            // A gerund naming a thing inside a nominal compound.
            ("processing", "NOUN"),
            ("learning", "NOUN"),
            // A relativizer standing for the noun its clause modifies.
            ("that", "PRON"),
            ("uses", "VERB"),
        ],
    ),
    (
        "an ordinary word opening a sentence in front of a name",
        "Contact John Smith at the Seattle office.",
        &[
            // The capital is the orthography's, and no predicate follows the run, so
            // the word is not read as part of the name behind it.
            ("Contact", "NOUN"),
            ("John", "PROPN"),
            ("Smith", "PROPN"),
            ("Seattle", "PROPN"),
        ],
    ),
    (
        "a word the lexicon claims standing inside a name",
        "The parcel went to Main Street yesterday.",
        &[("Main", "PROPN"), ("Street", "PROPN")],
    ),
    (
        "a copula with a verb behind it, and a determiner reading",
        "The team is reviewing that report.",
        &[
            // An auxiliary accompanying a main verb, and the progressive it heads.
            ("is", "AUX"),
            ("reviewing", "VERB"),
            // A relativizer determining a noun of its own is a determiner.
            ("that", "DET"),
            ("report", "NOUN"),
        ],
    ),
    (
        "a single capitalised common noun opening a sentence",
        "Dogs bark loudly.",
        &[("Dogs", "NOUN"), ("bark", "VERB")],
    ),
];

#[tokio::test]
async fn syntax_tags_the_reading_of_the_position_a_word_stands_in() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in SYNTAX_READINGS {
        let output = comprehend.ok("DetectSyntax", &document(text)).await;
        let tokens = output["SyntaxTokens"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected SyntaxTokens, got {output}"))
            .clone();
        let read: Vec<(String, String)> = tokens
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

        for (word, tag) in *expected {
            assert!(
                PART_OF_SPEECH_TAG_TYPE_VALUES.contains(tag),
                "case {case:?}: {tag:?} is not a modelled PartOfSpeechTagType value"
            );
            let readings: Vec<&String> = read
                .iter()
                .filter(|(text, _)| text == word)
                .map(|(_, tag)| tag)
                .collect();
            assert!(
                !readings.is_empty(),
                "case {case:?}: expected a token {word:?} in {read:?}"
            );
            for reading in readings {
                assert_eq!(
                    reading, tag,
                    "case {case:?}: expected {word:?} to read {tag:?}, got {reading:?} in \
                     {read:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. The PII family: a type with no closed form is still placed
// ---------------------------------------------------------------------------

/// `(case, document, the (type, text) pairs the recogniser must place)`.
type PiiRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

/// The rows differ along HOW the document writes the value rather than along the type:
/// a name introduced by a courtesy title and one standing in running text, an address
/// written as a street line alone and one written with its whole locality line.
const PII_PLACEMENTS: &[PiiRow] = &[
    (
        "a name introduced by a courtesy title",
        "The lease was signed by Dr. Jane Roe.",
        &[("NAME", "Jane Roe")],
    ),
    (
        "a name standing in running text",
        "Contact John Smith at the office.",
        &[("NAME", "John Smith")],
    ),
    (
        "a name the document writes beside its address and e-mail",
        "Contact John Smith at 123 Main Street, Seattle, WA 98101 or \
         john.smith@example.com.",
        &[
            ("NAME", "John Smith"),
            ("ADDRESS", "123 Main Street, Seattle, WA 98101"),
            ("EMAIL", "john.smith@example.com"),
        ],
    ),
    (
        "an address written as a street line alone",
        "The parcel went to 42 Oak Avenue last week.",
        &[("ADDRESS", "42 Oak Avenue")],
    ),
    (
        "an address written with a city and a postal code",
        "The parcel went to 42 Oak Avenue, Portland, OR 97201 last week.",
        &[("ADDRESS", "42 Oak Avenue, Portland, OR 97201")],
    ),
];

#[tokio::test]
async fn the_pii_family_places_every_type_the_document_carries() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in PII_PLACEMENTS {
        let output = comprehend.ok("DetectPiiEntities", &document(text)).await;
        let entities = output["Entities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Entities, got {output}"))
            .clone();
        let characters: Vec<char> = text.chars().collect();
        let placed: Vec<(String, String)> = entities
            .iter()
            .map(|entity| {
                let begin = entity["BeginOffset"].as_u64().unwrap_or_default() as usize;
                let end = entity["EndOffset"].as_u64().unwrap_or_default() as usize;
                (
                    entity["Type"].as_str().unwrap_or_default().to_string(),
                    characters[begin..end.min(characters.len())]
                        .iter()
                        .collect::<String>(),
                )
            })
            .collect();

        for (entity_type, value) in *expected {
            assert!(
                placed
                    .iter()
                    .any(|(placed_type, placed_value)| placed_type == entity_type
                        && placed_value == value),
                "case {case:?}: expected a {entity_type} over {value:?}, got {placed:?}"
            );
        }

        // Offsets delimit the value: a redaction client driven by them removes exactly
        // the entity, and every score is a confidence inside (0, 1].
        for entity in &entities {
            let score = entity["Score"].as_f64().unwrap_or(f64::NAN);
            assert!(
                score > 0.0 && score <= 1.0,
                "case {case:?}: expected a Score in (0, 1], got {score} in {entity}"
            );
        }

        // The two operations read ONE recogniser, so the label set is exactly the set
        // of types the entity list carries.
        let labelled = comprehend.ok("ContainsPiiEntities", &document(text)).await;
        let mut labels: Vec<String> = labelled["Labels"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Labels, got {labelled}"))
            .iter()
            .map(|label| label["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        labels.sort();
        labels.dedup();
        let mut types: Vec<String> = placed
            .iter()
            .map(|(entity_type, _)| entity_type.clone())
            .collect();
        types.sort();
        types.dedup();
        assert_eq!(
            labels, types,
            "case {case:?}: ContainsPiiEntities labels the types DetectPiiEntities \
             places, got {labels:?} against {types:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. The named-thing recogniser the entity and targeted-sentiment families share
// ---------------------------------------------------------------------------

/// `(case, document, the (type, text) pairs the entity list must carry)`.
type EntityRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

/// The rows differ along how the document writes the name: a company name followed by
/// the name of a product of its own, a company name followed by its own legal-form
/// designator (which is part of the company's name and so is NOT divided off), and a
/// name standing after an ordinary word the orthography capitalised.
const ENTITY_READINGS: &[EntityRow] = &[
    (
        "a company name followed by a product of its own",
        "Amazon Comprehend reads the document.",
        &[("ORGANIZATION", "Amazon"), ("COMMERCIAL_ITEM", "Comprehend")],
    ),
    (
        "another company name followed by a product of its own",
        "The team evaluated Microsoft Fabric last quarter.",
        &[("ORGANIZATION", "Microsoft"), ("COMMERCIAL_ITEM", "Fabric")],
    ),
    (
        "a company name followed by its legal-form designator",
        "Initech Inc. shipped a release.",
        &[("ORGANIZATION", "Initech Inc")],
    ),
    (
        "a name standing after an ordinary word opening the sentence",
        "Contact John Smith at the office.",
        &[("PERSON", "John Smith")],
    ),
];

#[tokio::test]
async fn a_company_name_and_the_name_after_it_are_two_entities() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in ENTITY_READINGS {
        let output = comprehend.ok("DetectEntities", &document(text)).await;
        let entities = output["Entities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Entities, got {output}"))
            .clone();
        let characters: Vec<char> = text.chars().collect();
        let read: Vec<(String, String)> = entities
            .iter()
            .map(|entity| {
                let begin = entity["BeginOffset"].as_u64().unwrap_or_default() as usize;
                let end = entity["EndOffset"].as_u64().unwrap_or_default() as usize;
                let slice: String = characters[begin..end.min(characters.len())]
                    .iter()
                    .collect();
                assert_eq!(
                    entity["Text"].as_str(),
                    Some(slice.as_str()),
                    "case {case:?}: an entity's Text is the characters its offsets \
                     delimit, got {entity}"
                );
                (
                    entity["Type"].as_str().unwrap_or_default().to_string(),
                    slice,
                )
            })
            .collect();

        for (entity_type, value) in *expected {
            assert!(
                ENTITY_TYPE_VALUES.contains(entity_type),
                "case {case:?}: {entity_type:?} is not a modelled EntityType value"
            );
            assert!(
                read.iter()
                    .any(|(read_type, read_value)| read_type == entity_type
                        && read_value == value),
                "case {case:?}: expected a {entity_type} over {value:?}, got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn targeted_sentiment_types_every_target_from_the_entity_recogniser() {
    let comprehend = Comprehend::new();
    // One document naming a company, a product of it, a person, and a thing with no
    // capital of its own — the four kinds of named target the rule generalises over.
    let text = "Amazon Comprehend reads the document. Contact John Smith at \
                john.smith@example.com.";
    let output = comprehend
        .ok("DetectTargetedSentiment", &document(text))
        .await;
    let entities = output["Entities"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"named targets\": expected Entities, got {output}"))
        .clone();

    let characters: Vec<char> = text.chars().collect();
    let mut targets: Vec<(String, String)> = Vec::new();
    for entity in &entities {
        for mention in entity["Mentions"].as_array().cloned().unwrap_or_default() {
            let begin = mention["BeginOffset"].as_u64().unwrap_or_default() as usize;
            let end = mention["EndOffset"].as_u64().unwrap_or_default() as usize;
            let slice: String = characters[begin..end.min(characters.len())]
                .iter()
                .collect();
            let entity_type = mention["Type"].as_str().unwrap_or_default().to_string();
            assert!(
                TARGETED_SENTIMENT_ENTITY_TYPE_VALUES.contains(&entity_type.as_str()),
                "case \"named targets\": {entity_type:?} is not a modelled \
                 TargetedSentimentEntityType value"
            );
            assert_eq!(
                mention["Text"].as_str(),
                Some(slice.as_str()),
                "case \"named targets\": a mention's Text is the characters its offsets \
                 delimit, got {mention}"
            );
            targets.push((entity_type, slice));
        }
    }

    for (entity_type, value) in [
        ("ORGANIZATION", "Amazon"),
        ("COMMERCIAL_ITEM", "Comprehend"),
        ("PERSON", "John Smith"),
        ("OTHER", "john.smith@example.com"),
    ] {
        assert!(
            targets
                .iter()
                .any(|(target_type, target)| target_type == entity_type && target == value),
            "case \"named targets\": expected {value:?} targeted as {entity_type}, got \
             {targets:?}"
        );
    }

    // Each target is its own entity: the e-mail address naming the person is not a
    // mention of the person, because the recogniser typed the two differently.
    for value in ["Amazon", "Comprehend", "John Smith", "john.smith@example.com"] {
        let holders = entities
            .iter()
            .filter(|entity| {
                entity["Mentions"]
                    .as_array()
                    .is_some_and(|mentions| {
                        mentions
                            .iter()
                            .any(|mention| mention["Text"].as_str() == Some(value))
                    })
            })
            .count();
        assert_eq!(
            holders, 1,
            "case \"named targets\": expected {value:?} to be mentioned by exactly one \
             entity, got {holders} in {output}"
        );
    }
}
