// SPDX-License-Identifier: CC0-1.0

//! WHICH language a job whose language the service IDENTIFIES reports, end to end over
//! the wire, with the emulator's own Amazon S3 mounted beside transcribe on one engine
//! so the media a start names is really stored: `PutObject` ->
//! `StartTranscriptionJob` / `StartCallAnalyticsJob` -> `Get...`.
//!
//! THE RULE UNDER TEST: automatic language identification is a determination about the
//! MEDIA — "the confidence score associated with the language identified in your media
//! file" (`smithy_query(query="doc", shape="TranscriptionJob",
//! member="IdentifiedLanguageScore")`) — so a completed job reports the language the
//! stored object it was pointed at evidences, in the spelling the request's
//! `LanguageOptions` allowed identification to answer with ("Including LanguageOptions
//! restricts IdentifyLanguage to only the language options that you specify"). Two jobs
//! differing only in their media object therefore identify different languages, and the
//! option list is not itself the answer: it is only what narrows one. Media evidencing
//! no language leaves identification nothing to go on, and the job then reports the
//! first code the request allowed.
//!
//! Each test asserts that over a TABLE whose rows differ along what the rule
//! generalises over: where the evidence comes from (the language the stored object
//! DECLARES for its content, and the language tag its address carries), how that
//! evidence is spelled (a bare primary subtag, a whole language code, a regional
//! variant of an allowed language), which codes the request allowed (none, two, three),
//! whether the evidenced language is among them at all, and which identification was
//! asked for (single-language, multi-language, and the Call Analytics family, which has
//! no language member to name one with). The languages are read off the model's own
//! `LanguageCode` enum and include codes the issue behind this suite never named.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::ops::types::LANGUAGE_CODE_VALUES;
use service_com_amazonaws_transcribe_transcribe::state::IDENTIFIED_LANGUAGE_SCORE;

use crate::media_object_resolution::{given_object_declaring_language, BUCKET, MEDIA_BYTES};
use crate::transcription_jobs::Transcribe;

/// How long to wait between two polls of a job that has not finished yet.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How many polls a job gets before the suite gives up: a span generous enough that a
/// loaded machine still finishes the job's two clock-owed transitions (the condition is
/// what is waited for, never a fixed time).
const POLLS_FOR_COMPLETION: usize = 1_200;

/// One row of the table: the media object a job is pointed at, what that object
/// evidences about its language, what the request allowed identification to answer
/// with, and the `LanguageCode` the completed job must report.
struct Row {
    case: &'static str,
    /// The key the media is stored under in [`BUCKET`].
    key: &'static str,
    /// The `Content-Language` the stored object declares, if any.
    declared: Option<&'static str>,
    /// `LanguageOptions`, as the start request lists them.
    options: Option<&'static [&'static str]>,
    /// Whether the stored media evidenced a language identification was allowed to
    /// answer with: `false` for a row whose media evidences nothing, and for one whose
    /// evidence names a language no option allowed, where the answer falls back to the
    /// options the request listed.
    from_media: bool,
    /// The code the finished job must report.
    expected: &'static str,
}

/// The table. Every expected code is a value of the model's own `LanguageCode` enum,
/// which the last assertion of each test re-checks against `LANGUAGE_CODE_VALUES`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the subtag the stored object's name carries, among the allowed options",
            key: "speech/speech-en.mp3",
            declared: None,
            options: Some(&["en-US", "es-ES", "fr-FR"]),
            from_media: true,
            expected: "en-US",
        },
        Row {
            // The same request as the row above but for the media object: the one
            // difference that has to change the answer.
            case: "another media object, whose name carries another subtag",
            key: "speech/speech-fr.mp3",
            declared: None,
            options: Some(&["en-US", "es-ES", "fr-FR"]),
            from_media: true,
            expected: "fr-FR",
        },
        Row {
            case: "a whole language code in the stored object's name",
            key: "speech/interview-es-ES.mp3",
            declared: None,
            options: Some(&["en-US", "es-ES", "fr-FR"]),
            from_media: true,
            expected: "es-ES",
        },
        Row {
            case: "a language code in the prefix the object is stored under",
            key: "ja-JP/town-hall.mp3",
            declared: None,
            options: Some(&["en-US", "ja-JP"]),
            from_media: true,
            expected: "ja-JP",
        },
        Row {
            // A regional variant of an allowed language: identification may only answer
            // with a code the request allowed, so it answers with that language's
            // allowed variant.
            case: "a regional variant of an allowed language",
            key: "speech/entretien-fr-CA.mp3",
            declared: None,
            options: Some(&["en-US", "fr-FR"]),
            from_media: true,
            expected: "fr-FR",
        },
        Row {
            case: "the language the stored object declares for its content",
            key: "speech/clip.mp3",
            declared: Some("de-DE"),
            options: Some(&["de-DE", "en-US"]),
            from_media: true,
            expected: "de-DE",
        },
        Row {
            // The object's own declaration is the more authoritative evidence, so it is
            // what identification reads.
            case: "a declared language over a name carrying another tag",
            key: "speech/greeting-en.mp3",
            declared: Some("pt-BR"),
            options: Some(&["en-US", "pt-BR"]),
            from_media: true,
            expected: "pt-BR",
        },
        Row {
            // Nothing narrows the choice, and a whole code states the variant.
            case: "a declared language with no options narrowing the choice",
            key: "speech/keynote.mp3",
            declared: Some("zh-TW"),
            options: None,
            from_media: true,
            expected: "zh-TW",
        },
        Row {
            // Identification may answer with no code the request disallowed, so the
            // evidence is not usable and the answer falls back to the options.
            case: "a language no option allowed",
            key: "speech/talk-ja-JP.mp3",
            declared: None,
            options: Some(&["pt-BR", "en-US"]),
            from_media: false,
            expected: "pt-BR",
        },
        Row {
            case: "media evidencing no language at all",
            key: "speech/dictation.mp3",
            declared: None,
            options: Some(&["it-IT", "en-GB"]),
            from_media: false,
            expected: "it-IT",
        },
    ]
}

impl Row {
    /// The `s3://` location this row's media is stored at.
    fn uri(&self) -> String {
        format!("s3://{BUCKET}/{key}", key = self.key)
    }

    /// Store this row's media object, declaring its language where the row names one.
    async fn stage(&self, transcribe: &Transcribe) {
        match self.declared {
            Some(language) => {
                given_object_declaring_language(
                    &transcribe.router,
                    BUCKET,
                    self.key,
                    MEDIA_BYTES,
                    language,
                )
                .await
            }
            None => transcribe.given_object(BUCKET, self.key, MEDIA_BYTES).await,
        }
    }

    /// A start request asking for the identification `member` names over this row's
    /// media and options.
    fn start_request(&self, name: &str, member: &str) -> Value {
        let mut request = json!({
            "TranscriptionJobName": name,
            "Media": {"MediaFileUri": self.uri()},
            // Every row's media is stored under a name ending `.mp3`, so the declared
            // format is the one the stored object evidences and no row fails on a
            // format mismatch.
            "MediaFormat": "mp3",
            member: true
        });
        if let Some(options) = self.options {
            request["LanguageOptions"] = json!(options);
        }
        request
    }
}

/// Poll `operation` for the job `name` until it has left `IN_PROGRESS`, and answer the
/// job it settled as — the wait a caller performs, looking again until the condition
/// holds rather than for a fixed time.
async fn settled_job(
    transcribe: &Transcribe,
    case: &str,
    operation: &str,
    name_member: &str,
    job_member: &str,
    status_member: &str,
    name: &str,
) -> Value {
    for _ in 0..POLLS_FOR_COMPLETION {
        let reply = transcribe
            .call(operation, json!({name_member: name}))
            .await;
        let job = reply.ok(case)[job_member].clone();
        let status = job[status_member].as_str().unwrap_or_default().to_string();
        if status != "QUEUED" && status != "IN_PROGRESS" {
            assert_eq!(
                status, "COMPLETED",
                "case {case:?}: the job must complete, got {status:?} ({job:?})"
            );
            return job;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!("case {case:?}: the job never finished being processed");
}

/// The same wait for a transcription job.
async fn settled_transcription_job(transcribe: &Transcribe, case: &str, name: &str) -> Value {
    settled_job(
        transcribe,
        case,
        "GetTranscriptionJob",
        "TranscriptionJobName",
        "TranscriptionJob",
        "TranscriptionJobStatus",
        name,
    )
    .await
}

/// Assert that `code` is a value of the model's own `LanguageCode` enum: whatever
/// identification answers is a modelled code, never a label read off a file name.
fn assert_modelled(case: &str, code: &Value) {
    let reported = code
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an identified LanguageCode, got {code:?}"));
    assert!(
        LANGUAGE_CODE_VALUES.contains(&reported),
        "case {case:?}: an identified language must be a modelled LanguageCode, got {reported:?}"
    );
}

#[tokio::test]
async fn single_language_identification_reports_the_language_its_media_evidences() {
    let transcribe = Transcribe::with_object_store();
    let rows = rows();

    for (index, row) in rows.iter().enumerate() {
        row.stage(&transcribe).await;
        let name = format!("idmedia-{index}");
        let request = row.start_request(&name, "IdentifyLanguage");
        let started = transcribe.call("StartTranscriptionJob", request).await;
        let job = started.ok(row.case)["TranscriptionJob"].clone();
        // Identifying the language is part of processing the media, so a job that has
        // not finished reports no result yet.
        assert!(
            job["LanguageCode"].is_null(),
            "case {:?}: a job that has not completed reports no LanguageCode, got {:?}",
            row.case,
            job["LanguageCode"]
        );
    }

    for (index, row) in rows.iter().enumerate() {
        let name = format!("idmedia-{index}");
        let job = settled_transcription_job(&transcribe, row.case, &name).await;
        assert_eq!(
            job["LanguageCode"],
            json!(row.expected),
            "case {:?}: expected the identified LanguageCode {:?}, got {:?}",
            row.case,
            row.expected,
            job["LanguageCode"]
        );
        assert_modelled(row.case, &job["LanguageCode"]);
        let score = job["IdentifiedLanguageScore"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: a completed identification reports a confidence score, got {:?}",
                row.case, job["IdentifiedLanguageScore"]
            )
        });
        assert!(
            (0.5..=1.0).contains(&score),
            "case {:?}: the identified language must be reported with a confidence above \
             a half, got {score}",
            row.case
        );
        assert_eq!(
            job["IdentifiedLanguageScore"],
            json!(IDENTIFIED_LANGUAGE_SCORE),
            "case {:?}: expected the score {IDENTIFIED_LANGUAGE_SCORE}, got {score}",
            row.case
        );

        // A read does not change what a later read sees: the identification is a
        // function of the stored record.
        let again = settled_transcription_job(&transcribe, row.case, &name).await;
        assert_eq!(
            again, job,
            "case {:?}: a second read must report the same job",
            row.case
        );
    }
}

#[tokio::test]
async fn two_jobs_differing_only_in_their_media_identify_different_languages() {
    let transcribe = Transcribe::with_object_store();
    // The pairs the rule's own invariant is about: the SAME request but for the media
    // object, each member of the pair evidencing a different language.
    let pairs: &[(&str, &str, &str, &str, &[&str])] = &[
        (
            "a name carrying a subtag",
            "pair/one-en.mp3",
            "pair/one-fr.mp3",
            "mp3",
            &["en-US", "es-ES", "fr-FR"],
        ),
        (
            "a prefix carrying a whole code",
            "de-DE/pair/two.wav",
            "sv-SE/pair/two.wav",
            "wav",
            &["de-DE", "sv-SE", "en-US"],
        ),
    ];

    for (index, (case, first_key, second_key, format, options)) in pairs.iter().enumerate() {
        let mut identified = Vec::new();
        for (leg, key) in [("a", first_key), ("b", second_key)] {
            transcribe.given_object(BUCKET, key, MEDIA_BYTES).await;
            let name = format!("pair-{index}-{leg}");
            let request = json!({
                "TranscriptionJobName": name,
                "Media": {"MediaFileUri": format!("s3://{BUCKET}/{key}")},
                "MediaFormat": format,
                "IdentifyLanguage": true,
                "LanguageOptions": options
            });
            transcribe
                .call("StartTranscriptionJob", request)
                .await
                .ok(case);
            identified.push((name, key));
        }
        let mut answers = Vec::new();
        for (name, key) in &identified {
            let job = settled_transcription_job(&transcribe, case, name).await;
            let code = job["LanguageCode"].clone();
            assert_modelled(case, &code);
            assert!(
                options.contains(&code.as_str().unwrap_or_default()),
                "case {case:?} ({key}): identification may only answer with an allowed \
                 option {options:?}, got {code:?}"
            );
            answers.push(code);
        }
        assert_ne!(
            answers[0], answers[1],
            "case {case:?}: two jobs differing only in their media must identify \
             different languages, both got {:?}",
            answers[0]
        );
    }
}

#[tokio::test]
async fn multi_language_identification_reports_the_language_its_media_evidences() {
    let transcribe = Transcribe::with_object_store();
    let rows = rows();

    for (index, row) in rows.iter().enumerate() {
        row.stage(&transcribe).await;
        let name = format!("idmulti-media-{index}");
        let request = row.start_request(&name, "IdentifyMultipleLanguages");
        transcribe
            .call("StartTranscriptionJob", request)
            .await
            .ok(row.case);
    }

    for (index, row) in rows.iter().enumerate() {
        let name = format!("idmulti-media-{index}");
        let job = settled_transcription_job(&transcribe, row.case, &name).await;
        let codes: Vec<Value> = job["LanguageCodes"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a completed multi-language job reports LanguageCodes, got {:?}",
                    row.case, job["LanguageCodes"]
                )
            })
            .iter()
            .map(|item| item["LanguageCode"].clone())
            .collect();
        assert!(
            codes.contains(&json!(row.expected)),
            "case {:?}: the languages identified in the media must include {:?}, got {codes:?}",
            row.case,
            row.expected
        );
        // Media the service found one language in holds that one language; media that
        // evidenced none leaves the answer to the options the request listed.
        if row.from_media {
            assert_eq!(
                codes,
                vec![json!(row.expected)],
                "case {:?}: media evidencing one language is identified as holding that \
                 one language, got {codes:?}",
                row.case
            );
        }
        for code in &codes {
            assert_modelled(row.case, code);
        }
        // The singular member and its score belong to single-language identification.
        for member in ["LanguageCode", "IdentifiedLanguageScore"] {
            assert!(
                job[member].is_null(),
                "case {:?}: multi-language identification reports no {member}, got {:?}",
                row.case,
                job[member]
            );
        }
    }
}

#[tokio::test]
async fn a_call_analytics_job_identifies_the_language_of_the_call_it_was_given() {
    let transcribe = Transcribe::with_object_store();
    // This family carries no language member at all, so every job of it identifies a
    // language: one row per form the evidence takes, with and without the
    // `LanguageOptions` its settings may narrow the choice with.
    /// One row: the case, the key the call audio is stored under, the language that
    /// object declares for its content, the options the job's settings narrow
    /// identification with, and the code the finished job must report.
    type CallRow<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a [&'a str]>,
        &'a str,
    );
    let rows: &[CallRow<'_>] = &[
        (
            "a whole code in the stored call's name, unnarrowed",
            "calls/support-ko-KR.wav",
            None,
            None,
            "ko-KR",
        ),
        (
            "a subtag in the stored call's name, narrowed by the settings",
            "calls/support-nl.wav",
            None,
            Some(&["en-US", "nl-NL"]),
            "nl-NL",
        ),
        (
            "the language the stored call declares for its content",
            "calls/support.wav",
            Some("pl-PL"),
            None,
            "pl-PL",
        ),
        (
            "a call evidencing no language, narrowed by the settings",
            "calls/recording.wav",
            None,
            Some(&["hi-IN", "en-IN"]),
            "hi-IN",
        ),
    ];

    for (index, (case, key, declared, options, _)) in rows.iter().enumerate() {
        match declared {
            Some(language) => {
                given_object_declaring_language(
                    &transcribe.router,
                    BUCKET,
                    key,
                    MEDIA_BYTES,
                    language,
                )
                .await
            }
            None => transcribe.given_object(BUCKET, key, MEDIA_BYTES).await,
        }
        let mut request = json!({
            "CallAnalyticsJobName": format!("callid-{index}"),
            "Media": {"MediaFileUri": format!("s3://{BUCKET}/{key}")}
        });
        if let Some(options) = options {
            request["Settings"] = json!({"LanguageOptions": options});
        }
        transcribe
            .call("StartCallAnalyticsJob", request)
            .await
            .ok(case);
    }

    for (index, (case, _, _, _, expected)) in rows.iter().enumerate() {
        let job = settled_job(
            &transcribe,
            case,
            "GetCallAnalyticsJob",
            "CallAnalyticsJobName",
            "CallAnalyticsJob",
            "CallAnalyticsJobStatus",
            &format!("callid-{index}"),
        )
        .await;
        assert_eq!(
            job["LanguageCode"],
            json!(expected),
            "case {case:?}: expected the identified LanguageCode {expected:?}, got {:?}",
            job["LanguageCode"]
        );
        assert_modelled(case, &job["LanguageCode"]);
    }
}
