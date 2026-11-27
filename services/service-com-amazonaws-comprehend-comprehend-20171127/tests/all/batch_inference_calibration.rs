// SPDX-License-Identifier: CC0-1.0

//! What `BatchDetectDominantLanguage`, `BatchDetectSentiment` and `BatchDetectSyntax`
//! report for a document, as a CONVENTION rather than as one recorded answer.
//!
//! Each suite drives the batched operation — one `ResultList` entry per document, so
//! every row of a table rides in a batch — and asserts the rule that operation's own
//! analyser follows:
//!
//! 1. `BatchDetectDominantLanguage`: `Languages` carries the candidates the
//!    identification engine DETECTED, ranked, each with a confidence strictly inside
//!    `(0, 1)` that follows the engine's own margin — a document plainly in one
//!    language is answered with that language alone, one the engine cannot place is
//!    answered with the candidates it ranked, and neither is answered with a
//!    certainty.
//! 2. `BatchDetectSentiment`: the four confidences are a distribution over the four
//!    modelled classes — they sum to 1, none of them is a flat zero, `MIXED` is the
//!    one reported for a document carrying both polarities, and the confidence follows
//!    how much sentiment-bearing material the document holds.
//! 3. `BatchDetectSyntax`: a tag is the reading the token's own evidence and the
//!    context it stands in support, so the same word in two contexts can come back
//!    with two tags, and the confidence is FULL only where no competing reading of the
//!    token is left open — a token one of whose readings stays live is reported below
//!    it.
//!
//! The fourth analyser of the family, `BatchDetectKeyPhrases`, is held in
//! `noun_phrase_key_phrases.rs`, which reads this suite's batch helpers.
//!
//! Every table spans what its rule generalises over — language, document length,
//! clarity, the word classes present, the construction a word stands in, the polarity
//! balance and the amount of evidence — rather than the one recorded request.

use serde_json::{json, Value};

use crate::comprehend_client::Comprehend;

/// The `ResultList` of one batched call, asserted to carry one entry per document at
/// that document's own index.
///
/// Shared with `noun_phrase_key_phrases.rs`: every suite of the `BatchDetect*` family
/// reads a result list the same way.
pub(crate) async fn batch_results(
    comprehend: &Comprehend,
    operation: &str,
    body: &Value,
) -> Vec<Value> {
    let output = comprehend.ok(operation, body).await;
    let results = output["ResultList"]
        .as_array()
        .unwrap_or_else(|| panic!("{operation}: expected a ResultList array, got {output}"))
        .clone();
    for (position, entry) in results.iter().enumerate() {
        assert_eq!(
            entry["Index"].as_u64(),
            Some(position as u64),
            "{operation}: expected the entry at position {position} to carry that Index, \
             got {output}"
        );
    }
    results
}

/// How many DISTINCT values a table of scores holds: a confidence that follows the
/// document cannot come back as one repeated constant.
pub(crate) fn distinct(scores: impl IntoIterator<Item = f64>) -> usize {
    let mut values: Vec<String> = scores.into_iter().map(|score| score.to_string()).collect();
    values.sort();
    values.dedup();
    values.len()
}

// ---------------------------------------------------------------------------
// 1. BatchDetectDominantLanguage
// ---------------------------------------------------------------------------

/// How many candidates a document's own clarity leaves.
#[derive(Debug, PartialEq, Eq)]
enum Candidates {
    /// Exactly one: the engine detected one language and rejected every other.
    OnlyTheDominantOne,
    /// More than one: the engine could not place the document in a single language.
    Ranked,
}

/// `(case, document, the language the document is plainly in, how many candidates its
/// clarity leaves)`.
///
/// The expected language is empty for a document no single language dominates, where
/// the ranking is the engine's own. The rows differ along language and script,
/// document length and how plainly one language dominates: a full sentence, a short
/// sentence, a document carrying two languages in comparable measure, and a fragment
/// too short to carry a language profile at all.
type LanguageRow = (&'static str, &'static str, &'static str, Candidates);

const LANGUAGE_DOCUMENTS: &[LanguageRow] = &[
    (
        "a full english sentence",
        "Good afternoon, how are you today? The weather in London is rather pleasant.",
        "en",
        Candidates::OnlyTheDominantOne,
    ),
    (
        "a short english sentence",
        "The flight was delayed and the staff were rude.",
        "en",
        Candidates::OnlyTheDominantOne,
    ),
    (
        "a full german sentence",
        "Guten Tag, wie geht es Ihnen heute? Das Wetter in Berlin ist heute sehr schön.",
        "de",
        Candidates::OnlyTheDominantOne,
    ),
    (
        "a japanese sentence",
        "こんにちは、今日はお元気ですか。東京の天気はとても良いです。",
        "ja",
        Candidates::OnlyTheDominantOne,
    ),
    (
        "an english clause beside greetings in five languages",
        "Hello 你好 Bonjour Hola Ciao Guten Tag Namaste. I am Bob, a software engineer \
         in Seattle.",
        "en",
        Candidates::Ranked,
    ),
    ("one latin letter", "a", "", Candidates::Ranked),
    (
        "two words of two languages",
        "Hola Ciao",
        "",
        Candidates::Ranked,
    ),
];

/// Every `Languages` array's invariants, and the leading candidate's score.
fn assert_ranked(case: &str, languages: &[Value]) -> f64 {
    let mut previous = f64::INFINITY;
    let mut total = 0.0;
    let mut seen: Vec<&str> = Vec::new();
    for entry in languages {
        let code = entry["LanguageCode"].as_str().unwrap_or_default();
        assert!(
            !seen.contains(&code),
            "case {case:?}: {code:?} is ranked twice in {languages:?}"
        );
        seen.push(code);
        let score = entry["Score"].as_f64().unwrap_or(f64::NAN);
        // An identification is a confidence, never a certainty: a client thresholding
        // on it must be able to tell a confident detection from a guess.
        assert!(
            score > 0.0 && score < 1.0,
            "case {case:?}: expected a Score strictly inside (0, 1), got {score} for \
             {code:?} in {languages:?}"
        );
        assert!(
            score <= previous,
            "case {case:?}: Languages must be sorted by descending Score, got {score} \
             after {previous} in {languages:?}"
        );
        previous = score;
        total += score;
    }
    assert!(
        total <= 1.000_01,
        "case {case:?}: the scores are a distribution over the candidates, so they \
         cannot sum past 1, got {total} from {languages:?}"
    );
    languages
        .first()
        .and_then(|entry| entry["Score"].as_f64())
        .unwrap_or(f64::NAN)
}

#[tokio::test]
async fn every_identified_candidate_is_one_the_engine_detected() {
    let comprehend = Comprehend::new();
    let text_list: Vec<&str> = LANGUAGE_DOCUMENTS
        .iter()
        .map(|(_, document, _, _)| *document)
        .collect();

    // The whole table in ONE batch, so each row is answered as a `ResultList` entry
    // beside the others, as the recorded request's documents were.
    let results = batch_results(
        &comprehend,
        "BatchDetectDominantLanguage",
        &json!({ "TextList": text_list }),
    )
    .await;

    let mut leading: Vec<(&str, f64)> = Vec::new();
    for (position, (case, _, expected, candidates)) in LANGUAGE_DOCUMENTS.iter().enumerate() {
        let languages = results[position]["Languages"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !languages.is_empty(),
            "case {case:?}: expected the engine's candidates, got {:?}",
            results[position]
        );
        let top = assert_ranked(case, &languages);
        leading.push((case, top));

        match candidates {
            Candidates::OnlyTheDominantOne => assert_eq!(
                languages.len(),
                1,
                "case {case:?}: a document plainly in one language is answered with \
                 that language alone, got {languages:?}"
            ),
            Candidates::Ranked => assert!(
                languages.len() >= 2,
                "case {case:?}: a document the engine cannot place in one language \
                 carries the candidates it ranked, so expected at least 2, got \
                 {languages:?}"
            ),
        }
        if !expected.is_empty() {
            assert_eq!(
                languages[0]["LanguageCode"].as_str(),
                Some(*expected),
                "case {case:?}: expected the dominant language {expected:?} first, got \
                 {languages:?}"
            );
        }
    }

    // The confidence FOLLOWS the document: a fragment the engine cannot place scores
    // far below a sentence it can, and the table does not come back carrying one
    // repeated constant.
    let spread = distinct(leading.iter().map(|(_, score)| *score));
    assert!(
        spread >= 3,
        "expected the leading confidence to follow each document's own clarity, got \
         {spread} distinct values across {leading:?}"
    );
    let score_of = |wanted: &str| -> f64 {
        leading
            .iter()
            .find(|(case, _)| *case == wanted)
            .map(|(_, score)| *score)
            .unwrap_or_else(|| panic!("no leading score for case {wanted:?}"))
    };
    let sentence = score_of("a full english sentence");
    let fragment = score_of("one latin letter");
    assert!(
        fragment < sentence,
        "expected a fragment the engine cannot place to score below a sentence it \
         can, got {fragment} against {sentence}"
    );
    // How MUCH text the identification was made from is part of the evidence, so a
    // document carrying little of it is reported materially below one carrying a lot,
    // and the branch a client takes when it cannot trust the detected language is
    // reachable. A whole sentence of one language is past the gate; two words that
    // two languages both write are not.
    let short = score_of("two words of two languages");
    assert!(
        short < LOW_EVIDENCE_GATE,
        "expected a two-word document to be reported below the {LOW_EVIDENCE_GATE} a \
         client trusts a detected language above, got {short}"
    );
    for case in [
        "a full english sentence",
        "a full german sentence",
        "a japanese sentence",
    ] {
        assert!(
            score_of(case) > LOW_EVIDENCE_GATE,
            "case {case:?}: expected a document carrying a whole sentence of one \
             language to be reported above {LOW_EVIDENCE_GATE}, got {}",
            score_of(case)
        );
    }
}

/// The confidence a client stops trusting a detected language below: a document that
/// leaves the identification this much doubt is the one whose fallback branch has to
/// be reachable.
const LOW_EVIDENCE_GATE: f64 = 0.9;

// ---------------------------------------------------------------------------
// 2. BatchDetectSentiment
// ---------------------------------------------------------------------------

/// `(case, document, the modelled SentimentType the four confidences must make the
/// arg-max)`.
///
/// The rows differ along polarity, how much sentiment-bearing material the document
/// holds, and how the two polarities balance: one-sided praise and complaint with one
/// word of evidence and with several, a document of plain fact, and documents carrying
/// both polarities — joined by a contrastive conjunction, and in a run.
const SENTIMENT_DOCUMENTS: &[(&str, &str, &str)] = &[
    ("one positive word", "The screen is bright.", "POSITIVE"),
    (
        "several positive words",
        "I love this product, it is wonderful and the battery is excellent.",
        "POSITIVE",
    ),
    (
        "one negative word",
        "The seat was uncomfortable.",
        "NEGATIVE",
    ),
    (
        "several negative words",
        "This is terrible and awful and I am very unhappy.",
        "NEGATIVE",
    ),
    (
        "plain fact",
        "The meeting is scheduled for three in the afternoon.",
        "NEUTRAL",
    ),
    (
        "both polarities, joined by a contrast",
        "The food was delicious but the service was terrible.",
        "MIXED",
    ),
    (
        "both polarities, in a run",
        "Wonderful excellent great terrible awful horrible.",
        "MIXED",
    ),
];

/// The four modelled members of `SentimentScore`, in the order the service's own
/// tie-break reads them.
const SENTIMENT_MEMBERS: &[&str] = &["Positive", "Negative", "Neutral", "Mixed"];

#[tokio::test]
async fn the_four_sentiment_confidences_are_a_distribution_the_document_decides() {
    let comprehend = Comprehend::new();
    let text_list: Vec<&str> = SENTIMENT_DOCUMENTS
        .iter()
        .map(|(_, document, _)| *document)
        .collect();
    let results = batch_results(
        &comprehend,
        "BatchDetectSentiment",
        &json!({ "TextList": text_list, "LanguageCode": "en" }),
    )
    .await;

    let mut by_case: Vec<(&str, Vec<(&str, f64)>)> = Vec::new();
    for (position, (case, _, expected)) in SENTIMENT_DOCUMENTS.iter().enumerate() {
        let entry = &results[position];
        let scores: Vec<(&str, f64)> = SENTIMENT_MEMBERS
            .iter()
            .map(|member| {
                let score = entry["SentimentScore"][member].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: expected SentimentScore.{member}, got {entry}")
                });
                (*member, score)
            })
            .collect();

        let total: f64 = scores.iter().map(|(_, score)| score).sum();
        assert!(
            (total - 1.0).abs() < 1e-5,
            "case {case:?}: the four confidences are a distribution, so they sum to 1, \
             got {total} from {entry}"
        );
        // No reading the lexicon could have MISSED is impossible: a client
        // thresholding on any member must read a small confidence where the engine
        // found no word for it, not a flat zero. MIXED is open only where BOTH signed
        // readings are — it is the confidence the document carries both polarities at
        // once — so it keeps the room the two of them leave together.
        for (member, score) in &scores {
            assert!(
                *score >= 0.0 && *score < 1.0,
                "case {case:?}: expected SentimentScore.{member} inside \
                 [0, 1), got {score} in {entry}"
            );
            assert!(
                *score > 0.0,
                "case {case:?}: expected SentimentScore.{member}, a reading the \
                 document leaves open, above 0, got {score} in {entry}"
            );
        }
        let winner = scores
            .iter()
            .copied()
            .reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best })
            .map(|(member, _)| member.to_uppercase())
            .unwrap_or_default();
        assert_eq!(
            winner,
            entry["Sentiment"].as_str().unwrap_or_default(),
            "case {case:?}: Sentiment must be the arg-max of the four confidences, got \
             {entry}"
        );
        assert_eq!(
            entry["Sentiment"].as_str(),
            Some(*expected),
            "case {case:?}: expected {expected:?}, got {entry}"
        );
        by_case.push((case, scores));
    }

    // The confidence follows HOW MUCH sentiment-bearing material the document holds:
    // one word leaves real doubt, several leave a classifier's confidence.
    let confidence = |case: &str, member: &str| -> f64 {
        by_case
            .iter()
            .find(|(name, _)| *name == case)
            .and_then(|(_, scores)| {
                scores
                    .iter()
                    .find(|(name, _)| *name == member)
                    .map(|(_, score)| *score)
            })
            .unwrap_or_else(|| panic!("no {member} confidence for case {case:?}"))
    };
    for (one, several, member) in [
        ("one positive word", "several positive words", "Positive"),
        ("one negative word", "several negative words", "Negative"),
    ] {
        assert!(
            confidence(one, member) < confidence(several, member),
            "expected {member} on {several:?} to be reported above {one:?}, got {} \
             against {}",
            confidence(several, member),
            confidence(one, member)
        );
    }
    // A document carrying both polarities puts its mass on MIXED rather than on the
    // stronger of the two directions.
    for case in [
        "both polarities, joined by a contrast",
        "both polarities, in a run",
    ] {
        let mixed = confidence(case, "Mixed");
        for member in ["Positive", "Negative", "Neutral"] {
            assert!(
                mixed > confidence(case, member),
                "case {case:?}: expected Mixed above {member}, got {mixed} against {}",
                confidence(case, member)
            );
        }
    }

    // The distribution CONCENTRATES on the verdict rather than splitting between it
    // and the runner-up: a client gating a decision on the member it acted on reads a
    // classifier's confidence there, and the readings the document carries nothing for
    // stay far below any threshold it could set.
    for (case, _, expected) in SENTIMENT_DOCUMENTS {
        let verdict = member_of(expected);
        let leading = confidence(case, verdict);
        assert!(
            leading > VERDICT_CONCENTRATION,
            "case {case:?}: expected the reported {expected} sentiment to carry more \
             than {VERDICT_CONCENTRATION} of the distribution, got {leading}"
        );
        for member in SENTIMENT_MEMBERS {
            if *member == verdict {
                continue;
            }
            assert!(
                confidence(case, member) < 1.0 - VERDICT_CONCENTRATION,
                "case {case:?}: expected {member} below the verdict's residual, got {}",
                confidence(case, member)
            );
        }
    }
    // A document the engine read as one-sided leaves the opposite polarity negligible
    // rather than at a magnitude a threshold would act on.
    for (case, opposite) in [
        ("several positive words", "Negative"),
        ("several negative words", "Positive"),
        ("plain fact", "Mixed"),
    ] {
        assert!(
            confidence(case, opposite) < UNSUPPORTED_CLASS_CEILING,
            "case {case:?}: expected {opposite}, which the document carries no \
             evidence for, below {UNSUPPORTED_CLASS_CEILING}, got {}",
            confidence(case, opposite)
        );
    }
    // MIXED is the confidence the document carries BOTH polarities, so it is open only
    // where both signed readings are: a document the engine read as carrying at most
    // one polarity keeps the room the two of them leave TOGETHER — the two signed rooms
    // multiplied, which is above zero and below either — rather than a flat zero no
    // log-scale comparison between documents survives.
    for case in [
        "one positive word",
        "several positive words",
        "one negative word",
        "several negative words",
        "plain fact",
    ] {
        let mixed = confidence(case, "Mixed");
        let signed = confidence(case, "Positive").min(confidence(case, "Negative"));
        assert!(
            mixed > 0.0 && mixed < signed,
            "case {case:?}: the document carries at most one polarity, so MIXED keeps \
             the room the two signed readings leave together — above 0 and below both — \
             got {mixed} against {signed}"
        );
    }
}

/// How much of the distribution the reported `Sentiment` has to carry: past the 0.95
/// a client gates an automated decision on, which a verdict sharing its document with
/// a runner-up of comparable magnitude cannot clear.
const VERDICT_CONCENTRATION: f64 = 0.95;

/// What a class the document carries NO evidence for may be reported at: the prior
/// every hypothesis keeps and nothing more, which is orders below any threshold a
/// client sets.
const UNSUPPORTED_CLASS_CEILING: f64 = 1e-4;

/// The `SentimentScore` member carrying the confidence of a modelled `SentimentType`.
fn member_of(sentiment: &str) -> &'static str {
    SENTIMENT_MEMBERS
        .iter()
        .find(|member| member.to_uppercase() == sentiment)
        .copied()
        .unwrap_or_else(|| panic!("{sentiment:?} is not a modelled SentimentType value"))
}

// ---------------------------------------------------------------------------
// 3. BatchDetectSyntax
// ---------------------------------------------------------------------------

/// `(case, document, [(the token, the tag it must come back with)])`.
///
/// The rows differ along the construction a word stands in rather than along the word:
/// the same forms of `be` and the same participles appear as an auxiliary and as a main
/// verb, as a predicate's adjective and as a passive's verb, so a tagger that read a
/// word out of a list without its context would disagree with at least one row.
///
/// The tag set is the model's own `PartOfSpeechTagType`, whose values name the
/// Universal Dependencies v2 word classes (<https://universaldependencies.org/u/pos/>):
/// a function word accompanying the main verb of its clause is `AUX`, and the word it
/// accompanies keeps `VERB`.
type SyntaxRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

const SYNTAX_DOCUMENTS: &[SyntaxRow] = &[
    (
        "a copula with an adjective predicate is the clause's main verb",
        "The train was late.",
        &[("The", "DET"), ("was", "VERB"), ("late", "ADJ")],
    ),
    (
        "an auxiliary accompanying a main verb",
        "It is raining heavily now.",
        &[("is", "AUX"), ("raining", "VERB")],
    ),
    (
        "a modal accompanying a main verb",
        "She can swim.",
        &[("can", "AUX"), ("swim", "VERB")],
    ),
    (
        "a form of have with an object is the main verb",
        "I have a car.",
        &[("have", "VERB"), ("a", "DET")],
    ),
    (
        "the same form of have accompanying a participle is an auxiliary",
        "I have finished the report.",
        &[("have", "AUX"), ("finished", "VERB")],
    ),
    (
        "a participle in the predicate of a copula is that clause's verb",
        "The flight was delayed.",
        &[("was", "AUX"), ("delayed", "VERB")],
    ),
    (
        "a passive with an agent keeps its verb reading",
        "The window was broken by a storm.",
        &[("was", "AUX"), ("broken", "VERB"), ("by", "ADP")],
    ),
    (
        "a word with no verbal reading is the predicate's adjective",
        "The staff were rude.",
        &[("were", "VERB"), ("rude", "ADJ")],
    ),
    (
        "a word the lexicon claims as a verb closes a determiner-led phrase",
        "The quarterly report is due next Friday.",
        &[
            ("The", "DET"),
            // Read as the head's modifier: only its own `-ly` suffix speaks for an
            // adverb reading, and it stands between a determiner and the head.
            ("quarterly", "ADJ"),
            ("report", "NOUN"),
            ("Friday", "PROPN"),
        ],
    ),
    (
        "an adverb English's own class claims keeps that reading in a phrase",
        "The only option is clear.",
        &[("The", "DET"), ("only", "ADV"), ("option", "NOUN")],
    ),
    (
        "a gerund inside a compound is nominal",
        "The language processing service works.",
        &[("processing", "NOUN"), ("works", "VERB")],
    ),
    (
        "a finite verb between two nouns is the predicate",
        "The service finds insights.",
        &[("finds", "VERB"), ("insights", "NOUN")],
    ),
    (
        "a word after an article heads its noun phrase",
        "I watch the play and the running water.",
        &[("play", "NOUN"), ("running", "NOUN"), ("watch", "VERB")],
    ),
];

#[tokio::test]
async fn a_tag_is_the_reading_the_token_and_its_context_support() {
    let comprehend = Comprehend::new();
    let text_list: Vec<&str> = SYNTAX_DOCUMENTS
        .iter()
        .map(|(_, document, _)| *document)
        .collect();
    let results = batch_results(
        &comprehend,
        "BatchDetectSyntax",
        &json!({ "TextList": text_list, "LanguageCode": "en" }),
    )
    .await;

    // Every (case, token) pair's tag and confidence, for the cross-row assertions.
    let mut tagged: Vec<(&str, String, String, f64)> = Vec::new();
    for (position, (case, _, expected)) in SYNTAX_DOCUMENTS.iter().enumerate() {
        let tokens = results[position]["SyntaxTokens"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !tokens.is_empty(),
            "case {case:?}: expected the document's tokens, got {:?}",
            results[position]
        );

        for token in &tokens {
            let text = token["Text"].as_str().unwrap_or_default().to_string();
            let tag = token["PartOfSpeech"]["Tag"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let score = token["PartOfSpeech"]["Score"].as_f64().unwrap_or(f64::NAN);
            assert!(
                score > 0.0 && score <= 1.0,
                "case {case:?}: expected a Score in (0, 1], got {score} in {token}"
            );
            tagged.push((case, text, tag, score));
        }

        for (word, tag) in *expected {
            let reported = tagged
                .iter()
                .filter(|(name, _, _, _)| name == case)
                .find(|(_, text, _, _)| text == word)
                .map(|(_, _, tag, _)| tag.as_str())
                .unwrap_or_else(|| panic!("case {case:?}: no token {word:?} in {tokens:?}"));
            assert_eq!(
                reported, *tag,
                "case {case:?}: expected {word:?} to come back {tag:?}, got \
                 {reported:?} in {tokens:?}"
            );
        }
    }

    let reading = |case: &str, word: &str| -> (String, f64) {
        tagged
            .iter()
            .filter(|(name, _, _, _)| *name == case)
            .find(|(_, text, _, _)| text == word)
            .map(|(_, _, tag, score)| (tag.clone(), *score))
            .unwrap_or_else(|| panic!("no token {word:?} for case {case:?}"))
    };
    const COPULA_CASE: &str = "a copula with an adjective predicate is the clause's main verb";
    const PASSIVE_CASE: &str = "a passive with an agent keeps its verb reading";
    const GERUND_CASE: &str = "a gerund inside a compound is nominal";

    // The SAME word in two contexts: a tag read out of a word list could not differ
    // between these.
    let (auxiliary_tag, auxiliary_score) = reading(PASSIVE_CASE, "was");
    let (main_verb_tag, main_verb_score) = reading(COPULA_CASE, "was");
    assert_ne!(
        auxiliary_tag, main_verb_tag,
        "expected the same form of \"be\" to be read differently in the two contexts, \
         got {auxiliary_tag:?} both times"
    );

    // Full confidence exactly where nothing contests the reading. A token whose class
    // its own evidence and the position it stands in settle between them leaves no
    // competing reading of that token open — whether the class comes from its evidence
    // alone (a determiner one word class claims) or from the position (a form of `be`
    // read as its clause's main verb, and the same form read as the auxiliary of a
    // passive) — so it is reported at the certainty the recorded responses report such
    // a token with, rather than a hair below it at a value that recurs across
    // unrelated tokens.
    let (_, determiner) = reading(COPULA_CASE, "The");
    for (case, word, score) in [
        (COPULA_CASE, "The", determiner),
        (COPULA_CASE, "was", main_verb_score),
        (PASSIVE_CASE, "was", auxiliary_score),
    ] {
        assert_eq!(
            score, 1.0,
            "case {case:?}: expected {word:?}, whose reading nothing leaves open, to be \
             reported at full confidence, got {score}"
        );
    }

    // Below it exactly where a reading stays live: the position a gerund heading a
    // nominal compound stands in PREFERS the nominal reading without excluding the
    // verbal one, which is still a verb form, so the token keeps only the share the
    // displaced reading leaves it and a caller filtering on confidence can tell the
    // two apart.
    let (gerund_tag, gerund_score) = reading(GERUND_CASE, "processing");
    assert_eq!(
        gerund_tag, "NOUN",
        "case {GERUND_CASE:?}: expected the gerund to be read as the compound's noun, \
         got {gerund_tag:?}"
    );
    assert!(
        gerund_score < determiner,
        "case {GERUND_CASE:?}: expected a token whose other reading stays live to be \
         reported below one nothing contests, got {gerund_score} against {determiner}"
    );
    assert!(
        gerund_score > 0.9,
        "case {GERUND_CASE:?}: expected the preferred reading to stay a confident one \
         rather than being reported as a doubtful reading, got {gerund_score}"
    );

    // Over EVERY token of the table, not only the rows named above: a confidence is
    // either the certainty a token nothing contests carries, or visibly below it. The
    // band just under 1 is where a share left to a reading the position RULES OUT lands
    // — a value belonging to the rule table rather than to the token, which is what
    // recurs across unrelated tokens of unrelated documents — so no token may be
    // reported in it, and the table must hold tokens of both kinds.
    let mut settled = 0usize;
    let mut contested = 0usize;
    for (case, text, _, score) in tagged
        .iter()
        .filter(|(_, text, _, _)| text.chars().any(char::is_alphabetic))
    {
        assert!(
            *score == 1.0 || *score < CONTESTED_CONFIDENCE_CEILING,
            "case {case:?}: expected {text:?} to be reported at full confidence or \
             visibly below it, got {score}"
        );
        if *score == 1.0 {
            settled += 1;
        } else {
            contested += 1;
        }
    }
    assert!(
        settled > 0 && contested > 0,
        "expected the table to hold both tokens nothing contests and tokens a reading \
         stays live on, got {settled} at full confidence and {contested} below it"
    );
}

/// The confidence a token one of whose readings stays LIVE is reported below.
///
/// Not a rounding below 1: the share a live reading holds is a share of the question,
/// so it shows in the third decimal place at the latest. A token reported between this
/// and 1 would be one whose analysis is settled but whose confidence says otherwise,
/// which is what a residual left to an excluded reading produces.
const CONTESTED_CONFIDENCE_CEILING: f64 = 0.999;
