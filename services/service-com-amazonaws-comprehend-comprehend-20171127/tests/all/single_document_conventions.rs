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
//!    `NOUN` — the token list holds ONE record per word of the document, a word the
//!    document writes unbroken (a mail address, a URL) included, and a token's
//!    confidence is the supported reading's share of the readings its evidence leaves.
//! 4. `DetectPiiEntities` / `ContainsPiiEntities`: a modelled PII type with no closed
//!    form of its own is still placed — a personal NAME the document does not
//!    introduce with a courtesy title, and an ADDRESS over the whole address the
//!    document writes — and `Labels` names the categories the document CONTAINS: a
//!    category it writes a whole value of, at the confidence that settles, and not one
//!    it writes only a component of.
//! 5. `DetectEntities` / `DetectTargetedSentiment`: a company's name followed by a
//!    name of its own is TWO entities (the company, then its product), and targeted
//!    sentiment types every target from that same recogniser, including a named thing
//!    the document writes without a capital.
//! 6. `DetectTargetedSentiment`: a noun phrase that REFERS to nothing is no target —
//!    the predicate a copula states of its subject says what kind of thing the subject
//!    is rather than naming a further thing to direct sentiment at.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    ENTITY_TYPE_VALUES, PART_OF_SPEECH_TAG_TYPE_VALUES, PII_ENTITY_TYPE_VALUES,
    TARGETED_SENTIMENT_ENTITY_TYPE_VALUES, TOXIC_CONTENT_TYPE_VALUES,
};

use crate::comprehend_client::Comprehend;
use crate::toxic_content::PLACEMENT_FLOOR;

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
            // The analysers read a finite segment, so no content type is reported as
            // impossible: every entry of the vector carries a confidence of its own,
            // strictly inside (0, 1].
            assert!(
                *score > 0.0 && *score <= 1.0,
                "case {case:?}: expected a Score strictly inside (0, 1] for {name:?}, \
                 got {score}"
            );
        }

        let toxicity = entry["Toxicity"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Toxicity, got {entry}"));
        // `Toxicity` is the confidence that the segment carries toxic content of ANY
        // type: every placement is independent evidence for it, so it is at least as
        // large as the strongest single type and never larger than the whole vector
        // combined.
        let strongest = scores.iter().copied().fold(0.0f64, f64::max);
        let combined = 1.0 - scores.iter().map(|score| 1.0 - score).product::<f64>();
        assert!(
            toxicity + 1e-6 >= strongest && toxicity <= combined + 1e-6,
            "case {case:?}: expected a Toxicity between the strongest type {strongest} \
             and the combined vector {combined}, got {toxicity} from {scores:?}"
        );
        assert_eq!(
            scores.iter().any(|score| *score > PLACEMENT_FLOOR),
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
        // No reading the lexicon could have MISSED is impossible: every member of the
        // distribution stays strictly positive where the engine found no word for it.
        // MIXED is open only where BOTH signed readings are — it is the confidence the
        // document carries both polarities at once — so it keeps the room the two of
        // them leave together rather than a flat zero.
        for (label, score) in &by_label {
            assert!(
                *score >= 0.0 && *score < 1.0,
                "case {case:?}: expected SentimentScore.{label} inside [0, 1), \
                 got {score}"
            );
            assert!(
                *score > 0.0,
                "case {case:?}: expected SentimentScore.{label}, a reading the \
                 document leaves open, above 0, got {score}"
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

        let mixed = by_label
            .iter()
            .find(|(label, _)| *label == "Mixed")
            .map_or(f64::NAN, |(_, score)| *score);
        if *expected != "MIXED" {
            // Every row but the balanced one carries at most one polarity, and MIXED
            // needs a missed word of EACH, so it keeps the two signed rooms
            // multiplied: above zero, and below either of them.
            let signed = by_label
                .iter()
                .filter(|(label, _)| *label == "Positive" || *label == "Negative")
                .map(|(_, score)| *score)
                .fold(f64::INFINITY, f64::min);
            assert!(
                mixed > 0.0 && mixed < signed,
                "case {case:?}: the document carries at most one polarity, so MIXED \
                 keeps the room the two signed readings leave together — above 0 and \
                 below both — got {mixed} against {signed} in {output}"
            );
        } else {
            assert!(
                mixed > 0.0,
                "case {case:?}: the document carries both polarities, so MIXED is \
                 supported, got {output}"
            );
        }

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

        // The two operations read ONE recogniser: every type the document writes a
        // whole value of is labelled, and every label names a type the entity list
        // places a span of.
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
        for (entity_type, _) in *expected {
            assert!(
                labels.iter().any(|label| label == entity_type),
                "case {case:?}: the document writes a whole {entity_type}, so expected it \
                 among the labels, got {labels:?}"
            );
        }
        assert!(
            labels.iter().all(|label| types.contains(label)),
            "case {case:?}: every label names a type DetectPiiEntities places, got \
             {labels:?} against {types:?}"
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
    // One document naming a company together with a product of its own, a person, and
    // a thing with no capital of its own — the kinds of named target the rule
    // generalises over.
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
        // One named mention, whole: the recogniser types "Amazon" and the product
        // name beside it separately, which is its answer about those CHARACTERS, and
        // the name the document writes is still one mention — typed by the part that
        // says what is named.
        ("ORGANIZATION", "Amazon Comprehend"),
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
    // No part of a name is reported as a mention of its own.
    for fragment in ["Amazon", "Comprehend", "John", "Smith"] {
        assert!(
            !targets.iter().any(|(_, target)| target == fragment),
            "case \"named targets\": {fragment:?} is part of a name, so it is not a \
             mention of its own, got {targets:?}"
        );
    }

    // Each target is its own entity: the e-mail address naming the person is not a
    // mention of the person, because the recogniser typed the two differently.
    for value in ["Amazon Comprehend", "John Smith", "john.smith@example.com"] {
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

// ---------------------------------------------------------------------------
// 6. DetectSyntax: one token per WORD, a word written unbroken included
// ---------------------------------------------------------------------------

/// `(case, document, the (word, tag) pairs the token list must hold as ONE token
/// each)`.
///
/// `SyntaxTokens` is documented as "one syntax token record for each word in the
/// source text", so the rows differ along what makes a word hard to find rather than
/// along the word: the separators a mail address and a URI carry inside them, which
/// the word rules break on, where such a word stands in its sentence, and the
/// punctuation the document writes after it. The last row is the word-internal
/// punctuation the word rules already keep whole, so the table spans both kinds.
const UNBROKEN_WORDS: &[SyntaxRow] = &[
    (
        "a mail address in running text",
        "Write to jane.roe@example.com before noon.",
        &[("jane.roe@example.com", "O")],
    ),
    (
        "a mail address closing a sentence",
        "Write to jane.roe@example.com.",
        // The sentence's own period is a token of its own, not part of the address.
        &[("jane.roe@example.com", "O"), (".", "PUNCT")],
    ),
    (
        "an absolute URI with a path",
        "The dashboard lives at https://example.com/docs/profile for now.",
        &[("https://example.com/docs/profile", "O")],
    ),
    (
        "a scheme-less host name with a path",
        "See www.example.co.uk/help before calling.",
        &[("www.example.co.uk/help", "O")],
    ),
    (
        "a mail address whose local part carries digits and a plus",
        "Mail ops+alerts2@mail.example.com today.",
        &[("ops+alerts2@mail.example.com", "O")],
    ),
    (
        "a mail address whose local part is not ASCII",
        "Mail jos\u{e9}.p@example.com today.",
        &[("jos\u{e9}.p@example.com", "O")],
    ),
    (
        "the word-internal punctuation the word rules keep",
        "The company's invoice for 1,250.50 arrived.",
        &[("company's", "NOUN"), ("1,250.50", "NUM")],
    ),
];

#[tokio::test]
async fn syntax_reports_one_token_per_word_of_the_document() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in UNBROKEN_WORDS {
        let output = comprehend.ok("DetectSyntax", &document(text)).await;
        let tokens = output["SyntaxTokens"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected SyntaxTokens, got {output}"))
            .clone();

        let characters: Vec<char> = text.chars().collect();
        let mut read: Vec<(String, String)> = Vec::new();
        for (position, token) in tokens.iter().enumerate() {
            let begin = token["BeginOffset"].as_u64().unwrap_or_default() as usize;
            let end = token["EndOffset"].as_u64().unwrap_or_default() as usize;
            let slice: String = characters[begin..end.min(characters.len())]
                .iter()
                .collect();
            assert_eq!(
                token["Text"].as_str(),
                Some(slice.as_str()),
                "case {case:?}: a token's Text is the characters its offsets delimit, \
                 got {token}"
            );
            assert_eq!(
                token["TokenId"].as_u64(),
                Some(position as u64 + 1),
                "case {case:?}: TokenId is the token's 1-based position, got {token}"
            );
            read.push((
                slice,
                token["PartOfSpeech"]["Tag"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }

        // Every non-whitespace character of the document belongs to exactly one
        // token, in order: nothing is dropped and no record is emitted past the end
        // of the text.
        let rejoined: String = read.iter().map(|(text, _)| text.as_str()).collect();
        let without_whitespace: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(
            rejoined, without_whitespace,
            "case {case:?}: the tokens cover every non-whitespace character of the \
             document exactly once, got {rejoined:?} for {without_whitespace:?}"
        );

        for (word, tag) in *expected {
            assert!(
                PART_OF_SPEECH_TAG_TYPE_VALUES.contains(tag),
                "case {case:?}: {tag:?} is not a modelled PartOfSpeechTagType value"
            );
            let records: Vec<&(String, String)> =
                read.iter().filter(|(text, _)| text == word).collect();
            assert_eq!(
                records.len(),
                1,
                "case {case:?}: expected {word:?} to come back as exactly one token, \
                 got {read:?}"
            );
            assert_eq!(
                records[0].1, *tag,
                "case {case:?}: expected {word:?} to read {tag:?}, got {:?} in {read:?}",
                records[0].1
            );
        }
    }
}

/// `(case, document, word, whether the position SETTLES the word's class)`.
///
/// `PartOfSpeech.Score` is "the confidence that Amazon Comprehend has that the part of
/// speech was correctly identified", so the rows differ along how much the word's
/// class is in doubt: a word one word class claims and nothing competes for, three
/// words whose class the position they stand in settles, and two whose verbal reading
/// the position prefers against without excluding it — a gerund naming a thing inside a
/// nominal compound is still a verb form, wherever in the compound it stands.
const TOKEN_CONFIDENCES: &[(&str, &str, &str, bool)] = &[
    ("a word one word class claims", "The flight left.", "The", true),
    (
        "a word standing inside a name",
        "The parcel went to Main Street yesterday.",
        "Main",
        true,
    ),
    (
        "a relativizer standing for the noun its clause modifies",
        "The service that uses machine learning is new.",
        "that",
        true,
    ),
    (
        "a copula standing as its clause's only verb",
        "The staff were rude.",
        "were",
        true,
    ),
    (
        "a gerund naming a thing inside a nominal compound",
        "The language processing service is new.",
        "processing",
        false,
    ),
    (
        "a gerund heading a nominal compound",
        "Machine learning finds insights.",
        "learning",
        false,
    ),
];

#[tokio::test]
async fn a_tokens_confidence_is_the_share_of_the_readings_its_evidence_leaves() {
    let comprehend = Comprehend::new();
    let mut settled: Vec<(&str, f64)> = Vec::new();
    let mut ambiguous: Vec<(&str, f64)> = Vec::new();

    for (case, text, word, is_settled) in TOKEN_CONFIDENCES {
        let output = comprehend.ok("DetectSyntax", &document(text)).await;
        let tokens = output["SyntaxTokens"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected SyntaxTokens, got {output}"))
            .clone();
        let token = tokens
            .iter()
            .find(|token| token["Text"].as_str() == Some(*word))
            .unwrap_or_else(|| panic!("case {case:?}: no token {word:?} in {tokens:?}"));
        let score = token["PartOfSpeech"]["Score"].as_f64().unwrap_or(f64::NAN);
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: a confidence is reported inside (0, 1], got {score} in {token}"
        );
        if *is_settled {
            // Nothing is left competing for the reading, so the confidence is at or
            // next to the top of the scale.
            assert!(
                score > 0.99,
                "case {case:?}: the position settles {word:?}, so expected a confidence \
                 at or next to 1, got {score} in {token}"
            );
            settled.push((case, score));
        } else {
            // The other reading is one the word really has here, so it keeps a visible
            // share of the confidence — and the supported reading still leads clearly.
            assert!(
                score > 0.9 && score < 0.99,
                "case {case:?}: {word:?} keeps a live second reading, so expected a \
                 confidence visibly below 1 and still a confident reading, got {score} \
                 in {token}"
            );
            ambiguous.push((case, score));
        }
    }

    // A word only one word class claims, standing where no other reading is open, is
    // identified with nothing competing for the confidence at all.
    let (case, unambiguous) = settled[0];
    assert_eq!(
        unambiguous, 1.0,
        "case {case:?}: one reading and no competitor is a full confidence, got \
         {unambiguous}"
    );
    // The two kinds are DISTINGUISHABLE, which is what a caller filtering on
    // confidence reads: every settled token is reported above every ambiguous one.
    for (settled_case, settled_score) in &settled {
        for (ambiguous_case, ambiguous_score) in &ambiguous {
            assert!(
                settled_score > ambiguous_score,
                "case {settled_case:?} vs {ambiguous_case:?}: expected a settled reading \
                 above an ambiguous one, got {settled_score} against {ambiguous_score}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 7. ContainsPiiEntities: the label set names the categories the document holds
// ---------------------------------------------------------------------------

/// `(case, document, the labels the response must hold, and nothing else)`.
///
/// `EntityLabel` is documented as one of "the label or labels that categorize the
/// personally identifiable information (PII) entity being analyzed", so the rows differ
/// along WHICH categories a document carries and how each is written: a value its own
/// published form spells out, a personal name the recogniser places, a physical address
/// written as the whole line, and two documents naming a place without writing an
/// address at all — a city or a region code is part of what an address is written with,
/// so the document has not written one and the category is not among the labels, even
/// though `DetectPiiEntities` reports the place name it did write.
type PiiLabelRow = (&'static str, &'static str, &'static [&'static str]);
const PII_LABELS: &[PiiLabelRow] = &[
    (
        "a personal name and a mail address",
        "John Smith can be reached at john.smith@example.com.",
        &["EMAIL", "NAME"],
    ),
    (
        "a street address written with its locality line",
        "The parcel went to 42 Oak Avenue, Portland, OR 97201 last week.",
        &["ADDRESS"],
    ),
    (
        "a place name written without a street line",
        "The parcel went to Portland last week.",
        &[],
    ),
    (
        "a city and a region code written without a street line",
        "The team met in Seattle, WA last week.",
        &[],
    ),
    (
        "a mail address alone",
        "Write to ops@example.com now.",
        &["EMAIL"],
    ),
];

#[tokio::test]
async fn a_pii_label_names_a_category_the_document_contains() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in PII_LABELS {
        let output = comprehend.ok("ContainsPiiEntities", &document(text)).await;
        let labels = output["Labels"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Labels, got {output}"))
            .clone();

        let mut read: Vec<String> = labels
            .iter()
            .map(|label| label["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        for name in &read {
            assert!(
                PII_ENTITY_TYPE_VALUES.contains(&name.as_str()),
                "case {case:?}: {name:?} is not a modelled PiiEntityType value"
            );
        }
        read.sort();
        let mut names: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        names.sort();
        assert_eq!(
            read, names,
            "case {case:?}: the labels name exactly the categories the document \
             carries, expected {names:?}, got {labels:?}"
        );

        for name in *expected {
            let score = labels
                .iter()
                .find(|label| label["Name"].as_str() == Some(*name))
                .map(|label| label["Score"].as_f64().unwrap_or(f64::NAN))
                .unwrap_or_else(|| panic!("case {case:?}: no {name} label in {labels:?}"));
            // The question the label answers is whether the document CONTAINS the
            // category, and a document writing a whole value of it — a mail address, a
            // person's name, an address line — answers that outright, whatever the
            // span's own boundaries leave open.
            assert_eq!(
                score, 1.0,
                "case {case:?}: the document writes a whole {name}, so it contains the \
                 category and the label is reported at full confidence, got {score}"
            );
        }

        // The two PII operations read ONE recogniser, so every category the labels
        // name is one the entity list places a span of — the label set is the
        // categories the document CARRIES, which is why a span that is only a
        // component of its type is placed without putting its type in the set.
        let placed = comprehend.ok("DetectPiiEntities", &document(text)).await;
        let mut types: Vec<String> = placed["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entity| entity["Type"].as_str().unwrap_or_default().to_string())
            .collect();
        types.sort();
        types.dedup();
        assert!(
            read.iter().all(|label| types.contains(label)),
            "case {case:?}: every label names a category DetectPiiEntities places a \
             span of, got {read:?} against {types:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 8. DetectTargetedSentiment: a noun phrase that refers to nothing is no target
// ---------------------------------------------------------------------------

/// `(case, document, the thing the document is about, the head of the noun phrase in
/// question, whether that phrase is a mention of an entity)`.
///
/// `TargetedSentimentMention$Text` is "the text in the document that identifies the
/// entity", and the predicate a copula states of its subject identifies no entity: it
/// says what KIND of thing the subject is, so there is nothing there for sentiment to
/// be directed at. The rows differ along the construction the phrase stands in rather
/// than along the phrase — the forms of the copula, and what kind of thing the subject
/// is — and the last row is the control: a phrase of the same shape standing as a
/// clause's object does refer to something, and is a target.
type PredicationRow = (&'static str, &'static str, &'static str, &'static str, bool);
const PREDICATIONS: &[PredicationRow] = &[
    (
        "a company and the kind of thing it is",
        "Amazon Comprehend is a natural language processing service.",
        "Amazon Comprehend",
        "service",
        false,
    ),
    (
        "a person and the role they are",
        "Jane Roe is the project manager.",
        "Jane Roe",
        "manager",
        false,
    ),
    (
        "a past-tense copula",
        "Initech Inc was a small startup.",
        "Initech Inc",
        "startup",
        false,
    ),
    (
        "a noun phrase the clause's verb takes as its object",
        "Amazon Comprehend reads the document.",
        "Amazon Comprehend",
        "document",
        true,
    ),
];

#[tokio::test]
async fn a_noun_phrase_that_refers_to_nothing_is_not_a_targeted_mention() {
    let comprehend = Comprehend::new();

    for (case, text, subject, head, is_mention) in PREDICATIONS {
        let output = comprehend
            .ok("DetectTargetedSentiment", &document(text))
            .await;
        let entities = output["Entities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Entities, got {output}"))
            .clone();

        // Every mention the response carries, with the group it sits in.
        let characters: Vec<char> = text.chars().collect();
        let mut mentions: Vec<(usize, String, String)> = Vec::new();
        for (group, entity) in entities.iter().enumerate() {
            for mention in entity["Mentions"].as_array().cloned().unwrap_or_default() {
                let begin = mention["BeginOffset"].as_u64().unwrap_or_default() as usize;
                let end = mention["EndOffset"].as_u64().unwrap_or_default() as usize;
                let slice: String = characters[begin..end.min(characters.len())]
                    .iter()
                    .collect();
                assert_eq!(
                    mention["Text"].as_str(),
                    Some(slice.as_str()),
                    "case {case:?}: a mention's Text is the characters its offsets \
                     delimit, got {mention}"
                );
                let entity_type = mention["Type"].as_str().unwrap_or_default().to_string();
                assert!(
                    TARGETED_SENTIMENT_ENTITY_TYPE_VALUES.contains(&entity_type.as_str()),
                    "case {case:?}: {entity_type:?} is not a modelled \
                     TargetedSentimentEntityType value"
                );
                mentions.push((group, slice, entity_type));
            }
        }

        // What the document is about is a target whichever row this is.
        let named: Vec<&(usize, String, String)> = mentions
            .iter()
            .filter(|(_, text, _)| text == subject)
            .collect();
        assert_eq!(
            named.len(),
            1,
            "case {case:?}: expected {subject:?} to be targeted exactly once, got \
             {mentions:?}"
        );

        let found: Vec<&(usize, String, String)> =
            mentions.iter().filter(|(_, text, _)| text == head).collect();
        if *is_mention {
            assert_eq!(
                found.len(),
                1,
                "case {case:?}: {head:?} is what the clause is about, so expected it \
                 targeted once, got {mentions:?}"
            );
            assert_ne!(
                found[0].0, named[0].0,
                "case {case:?}: {head:?} is not what {subject:?} is, so expected its own \
                 entity group, got both in group {} in {entities:?}",
                found[0].0
            );
        } else {
            assert!(
                found.is_empty(),
                "case {case:?}: {head:?} heads the predicate the clause states of \
                 {subject:?}, so it identifies no entity and is no target, got \
                 {mentions:?}"
            );
            // Nothing else of that phrase is targeted either: the document writes ONE
            // thing to be about.
            assert_eq!(
                entities.len(),
                1,
                "case {case:?}: the document writes one entity and says what kind of \
                 thing it is, so expected one entity group, got {entities:?}"
            );
        }
    }
}
