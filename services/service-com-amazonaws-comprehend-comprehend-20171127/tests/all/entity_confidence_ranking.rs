// SPDX-License-Identifier: CC0-1.0

//! How `Entity.Score` RANKS the detections of one document, and what the ranking does
//! NOT depend on.
//!
//! The modelled `Entity$Score` is "the level of confidence that Amazon Comprehend has
//! in the accuracy of the detection", so it belongs to that one detection and is the
//! supported reading's share of everything that detection's own evidence leaves open.
//! Two consequences a client depends on — it thresholds detections on the value and
//! picks "the most confident mention of this document" by comparing two of them — and
//! neither is a property of the response:
//!
//! 1. Inside ONE document, a mention whose evidence accounts for its own characters
//!    whole is reported ABOVE one whose evidence accounts for only part of its
//!    characters, whatever the two mentions' `Type`s, whichever is written longer, and
//!    whichever of them the document (and so the `Entities` list) puts first. A score
//!    read out of the category, out of the rule class, or out of the position in the
//!    output list could not order a pair this way in both writings of the same pair.
//! 2. A document's detections carry the same confidences, and so the same ranking,
//!    wherever the document rides in the batch: the answer is a function of the
//!    document, so moving it from the end of `TextList` to the front cannot move a
//!    score.
//!
//! The rows span what rule 1 generalises over — the kind of evidence that accounts for
//! the whole mention (a closed calendar form, a place the lexicon spells out, an amount
//! written with its currency, a company the lexicon names), the kind that accounts for
//! only part of one (a three-character legal-form designator inside a fourteen-character
//! name, orthography that says a span is a name and nothing that says which kind), and
//! the `Type` on each side of the pair, so `ORGANIZATION` appears once as the better
//! evidenced mention and twice as the worse evidenced one.

use serde_json::{json, Value};

use crate::batch_inference_calibration::{batch_results, distinct};
use crate::comprehend_client::Comprehend;

/// `(case, the document writing the pair one way, the same pair written the other way,
/// the mention the evidence accounts for whole, its Type, the mention the evidence
/// accounts for in part, its Type, why the evidence ranks them that way)`.
type RankedPair = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const RANKED_PAIRS: &[RankedPair] = &[
    (
        "a calendar form against a name only a designator places",
        "The outage began on 2024-01-15 and Acme Widgets LLC filed the papers.",
        "Acme Widgets LLC filed the papers, and the outage began on 2024-01-15.",
        "2024-01-15",
        "DATE",
        "Acme Widgets LLC",
        "ORGANIZATION",
        "the calendar form fixes every character it spans, while the designator names \
         three of the name's fourteen and leaves the rest unaccounted for",
    ),
    (
        "a place the lexicon spells out against a phrase no rule places",
        "They expanded into Portugal, and the Zorblax Initiative published its findings.",
        "The Zorblax Initiative published its findings, and they expanded into Portugal.",
        "Portugal",
        "LOCATION",
        "Zorblax Initiative",
        "OTHER",
        "the lexicon spells the place name out, while the longer span carries only the \
         orthography that says it is a name of some kind",
    ),
    (
        "an amount written with its currency against a name only a designator places",
        "Acme Widgets LLC paid $100,000 for the parcel.",
        "The sum of $100,000 was paid by Acme Widgets LLC.",
        "$100,000",
        "QUANTITY",
        "Acme Widgets LLC",
        "ORGANIZATION",
        "the amount's currency mark and group separators fix its characters, while the \
         designator leaves most of the name's unaccounted for",
    ),
    (
        "a company the lexicon names against a phrase no rule places",
        "The filing names Amazon as the buyer of the Zorblax Initiative.",
        "The Zorblax Initiative was bought by Amazon in the filing.",
        "Amazon",
        "ORGANIZATION",
        "Zorblax Initiative",
        "OTHER",
        "the lexicon names the company outright, while nothing narrows what kind of \
         name the other span is — so the better evidenced mention of this pair carries \
         the Type the worse evidenced mention of the pairs above carries",
    ),
];

/// Every entity each document of one batch was answered with, in document order.
async fn batched_entities(comprehend: &Comprehend, documents: &[&str]) -> Vec<Vec<Value>> {
    let results = batch_results(
        comprehend,
        "BatchDetectEntities",
        &json!({ "TextList": documents, "LanguageCode": "en" }),
    )
    .await;
    results
        .iter()
        .map(|result| result["Entities"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// Where in `found` the entity reading `mention` sits, and the `Score` it carries,
/// asserted to be the one entity reading those characters, to carry `entity_type` and
/// to be a confidence (strictly inside `(0, 1)`).
fn ranked(case: &str, found: &[Value], mention: &str, entity_type: &str) -> (usize, f64) {
    let matching: Vec<(usize, &Value)> = found
        .iter()
        .enumerate()
        .filter(|(_, entity)| entity["Text"].as_str() == Some(mention))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one entity reading {mention:?}, got {found:?}"
    );
    let (position, entity) = matching[0];
    assert_eq!(
        entity["Type"].as_str(),
        Some(entity_type),
        "case {case:?}: expected {mention:?} to be typed {entity_type}, got {entity}"
    );
    let score = entity["Score"].as_f64().unwrap_or(f64::NAN);
    assert!(
        score > 0.0 && score < 1.0,
        "case {case:?}: expected a Score strictly inside (0, 1) for {mention:?}, got \
         {score} in {entity}"
    );
    (position, score)
}

#[tokio::test]
async fn the_better_evidenced_mention_of_a_document_outranks_the_worse_evidenced_one() {
    let comprehend = Comprehend::new();
    let documents: Vec<&str> = RANKED_PAIRS
        .iter()
        .flat_map(|(_, one_way, other_way, ..)| [*one_way, *other_way])
        .collect();
    let answers = batched_entities(&comprehend, &documents).await;

    for (row, (case, _, _, whole, whole_type, partial, partial_type, reason)) in
        RANKED_PAIRS.iter().enumerate()
    {
        // The SAME pair of mentions, written in both orders, so the only thing that
        // changes between the two documents is which of them the `Entities` list puts
        // first.
        let mut whole_positions = Vec::new();
        for (writing, document_index) in [row * 2, row * 2 + 1].into_iter().enumerate() {
            let case = format!("{case} (writing {writing})");
            let found = &answers[document_index];
            let (whole_position, whole_score) = ranked(&case, found, whole, whole_type);
            let (partial_position, partial_score) = ranked(&case, found, partial, partial_type);

            assert!(
                whole_score > partial_score,
                "case {case:?}: expected {whole:?} to be scored above {partial:?} \
                 ({reason}), got {whole_score} against {partial_score}"
            );
            assert_eq!(
                distinct([whole_score, partial_score]),
                2,
                "case {case:?}: expected {whole:?} and {partial:?} to be scored from \
                 their own evidence, got the single value {whole_score}"
            );
            assert_ne!(
                whole_position, partial_position,
                "case {case:?}: expected two entities, got one at position \
                 {whole_position} in {found:?}"
            );
            whole_positions.push(whole_position);
        }

        // The two writings really do put the better evidenced mention in two different
        // places of the `Entities` list, so the ranking above is not the output order
        // under another name.
        assert_ne!(
            whole_positions[0], whole_positions[1],
            "case {case:?}: the two writings are meant to swap the mentions, so expected \
             {whole:?} to be reported at two different positions, got \
             {whole_positions:?}"
        );
    }
}

#[tokio::test]
async fn a_documents_scores_do_not_depend_on_where_it_rides_in_the_batch() {
    let comprehend = Comprehend::new();
    let documents: Vec<&str> = RANKED_PAIRS
        .iter()
        .flat_map(|(_, one_way, other_way, ..)| [*one_way, *other_way])
        .collect();

    let in_order = batched_entities(&comprehend, &documents).await;

    let mut reversed_documents = documents.clone();
    reversed_documents.reverse();
    let reversed = batched_entities(&comprehend, &reversed_documents).await;

    for (position, document) in documents.iter().enumerate() {
        let case = format!("{document:?} at index {position}");

        // The same document read from the other end of the same batch.
        let from_the_other_end = &reversed[documents.len() - 1 - position];
        assert_eq!(
            &in_order[position], from_the_other_end,
            "case {case:?}: a score belongs to the detection and not to the position of \
             its document, so expected the same entities whichever end of the batch the \
             document rides at, got {:?} against {from_the_other_end:?}",
            in_order[position]
        );

        // And read alone, as the only document of its batch.
        let alone = batched_entities(&comprehend, &[document]).await;
        assert_eq!(
            &in_order[position], &alone[0],
            "case {case:?}: expected the document submitted on its own to be answered \
             exactly as it is inside a batch, got {:?} against {:?}",
            in_order[position], alone[0]
        );
    }
}
