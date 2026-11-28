// SPDX-License-Identifier: CC0-1.0

//! Which `LanguageCode` Comprehend infers sentiment in.
//!
//! THE RULE, asserted over the model's own `LanguageCode` enum rather than over one
//! recorded call: Comprehend infers sentiment in every one of the primary languages
//! that enum names, so a request carrying any of those values is ANALYSED — HTTP 200
//! with a `Sentiment` and a four-member `SentimentScore` — and only a value outside
//! the enum is refused. The verdict is read from the document itself in that
//! language: a document of praise comes back `POSITIVE`, one of complaint
//! `NEGATIVE`, one of plain fact `NEUTRAL`, and a negated evaluation comes back with
//! its polarity turned around.
//!
//! Batching changes how many documents one request carries, never how a document is
//! scored, so `BatchDetectSentiment` answers the same verdict for the same document
//! in the same language.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    LANGUAGE_CODE_VALUES, SENTIMENT_TYPE_VALUES,
};

use crate::comprehend_client::{error_code, Comprehend};

/// One document, the modelled `LanguageCode` it is written in, and the verdict the
/// four confidences must make the arg-max.
///
/// Every value of the model's `LanguageCode` enum
/// (`smithy_query(query="shape",shape="LanguageCode")`) appears, each with a document
/// of praise and one of complaint, so no value is left to one the suite happens to
/// cite; the rows differ along what the rule generalises over — the language, its
/// script, whether its writing separates words, and whether the document is
/// one-sided praise, one-sided complaint, plain fact or a negated evaluation.
const DOCUMENTS: &[(&str, &str, &str, &str)] = &[
    (
        "arabic praise",
        "ar",
        "هذا المنتج رائع وأنا سعيد جدا",
        "POSITIVE",
    ),
    (
        "arabic complaint",
        "ar",
        "هذا المنتج سيء والخدمة فظيعة",
        "NEGATIVE",
    ),
    (
        "german praise",
        "de",
        "Ich liebe dieses Produkt, es ist wunderbar und sehr nützlich.",
        "POSITIVE",
    ),
    (
        "german complaint",
        "de",
        "Der Service war schrecklich und das Produkt kam kaputt an.",
        "NEGATIVE",
    ),
    (
        "english praise",
        "en",
        "I love this product, it is wonderful!",
        "POSITIVE",
    ),
    (
        "english complaint",
        "en",
        "This is terrible and I am very unhappy.",
        "NEGATIVE",
    ),
    (
        "spanish praise",
        "es",
        "Me encanta este producto, es excelente y muy útil.",
        "POSITIVE",
    ),
    (
        "spanish complaint",
        "es",
        "El servicio fue horrible y el producto llegó roto.",
        "NEGATIVE",
    ),
    (
        "spanish plain fact",
        "es",
        "La reunión está programada para las tres de la tarde.",
        "NEUTRAL",
    ),
    (
        "spanish negated praise",
        "es",
        "El producto no es bueno.",
        "NEGATIVE",
    ),
    (
        "french praise",
        "fr",
        "J'adore ce produit, il est excellent et très utile.",
        "POSITIVE",
    ),
    (
        "french complaint",
        "fr",
        "Le service était horrible et le produit est arrivé cassé.",
        "NEGATIVE",
    ),
    (
        "hindi praise",
        "hi",
        "यह उत्पाद बहुत अच्छा है और मैं खुश हूं",
        "POSITIVE",
    ),
    (
        "hindi complaint",
        "hi",
        "यह उत्पाद खराब है और सेवा भयानक थी",
        "NEGATIVE",
    ),
    (
        "italian praise",
        "it",
        "Adoro questo prodotto, è eccellente e molto utile.",
        "POSITIVE",
    ),
    (
        "italian complaint",
        "it",
        "Il servizio è stato orribile e il prodotto è arrivato rotto.",
        "NEGATIVE",
    ),
    (
        "japanese praise",
        "ja",
        "この製品は素晴らしいです。とても満足しています。",
        "POSITIVE",
    ),
    (
        "japanese complaint",
        "ja",
        "この製品は最悪です。サービスはひどいです。",
        "NEGATIVE",
    ),
    (
        "japanese plain fact",
        "ja",
        "会議は午後三時に予定されています。",
        "NEUTRAL",
    ),
    (
        "korean praise",
        "ko",
        "이 제품은 정말 좋아요. 매우 만족합니다.",
        "POSITIVE",
    ),
    (
        "korean complaint",
        "ko",
        "이 제품은 최악입니다. 서비스가 끔찍했어요.",
        "NEGATIVE",
    ),
    (
        "portuguese praise",
        "pt",
        "Eu amo este produto, é excelente e muito útil.",
        "POSITIVE",
    ),
    (
        "portuguese complaint",
        "pt",
        "O serviço foi horrível e o produto chegou quebrado.",
        "NEGATIVE",
    ),
    (
        "simplified chinese praise",
        "zh",
        "这个产品很好，我非常满意。",
        "POSITIVE",
    ),
    (
        "simplified chinese complaint",
        "zh",
        "这个产品很差，服务糟糕。",
        "NEGATIVE",
    ),
    (
        "simplified chinese negated praise",
        "zh",
        "这个产品不好。",
        "NEGATIVE",
    ),
    (
        "traditional chinese praise",
        "zh-TW",
        "這個產品很好，我非常滿意。",
        "POSITIVE",
    ),
    (
        "traditional chinese complaint",
        "zh-TW",
        "這個產品很差，服務糟糕。",
        "NEGATIVE",
    ),
];

/// The four members of the modelled `SentimentScore`.
const SCORE_MEMBERS: [&str; 4] = ["Positive", "Negative", "Neutral", "Mixed"];

/// Assert the invariants of one reported distribution: every modelled member is
/// present, each in `[0, 1]`, and the four sum to 1.
fn assert_distribution(case: &str, scores: &Value) {
    let mut total = 0.0;
    for member in SCORE_MEMBERS {
        let score = scores[member].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected SentimentScore.{member}, got {scores}")
        });
        assert!(
            (0.0..=1.0).contains(&score),
            "case {case:?}: expected SentimentScore.{member} in [0, 1], got {score}"
        );
        total += score;
    }
    assert!(
        (total - 1.0).abs() < 1e-5,
        "case {case:?}: expected the four confidences to sum to 1.0, got {total} from {scores}"
    );
}

/// Every modelled `LanguageCode` has a document in the table above, so a value added
/// to the enum cannot be left without an engine that reads it: a language with no
/// polarity resources would read every document as carrying no sentiment, which this
/// row is what makes loud.
#[test]
fn every_modelled_language_has_a_document_in_the_table() {
    for code in LANGUAGE_CODE_VALUES {
        let signed: Vec<&str> = DOCUMENTS
            .iter()
            .filter(|(_, language_code, _, _)| language_code == code)
            .map(|(_, _, _, expected)| *expected)
            .filter(|expected| *expected != "NEUTRAL")
            .collect();
        assert!(
            signed.contains(&"POSITIVE") && signed.contains(&"NEGATIVE"),
            "case {code:?}: the table must carry a document of praise and one of \
             complaint written in this language, got {signed:?}"
        );
    }
}

#[tokio::test]
async fn every_modelled_language_is_analysed_and_read_from_the_document() {
    let comprehend = Comprehend::new();

    for (case, language_code, text, expected) in DOCUMENTS {
        let (status, response) = comprehend
            .call(
                "DetectSentiment",
                &json!({ "Text": text, "LanguageCode": language_code }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {language_code:?} is a modelled LanguageCode, expected 200, \
             got {status}: {response}"
        );

        let sentiment = response["Sentiment"].as_str().unwrap_or_default();
        assert!(
            SENTIMENT_TYPE_VALUES.contains(&sentiment),
            "case {case:?}: {sentiment:?} is not a modelled SentimentType value, in \
             {response}"
        );
        assert_eq!(
            sentiment, *expected,
            "case {case:?}: expected {expected:?}, got {sentiment:?} with {response}"
        );
        assert_distribution(case, &response["SentimentScore"]);
    }
}

/// The other half of the rule: no value INSIDE the enum is refused for its language,
/// whatever the document says — asserted over the whole enum, including the values
/// the table above has no document for in that language.
#[tokio::test]
async fn no_modelled_language_is_refused() {
    let comprehend = Comprehend::new();

    for code in LANGUAGE_CODE_VALUES {
        let (status, response) = comprehend
            .call(
                "DetectSentiment",
                &json!({ "Text": "I love this product, it is wonderful!", "LanguageCode": code }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {code:?}: expected 200 for a modelled LanguageCode, got {status}: \
             {response}"
        );
        assert!(
            response["Sentiment"].is_string(),
            "case {code:?}: expected a Sentiment, got {response}"
        );
        assert_distribution(code, &response["SentimentScore"]);
    }
}

/// A value OUTSIDE the enum is still refused, at the request-validation boundary the
/// model drives.
#[tokio::test]
async fn a_language_outside_the_modelled_enum_is_still_refused() {
    let comprehend = Comprehend::new();

    // A code of a language Comprehend does not name (Dutch), the right code in the
    // wrong case, a language NAME rather than a code, a regional variant the enum
    // does not carry, and no value at all.
    for code in ["nl", "EN", "english", "es-MX", ""] {
        let (status, response) = comprehend
            .call(
                "DetectSentiment",
                &json!({ "Text": "Me encanta este producto.", "LanguageCode": code }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {code:?}: expected 400 for a value outside the LanguageCode enum, got \
             {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {code:?}: expected the boundary validator's ValidationException, got \
             {response}"
        );
        assert!(
            response["Sentiment"].is_null() && response["SentimentScore"].is_null(),
            "case {code:?}: a refused request reports no inference, got {response}"
        );
    }
}

/// One document scored alone and in a batch answers the same verdict, in every
/// language: there is ONE scorer behind both operations.
#[tokio::test]
async fn a_batch_answers_the_same_verdict_per_document_in_every_language() {
    let comprehend = Comprehend::new();

    for code in LANGUAGE_CODE_VALUES {
        let batch: Vec<&str> = DOCUMENTS
            .iter()
            .filter(|(_, language_code, _, _)| language_code == code)
            .map(|(_, _, text, _)| *text)
            .collect();
        if batch.is_empty() {
            continue;
        }

        let batched = comprehend
            .ok(
                "BatchDetectSentiment",
                &json!({ "TextList": batch, "LanguageCode": code }),
            )
            .await;
        let results = batched["ResultList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            batch.len(),
            "case {code:?}: expected one result per document, got {batched}"
        );

        for (position, text) in batch.iter().enumerate() {
            let alone = comprehend
                .ok(
                    "DetectSentiment",
                    &json!({ "Text": text, "LanguageCode": code }),
                )
                .await;
            let case = format!("{code}: document {position}");
            assert_eq!(
                results[position]["Sentiment"], alone["Sentiment"],
                "case {case:?}: expected the single-document verdict \
                 {:?}, got {:?}",
                alone["Sentiment"], results[position]["Sentiment"]
            );
            assert_eq!(
                results[position]["SentimentScore"], alone["SentimentScore"],
                "case {case:?}: expected the single-document confidences {:?}, got {:?}",
                alone["SentimentScore"], results[position]["SentimentScore"]
            );
        }
    }
}
