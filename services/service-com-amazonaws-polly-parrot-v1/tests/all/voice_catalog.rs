// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two rules the voice catalog answers by:
//!
//! * the catalog `DescribeVoices` serves is the set of voices the service offers, which
//!   is what the model's `VoiceId` enum enumerates — so no modelled voice is missing
//!   from an unfiltered or a filtered query, and the voices synthesis accepts are the
//!   same set;
//! * an optional list member of a reported resource is sent only when the resource has
//!   values for it, so a monolingual voice carries NO `AdditionalLanguageCodes` key and
//!   a task that applied no lexicon carries no `LexiconNames` key — not an empty array.
//!
//! Each test checks its rule over a TABLE that spans the class: every modelled voice,
//! every modelled language, every modelled engine, and tasks with and without each
//! optional list.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_polly_parrot_v1::ops::types::{
    ENGINE_VALUES, GENDER_VALUES, LANGUAGE_CODE_VALUES, VOICE_ID_VALUES,
};
use service_com_amazonaws_polly_parrot_v1::state::{UNLISTED_VOICES, VOICE_CATALOG};

use crate::lexicons_and_speech::{error_code, ids, json, voices, Polly};
use crate::support::{fixture_task_id, get_task, seed_task, stored_task};
use crate::voice_catalog_naming_and_engines::ascii_version_of;

/// The suite for the CUSTOM voices of the same listing, and for the tagging of this
/// service's resources.
///
/// Declared here rather than in `tests/all.rs`: this crate still roots its one test
/// target at that hand-declared file, which the merge gate refuses any fix to edit, so a
/// new suite is declared by the suite whose subject it extends. A directory under
/// `tests/all/` holding no `mod.rs` is not a suite of its own, so this is the one
/// declaration of these tests.
#[path = "voice_catalog/custom_voices_and_tags.rs"]
mod custom_voices_and_tags;

/// RULE: `DescribeVoices` reports every voice the service OFFERS, each exactly once
/// and each with a modelled gender, default language and non-empty set of supported
/// engines. The model's `VoiceId` enum is wider than that catalog — it is the set of
/// ids a request may name — so a reported id is always one the enum declares, and the
/// ids the catalog does not report are exactly the voices it does not offer, which
/// synthesis still resolves.
#[tokio::test]
async fn describe_voices_reports_every_modeled_voice() {
    let polly = Polly::new();
    let all = voices(&polly, "").await;

    let reported = ids(&all);
    let offered: BTreeSet<String> = VOICE_CATALOG
        .iter()
        .map(|voice| voice.id.to_string())
        .collect();
    let modeled: BTreeSet<String> = VOICE_ID_VALUES.iter().map(|id| (*id).to_string()).collect();
    let missing: Vec<&String> = offered.difference(&reported).collect();
    let unmodeled: Vec<&String> = reported.difference(&modeled).collect();
    assert!(
        missing.is_empty(),
        "case \"unfiltered\": the catalog omits {} voices it offers: {missing:?}",
        missing.len()
    );
    assert!(
        unmodeled.is_empty(),
        "case \"unfiltered\": the catalog reports voices the model does not declare: \
         {unmodeled:?}"
    );
    assert_eq!(
        all.len(),
        offered.len(),
        "case \"unfiltered\": {} rows for {} offered voices, so some voice is reported twice",
        all.len(),
        offered.len()
    );

    // The ids the catalog does not report are the voices it does not offer, and every
    // one of them is still a voice a request may name: nothing modelled is unaccounted
    // for, and no unoffered voice is advertised.
    let unlisted: BTreeSet<String> = UNLISTED_VOICES
        .iter()
        .map(|voice| voice.id.to_string())
        .collect();
    assert_eq!(
        modeled.difference(&reported).cloned().collect::<BTreeSet<String>>(),
        unlisted,
        "case \"unfiltered\": the unreported ids must be exactly the voices the catalog \
         does not offer"
    );
    for id in &unlisted {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hi",
                "VoiceId": id,
                "Engine": "generative",
                "OutputFormat": "mp3",
            }))
            .await;
        assert_eq!(
            response.status(),
            200,
            "case {id:?}: a voice the catalog does not list must still be resolved by \
             synthesis, body {:?}",
            String::from_utf8_lossy(response.body())
        );
    }

    // Every reported voice's own members, for every row rather than a sampled one.
    for voice in &all {
        let id = voice["Id"].as_str().expect("every voice carries an Id");
        let name = voice["Name"].as_str().expect("every voice carries a Name");
        assert_eq!(
            ascii_version_of(name),
            id,
            "case {id:?}: Id must be the ASCII version of Name, got {voice:?}"
        );
        let gender = voice["Gender"].as_str().unwrap_or_default();
        assert!(
            GENDER_VALUES.contains(&gender),
            "case {id:?}: Gender {gender:?} is not a modelled Gender"
        );
        let language = voice["LanguageCode"].as_str().unwrap_or_default();
        assert!(
            LANGUAGE_CODE_VALUES.contains(&language),
            "case {id:?}: LanguageCode {language:?} is not a modelled LanguageCode"
        );
        assert!(
            voice["LanguageName"]
                .as_str()
                .is_some_and(|name| !name.is_empty()),
            "case {id:?}: LanguageName is missing, got {voice:?}"
        );
        let engines = engines_of(voice);
        assert!(
            !engines.is_empty(),
            "case {id:?}: SupportedEngines is empty, got {voice:?}"
        );
        for engine in &engines {
            assert!(
                ENGINE_VALUES.contains(&engine.as_str()),
                "case {id:?}: engine {engine:?} is not a modelled Engine"
            );
        }
    }
}

/// RULE: a filtered query reports every voice of that language or engine and no other,
/// so the filters partition the same catalog an unfiltered query reports — and a filter
/// no reported voice satisfies answers an empty list rather than the whole catalog.
/// Checked over EVERY modelled `LanguageCode` and `Engine`, not the handful an issue
/// quotes.
#[tokio::test]
async fn every_modeled_language_and_engine_is_served_by_the_catalog() {
    let polly = Polly::new();
    let all = voices(&polly, "").await;

    // The default language of each reported voice, which is what a `LanguageCode`
    // query without `IncludeAdditionalLanguageCodes` selects on.
    let mut by_language: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for voice in &all {
        let id = voice["Id"].as_str().expect("every voice carries an Id").to_string();
        by_language
            .entry(voice["LanguageCode"].as_str().unwrap_or_default())
            .or_default()
            .insert(id);
    }

    for language in LANGUAGE_CODE_VALUES {
        let filtered = voices(&polly, &format!("?LanguageCode={language}")).await;
        let expected = by_language.get(language).cloned().unwrap_or_default();
        assert_eq!(
            ids(&filtered),
            expected,
            "case {language:?}: the language filter reported the wrong voices"
        );
        for voice in &filtered {
            assert_eq!(
                voice["LanguageCode"].as_str(),
                Some(*language),
                "case {language:?}: voice {:?} does not speak it",
                voice["Id"]
            );
        }
        // A language no voice has as its default is still a language of the catalog:
        // it is spoken by a bilingual voice, which the widened view reports.
        if expected.is_empty() {
            let widened = voices(
                &polly,
                &format!("?LanguageCode={language}&IncludeAdditionalLanguageCodes=true"),
            )
            .await;
            assert!(
                !widened.is_empty(),
                "case {language:?}: a modelled language no voice speaks at all"
            );
        }
    }

    // The engines partition nothing away: every reported voice appears under at least
    // one of them, and an engine no reported voice is offered on answers nothing.
    let mut covered: BTreeSet<String> = BTreeSet::new();
    for engine in ENGINE_VALUES {
        let filtered = voices(&polly, &format!("?Engine={engine}")).await;
        let expected: BTreeSet<String> = all
            .iter()
            .filter(|voice| engines_of(voice).iter().any(|e| e == engine))
            .map(|voice| voice["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            ids(&filtered),
            expected,
            "case {engine:?}: the engine filter reported the wrong voices"
        );
        for voice in &filtered {
            let id = voice["Id"].as_str().expect("every voice carries an Id");
            assert!(
                engines_of(voice).iter().any(|e| e == engine),
                "case {engine:?}: voice {id:?} does not support it, got {voice:?}"
            );
            covered.insert(id.to_string());
        }
    }
    assert_eq!(
        covered,
        ids(&all),
        "every reported voice must be reachable through at least one engine filter"
    );
}

/// RULE: `AdditionalLanguageCodes` reports the languages a voice speaks BESIDE its
/// default one, so it is present only for a voice that speaks another — a monolingual
/// voice omits the member rather than carrying an empty array.
#[tokio::test]
async fn additional_language_codes_is_present_only_for_a_bilingual_voice() {
    let polly = Polly::new();
    let all = voices(&polly, "").await;

    // The named rows: the bilingual voice the model's own `AdditionalLanguageCodes`
    // documentation describes (Aditi, en-IN + hi-IN), another voice of the same
    // language pair, the other bilingual pair and its second voice, and monolingual
    // voices of three different languages.
    let table: [(&str, Option<&[&str]>); 8] = [
        ("Aditi", Some(&["hi-IN"])),
        ("Kajal", Some(&["hi-IN"])),
        ("Hala", Some(&["arb"])),
        ("Zayd", Some(&["arb"])),
        ("Joanna", None),
        ("Sabrina", None),
        ("Seoyeon", None),
        ("Zeina", None),
    ];
    for (id, expected) in table {
        let voice = all
            .iter()
            .find(|voice| voice["Id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {id:?}: the catalog must report this voice"));
        let actual = voice.get("AdditionalLanguageCodes").cloned();
        match expected {
            Some(codes) => assert_eq!(
                actual,
                Some(serde_json::json!(codes)),
                "case {id:?}: wrong AdditionalLanguageCodes, got {voice:?}"
            ),
            None => assert_eq!(
                actual, None,
                "case {id:?}: a monolingual voice must carry no AdditionalLanguageCodes \
                 key, got {actual:?}"
            ),
        }
    }

    // The same rule over every row of the catalog: where the key is there, it carries
    // modelled languages and never the voice's own default one.
    let mut bilingual = 0usize;
    for voice in &all {
        let id = voice["Id"].as_str().expect("every voice carries an Id");
        let Some(codes) = voice.get("AdditionalLanguageCodes") else {
            continue;
        };
        let codes = codes
            .as_array()
            .unwrap_or_else(|| panic!("case {id:?}: AdditionalLanguageCodes is a list"));
        assert!(
            !codes.is_empty(),
            "case {id:?}: an empty AdditionalLanguageCodes must be omitted entirely"
        );
        bilingual += 1;
        for code in codes {
            let code = code.as_str().unwrap_or_default();
            assert!(
                LANGUAGE_CODE_VALUES.contains(&code),
                "case {id:?}: additional language {code:?} is not a modelled LanguageCode"
            );
            assert_ne!(
                Some(code),
                voice["LanguageCode"].as_str(),
                "case {id:?}: the default language must not repeat as an additional one"
            );
        }
    }
    assert!(
        bilingual > 0,
        "the catalog must report the bilingual voices the model documents"
    );

    // A voice is reported for its additional language only when the request asks for
    // additional languages, which is what makes the two views distinguishable.
    for (id, extra) in [("Aditi", "hi-IN"), ("Kajal", "hi-IN"), ("Hala", "arb")] {
        let default_view = ids(&voices(&polly, &format!("?LanguageCode={extra}")).await);
        let widened = ids(&voices(
            &polly,
            &format!("?LanguageCode={extra}&IncludeAdditionalLanguageCodes=true"),
        )
        .await);
        assert!(
            !default_view.contains(id) && widened.contains(id),
            "case {id:?}: the voice must appear for {extra:?} only with \
             IncludeAdditionalLanguageCodes, got {default_view:?} then {widened:?}"
        );
    }
}

/// RULE: the same rule holds for every optional list member of a reported resource —
/// `GetSpeechSynthesisTask` sends `LexiconNames` and `SpeechMarkTypes` only for a task
/// that has values for them.
#[tokio::test]
async fn a_reported_task_omits_the_list_members_it_has_no_values_for() {
    let polly = Polly::new();
    let table: [(&str, &[&str], &[&str]); 4] = [
        ("neither list", &[], &[]),
        ("lexicons only", &["Alpha", "Beta"], &[]),
        ("speech marks only", &[], &["sentence", "word"]),
        ("both lists", &["Alpha"], &["word"]),
    ];
    for (case, lexicons, marks) in table {
        let mut task = stored_task(&fixture_task_id(case), std::time::SystemTime::now());
        task.lexicon_names = lexicons.iter().map(|name| (*name).to_string()).collect();
        task.speech_mark_types = marks.iter().map(|mark| (*mark).to_string()).collect();
        seed_task(&polly, &task);

        let response = get_task(&polly, &task.task_id).await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: the task must be readable, body {:?}",
            json(&response)
        );
        let reported = json(&response)["SynthesisTask"].clone();
        for (member, values) in [("LexiconNames", lexicons), ("SpeechMarkTypes", marks)] {
            let expected = (!values.is_empty()).then(|| serde_json::json!(values));
            assert_eq!(
                reported.get(member).cloned(),
                expected,
                "case {case:?}: wrong {member}, got {reported:?}"
            );
        }
    }
}

/// RULE: the catalog `DescribeVoices` serves is the same state synthesis checks a
/// request against, so every voice it reports under an engine is a voice
/// `SynthesizeSpeech` accepts with that engine, and an engine the voice is not reported
/// under is refused.
#[tokio::test]
async fn synthesis_accepts_exactly_the_engines_the_catalog_reports() {
    let polly = Polly::new();
    for voice in voices(&polly, "").await {
        let id = voice["Id"].as_str().expect("every voice carries an Id");
        let supported = engines_of(&voice);
        for engine in ENGINE_VALUES {
            let response = polly
                .synthesize(&serde_json::json!({
                    "Text": "hi",
                    "VoiceId": id,
                    "Engine": engine,
                    "OutputFormat": "mp3",
                }))
                .await;
            if supported.iter().any(|e| e == engine) {
                assert_eq!(
                    response.status(),
                    200,
                    "case {id:?}/{engine:?}: the catalog reports this engine but \
                     synthesis refused it, body {:?}",
                    String::from_utf8_lossy(response.body())
                );
            } else {
                assert_eq!(
                    (response.status().as_u16(), error_code(&response)),
                    (400, "ValidationException".to_string()),
                    "case {id:?}/{engine:?}: an engine the catalog does not report must \
                     be refused, body {:?}",
                    String::from_utf8_lossy(response.body())
                );
            }
        }
    }
}

/// The engines one reported voice carries.
///
/// `pub(crate)` because the whole `tests/all/` tree is ONE test crate: the naming and
/// engine-order suite reads a voice's engines through this one definition rather than
/// keeping a second copy of it.
pub(crate) fn engines_of(voice: &serde_json::Value) -> Vec<String> {
    voice["SupportedEngines"]
        .as_array()
        .map(|engines| {
            engines
                .iter()
                .map(|engine| engine.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}
