// SPDX-License-Identifier: CC0-1.0

//! Comprehend's synchronous inference: `DetectDominantLanguage` and
//! `DetectSentiment`.
//!
//! Both are pure functions of the submitted document, so each suite asserts the
//! RULE over a table of documents rather than one recorded request: `Languages` is
//! the identification engine's ranked confidence distribution over the modelled
//! `LanguageCode` set — the dominant language first, the runner-up candidates after
//! it in descending order of score — and the sentiment is the arg-max of four
//! confidences that sum to 1.
//!
//! The per-document BYTE quota both operations refuse an oversized document with
//! lives in `realtime_text_quotas.rs`.

use serde_json::json;
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    LANGUAGE_CODE_VALUES, SENTIMENT_TYPE_VALUES,
};

use crate::comprehend_client::Comprehend;

/// Every modelled `LanguageCode` paired with a document written in it.
///
/// The language set is the model's own (`LANGUAGE_CODE_VALUES`, asserted below);
/// the issue cited only `fr` and `en`, so every other row is an entry it did not
/// cite. `zh-TW` is absent on purpose: the identification engine does not tell the
/// traditional-script variant of Chinese apart, so it is never reported.
pub(crate) const DOCUMENTS: &[(&str, &str, &str)] = &[
    (
        "arabic",
        "ar",
        "مرحبا، كيف حالك اليوم؟ الطقس في القاهرة جميل جدا في هذا الوقت من السنة.",
    ),
    (
        "german",
        "de",
        "Guten Tag, wie geht es Ihnen heute? Das Wetter in Berlin ist heute sehr schön.",
    ),
    (
        "english",
        "en",
        "Good afternoon, how are you today? The weather in London is rather pleasant.",
    ),
    (
        "spanish",
        "es",
        "Hola, ¿cómo estás hoy? El clima en Madrid es muy agradable en esta época del año.",
    ),
    (
        "french",
        "fr",
        "Bonjour, comment allez-vous aujourd'hui ? Il fait très beau à Paris en ce moment.",
    ),
    (
        "hindi",
        "hi",
        "नमस्ते, आज आप कैसे हैं? दिल्ली का मौसम इस समय बहुत अच्छा है।",
    ),
    (
        "italian",
        "it",
        "Buongiorno, come stai oggi? Il tempo a Roma è molto bello in questo periodo.",
    ),
    (
        "japanese",
        "ja",
        "こんにちは、今日はお元気ですか。東京の天気はとても良いです。",
    ),
    (
        "korean",
        "ko",
        "안녕하세요, 오늘 기분이 어떠세요? 서울의 날씨는 요즘 매우 좋습니다.",
    ),
    (
        "portuguese",
        "pt",
        "Bom dia, como você está hoje? O tempo em Lisboa está muito bom nesta altura.",
    ),
    (
        "chinese",
        "zh",
        "你好，你今天过得怎么样？北京的天气这个时候非常好。",
    ),
];

#[tokio::test]
async fn detect_dominant_language_ranks_the_language_of_the_document() {
    let comprehend = Comprehend::new();

    for (case, expected, text) in DOCUMENTS {
        let output = comprehend
            .ok("DetectDominantLanguage", &json!({ "Text": text }))
            .await;
        let languages = output["Languages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Languages array, got {output}"))
            .clone();

        assert!(
            !languages.is_empty(),
            "case {case:?}: expected at least one detected language, got {output}"
        );
        let top = languages[0]["LanguageCode"].as_str().unwrap_or_default();
        assert_eq!(
            top, *expected,
            "case {case:?}: expected the dominant language {expected:?}, got {top:?} \
             in {languages:?}"
        );
        assert_ranked_distribution(case, &languages);
    }
}

/// Documents the engine cannot place in a SINGLE language, and the language each is
/// predominantly written in.
///
/// The ranked list is observable only on a document the engine ranks more than one
/// candidate for, which is what these rows are: a document carrying two languages in
/// comparable measure, and a fragment too short to carry a language profile at all.
/// The rows differ along what the rule generalises over — which language
/// predominates, how many others appear, whether the minority material is a greeting
/// or a whole clause, and document length.
///
/// A document plainly in ONE language is the other half of the same rule: the engine
/// detects that language and nothing else, so the array carries one entry. That half
/// is asserted by [`a_document_plainly_in_one_language_reports_that_language_alone`]
/// over the whole per-language table.
const MIXED_DOCUMENTS: &[(&str, &str, &str)] = &[
    (
        "english clause beside greetings in five languages",
        "en",
        "Hello 你好 Bonjour Hola Ciao Guten Tag Namaste. I am Bob, a software engineer \
         in Seattle.",
    ),
    (
        "one latin letter",
        // Every Latin-script language the engine was built with is a candidate for a
        // single letter; which one leads is the engine's own ranking, so no dominant
        // language is asserted for this row.
        "",
        "a",
    ),
    (
        "two words, one from each of two languages",
        "",
        "Hola Ciao",
    ),
];

#[tokio::test]
async fn detect_dominant_language_ranks_runner_up_candidates_behind_the_dominant_one() {
    let comprehend = Comprehend::new();

    for (case, expected, text) in MIXED_DOCUMENTS {
        let output = comprehend
            .ok("DetectDominantLanguage", &json!({ "Text": text }))
            .await;
        let languages = output["Languages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Languages array, got {output}"))
            .clone();

        assert!(
            languages.len() >= 2,
            "case {case:?}: a document the engine cannot place in one language has \
             runner-up candidates, so expected at least 2 ranked entries, got \
             {languages:?}"
        );
        if !expected.is_empty() {
            let top = languages[0]["LanguageCode"].as_str().unwrap_or_default();
            assert_eq!(
                top, *expected,
                "case {case:?}: expected the dominant language {expected:?} first, got \
                 {top:?} in {languages:?}"
            );
        }
        assert_ranked_distribution(case, &languages);
    }
}

/// The other half of the rule: a document plainly in one language is answered with
/// THAT language and no runner-up — a hypothesis the engine all but rejected is not a
/// language it detected — and with a confidence strictly below 1, since an
/// identification is never certain.
#[tokio::test]
async fn a_document_plainly_in_one_language_reports_that_language_alone() {
    let comprehend = Comprehend::new();

    for (case, expected, text) in DOCUMENTS {
        let output = comprehend
            .ok("DetectDominantLanguage", &json!({ "Text": text }))
            .await;
        let languages = output["Languages"].as_array().cloned().unwrap_or_default();

        let reported: Vec<&str> = languages
            .iter()
            .map(|entry| entry["LanguageCode"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported,
            vec![*expected],
            "case {case:?}: expected the document's own language alone, got {languages:?}"
        );
        let score = languages[0]["Score"].as_f64().unwrap_or(f64::NAN);
        assert!(
            score < 1.0,
            "case {case:?}: an identification is never certain, so expected a Score \
             below 1, got {score}"
        );
        assert_ranked_distribution(case, &languages);
    }
}

/// The invariants every `Languages` array carries, whatever the document: each
/// entry names a modelled `LanguageCode` and carries a score in (0, 1], the array
/// is sorted in descending order of score (the modelled `Languages` member's own
/// sentence), no language appears twice, and the scores are a distribution over the
/// candidates the engine placed, so they never sum past 1.
fn assert_ranked_distribution(case: &str, languages: &[serde_json::Value]) {
    let mut previous = f64::INFINITY;
    let mut seen: Vec<&str> = Vec::new();
    let mut total = 0.0;
    for entry in languages {
        let code = entry["LanguageCode"].as_str().unwrap_or_default();
        assert!(
            LANGUAGE_CODE_VALUES.contains(&code),
            "case {case:?}: {code:?} is not a modelled LanguageCode value"
        );
        assert!(
            !seen.contains(&code),
            "case {case:?}: {code:?} is ranked twice in {languages:?}"
        );
        seen.push(code);
        let score = entry["Score"].as_f64().unwrap_or(f64::NAN);
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: expected a Score in (0, 1], got {score} for {code:?}"
        );
        assert!(
            score <= previous,
            "case {case:?}: Languages is not sorted by descending Score, got \
             {score} after {previous} in {languages:?}"
        );
        previous = score;
        total += score;
    }
    assert!(
        total <= 1.000_01,
        "case {case:?}: the scores are a distribution over the ranked candidates, so \
         they cannot sum past 1, got {total} from {languages:?}"
    );
}

/// Documents the two honest edge answers: the engine ranks the candidates it can
/// place for any document that carries letters, however weakly, and none at all for
/// one that carries none — rather than an invented candidate.
///
/// A document long enough to carry a language profile always comes back with its
/// own language first — the table above is 11 rows of that — so this pins the other
/// end of the same rule.
#[tokio::test]
async fn detect_dominant_language_reports_a_candidate_only_when_the_engine_places_one() {
    let comprehend = Comprehend::new();

    for (case, text, expect_candidate) in [
        ("one latin letter", "a", true),
        ("one arabic letter", "ب", true),
        ("digits only", "1234567890", false),
        ("punctuation only", "!!! ... ???", false),
    ] {
        let output = comprehend
            .ok("DetectDominantLanguage", &json!({ "Text": text }))
            .await;
        let languages = output["Languages"].as_array().cloned().unwrap_or_default();

        if expect_candidate {
            assert!(
                !languages.is_empty(),
                "case {case:?}: the engine places this letter, so expected a ranked \
                 candidate, got {output}"
            );
            assert_ranked_distribution(case, &languages);
        } else {
            assert!(
                languages.is_empty(),
                "case {case:?}: the engine places no language in a document with no \
                 letters, so no candidate may be reported, got {output}"
            );
        }
    }
}

#[tokio::test]
async fn detect_dominant_language_is_deterministic() {
    let comprehend = Comprehend::new();
    let body = json!({ "Text": DOCUMENTS[4].2 });

    let first = comprehend.ok("DetectDominantLanguage", &body).await;
    let second = comprehend.ok("DetectDominantLanguage", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical call\": expected identical scores, got {first} then {second}"
    );
}

/// Documents and the sentiment the four confidences must make the arg-max.
///
/// The rows differ along what the rule generalises over: one-sided praise, one-sided
/// complaint, a document with no sentiment at all, and one carrying both polarities
/// in balance. Shared with the `batch_sentiment` suite, which asserts that batching
/// these same documents answers the same verdict for each one.
pub(crate) const SENTIMENTS: &[(&str, &str, &str)] = &[
    (
        "praise",
        "POSITIVE",
        "I love this product, it is wonderful!",
    ),
    (
        "complaint",
        "NEGATIVE",
        "This is terrible and I am very unhappy.",
    ),
    (
        "factual",
        "NEUTRAL",
        "The meeting is scheduled for three in the afternoon.",
    ),
    (
        "both polarities",
        "MIXED",
        "Wonderful excellent great terrible awful horrible.",
    ),
];

#[tokio::test]
async fn detect_sentiment_returns_the_arg_max_of_four_confidences_summing_to_one() {
    let comprehend = Comprehend::new();

    for (case, expected, text) in SENTIMENTS {
        let output = comprehend
            .ok(
                "DetectSentiment",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;

        let sentiment = output["Sentiment"].as_str().unwrap_or_default();
        assert!(
            SENTIMENT_TYPE_VALUES.contains(&sentiment),
            "case {case:?}: {sentiment:?} is not a modelled SentimentType value"
        );
        assert_eq!(
            sentiment, *expected,
            "case {case:?}: expected {expected:?}, got {sentiment:?} with {output}"
        );

        let scores = &output["SentimentScore"];
        let by_label: Vec<(&str, f64)> = ["Positive", "Negative", "Neutral", "Mixed"]
            .into_iter()
            .map(|label| {
                let score = scores[label].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: expected SentimentScore.{label}, got {scores}")
                });
                (label, score)
            })
            .collect();

        let total: f64 = by_label.iter().map(|(_, score)| score).sum();
        assert!(
            (total - 1.0).abs() < 1e-5,
            "case {case:?}: expected the four scores to sum to 1.0, got {total} from {scores}"
        );
        for (label, score) in &by_label {
            assert!(
                *score >= 0.0 && *score <= 1.0,
                "case {case:?}: expected SentimentScore.{label} in [0, 1], got {score}"
            );
        }

        // The returned label must be the arg-max of the four it returned, with the
        // same tie-break the handler documents: the first of (Positive, Negative,
        // Neutral, Mixed), which is the order `by_label` is built in, so `reduce`
        // keeping the incumbent resolves a tie exactly as the handler does.
        let winner = by_label
            .iter()
            .copied()
            .reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best })
            .map(|(label, _)| label.to_uppercase())
            .unwrap_or_default();
        assert_eq!(
            winner, sentiment,
            "case {case:?}: Sentiment is not the arg-max of SentimentScore, got \
             {sentiment:?} with {scores}"
        );
    }
}

#[tokio::test]
async fn detect_sentiment_is_deterministic() {
    let comprehend = Comprehend::new();
    let body = json!({ "Text": SENTIMENTS[0].2, "LanguageCode": "en" });

    let first = comprehend.ok("DetectSentiment", &body).await;
    let second = comprehend.ok("DetectSentiment", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical call\": expected identical scores, got {first} then {second}"
    );
}

/// Every value of the modelled `LanguageCode` enum is a language Comprehend infers
/// sentiment in, so none of them is refused. The per-language verdicts, and the
/// refusal of a value outside the enum, are asserted over the whole enum by
/// `sentiment_languages.rs`; this row keeps the acceptance pinned beside the
/// English suites above.
#[tokio::test]
async fn detect_sentiment_analyses_every_modelled_language() {
    let comprehend = Comprehend::new();

    for code in LANGUAGE_CODE_VALUES {
        let (status, body) = comprehend
            .call(
                "DetectSentiment",
                &json!({ "Text": "I love this product, it is wonderful!", "LanguageCode": code }),
            )
            .await;

        assert_eq!(
            status, 200,
            "case {code:?}: a modelled LanguageCode is analysed, expected 200, got \
             {status}: {body}"
        );
        assert!(
            body["Sentiment"].is_string() && body["SentimentScore"].is_object(),
            "case {code:?}: expected a Sentiment and a SentimentScore, got {body}"
        );
    }
}

#[tokio::test]
async fn a_language_code_outside_the_modelled_enum_is_refused() {
    let comprehend = Comprehend::new();

    for code in ["xx", "EN", "english", ""] {
        let (status, body) = comprehend
            .call(
                "DetectSentiment",
                &json!({ "Text": "I love this product!", "LanguageCode": code }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {code:?}: expected 400 for a value outside the LanguageCode enum, \
             got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn an_absent_or_empty_text_is_refused_by_the_modelled_length_floor() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        ("empty Text", json!({ "Text": "", "LanguageCode": "en" })),
        ("absent Text", json!({ "LanguageCode": "en" })),
    ] {
        let (status, response) = comprehend.call("DetectSentiment", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
    }
}
