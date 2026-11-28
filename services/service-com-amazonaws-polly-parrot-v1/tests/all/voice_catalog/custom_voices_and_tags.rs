// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the six operations the model update added: the custom-voice
//! lifecycle (`CreateVoice`, `GetVoice`, `DeleteVoice`, and what `DescribeVoices` reports
//! for a custom voice) and the tagging of this service's resources (`TagResource`,
//! `UntagResource`, `ListTagsForResource`, and the `Tags` `PutLexicon` takes).
//!
//! Each test states the RULE it is checking and then checks it over a TABLE of inputs
//! that spans the class the rule covers — every language a custom voice can be created
//! in, both audio sources a recording can name, both resource types this service tags,
//! and every form of ARN that addresses no resource — rather than the one request an
//! example happens to quote.
//!
//! It lives in a file of its own under `tests/all/voice_catalog/` and is declared by
//! `tests/all/voice_catalog.rs`: this crate still roots its one test target at the
//! hand-declared `tests/all.rs`, which the merge gate refuses any fix to edit, so a new
//! suite is declared by the suite whose subject it extends instead of there.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};

use service_com_amazonaws_polly_parrot_v1::ops::create_voice::{
    MAX_CUSTOM_VOICES, MAX_STORED_RECORDINGS, MAX_STORED_RECORDING_URL_CHARACTERS,
};
use service_com_amazonaws_polly_parrot_v1::ops::types::CUSTOM_VOICE_LANGUAGE_CODE_VALUES;
use service_com_amazonaws_polly_parrot_v1::state::{
    CUSTOM_VOICE_ENGINES, MAX_TAGS_PER_RESOURCE, VOICE_STATUS_ACTIVE, VOICE_STATUS_PENDING,
};

use crate::lexicons_and_speech::{error_code, json as body_of, pls, Polly};

/// The Region the suite signs for unless a case is about another one.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// How long a case waits for a voice to reach `active`, and how often it looks.
///
/// Driven off what has HAPPENED rather than off one fixed sleep, so a slow machine delays
/// a case instead of breaking it. The wait is far longer than the activation span the
/// service stores, which is what makes it insensitive to how fast the host runs.
const ACTIVE_WAIT: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// One `CreateVoice` request body, with the members a case is about.
fn create_request(name: &str, language: &str, recordings: Value) -> Value {
    json!({
        "Name": name,
        "LanguageCode": language,
        "TermsAccepted": true,
        "VoiceCreationData": { "Recordings": recordings },
    })
}

/// A recording Amazon Polly would read from Amazon S3.
fn s3_recording(format: &str, url: &str) -> Value {
    json!({ "Format": format, "Data": { "S3Url": url } })
}

/// A recording whose audio travels in the request, base64 as the protocol carries a blob.
fn bytes_recording(format: &str) -> Value {
    // Four bytes of audio stand in for a recording: nothing here reads the audio, and
    // the service does not keep it.
    json!({ "Format": format, "Data": { "Bytes": "AAECAw==" } })
}

/// `CreateVoice` in `region`.
async fn create_voice(polly: &Polly, region: &str, request: &Value) -> http::Response<Bytes> {
    polly
        .call_in_region(region, "POST", "/v1/voices", &request.to_string())
        .await
}

/// `GetVoice` in `region`.
async fn get_voice(polly: &Polly, region: &str, voice_id: &str) -> http::Response<Bytes> {
    polly
        .call_in_region(region, "GET", &format!("/v1/voices/{voice_id}"), "")
        .await
}

/// `DeleteVoice` in `region`.
async fn delete_voice(polly: &Polly, region: &str, voice_id: &str) -> http::Response<Bytes> {
    polly
        .call_in_region(region, "DELETE", &format!("/v1/voices/{voice_id}"), "")
        .await
}

/// Every voice `DescribeVoices` reports in `region`, by `Id`.
async fn described(polly: &Polly, region: &str, query: &str) -> BTreeMap<String, Value> {
    let response = polly
        .call_in_region(region, "GET", &format!("/v1/voices{query}"), "")
        .await;
    assert_eq!(
        response.status(),
        200,
        "DescribeVoices{query} must answer, body {:?}",
        body_of(&response)
    );
    body_of(&response)["Voices"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|voice| {
            voice["Id"]
                .as_str()
                .map(|id| (id.to_string(), voice.clone()))
        })
        .collect()
}

/// The voice a successful `CreateVoice` answered with.
fn created_voice(case: &str, response: &http::Response<Bytes>) -> Value {
    assert_eq!(
        response.status(),
        200,
        "case {case:?}: CreateVoice must answer, body {:?}",
        body_of(response)
    );
    let voice = body_of(response)["Voice"].clone();
    assert!(
        voice.is_object(),
        "case {case:?}: CreateVoice reports no voice, got {voice}"
    );
    voice
}

/// The `message` of an error document, whatever case the member is spelled in.
fn message(response: &http::Response<Bytes>) -> String {
    let body = body_of(response);
    for key in ["message", "Message"] {
        if let Some(value) = body.get(key).and_then(Value::as_str) {
            return value.to_string();
        }
    }
    String::new()
}

/// Poll `GetVoice` until the voice is `active`, and answer what it then reports.
async fn awaited_active(polly: &Polly, case: &str, voice_id: &str) -> Value {
    let looks = ACTIVE_WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut reported = Value::Null;
    for _ in 0..looks {
        let response = get_voice(polly, REGION, voice_id).await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: the created voice must read back, body {:?}",
            body_of(&response)
        );
        reported = body_of(&response)["Voice"].clone();
        if reported["Status"].as_str() == Some(VOICE_STATUS_ACTIVE) {
            return reported;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: the voice never became active, last report {reported}")
}

/// The ARN of the lexicon `name` of `region`, as this emulator mints it.
fn lexicon_arn(region: &str, name: &str) -> String {
    format!("arn:aws:polly:{region}:{ACCOUNT}:lexicon/{name}")
}

/// `ListTagsForResource` for `arn` in `region`.
async fn list_tags(polly: &Polly, region: &str, arn: &str) -> http::Response<Bytes> {
    polly
        .call_in_region(
            region,
            "GET",
            &format!("/v1/listTagsForResource?arn={arn}"),
            "",
        )
        .await
}

/// `TagResource` for `arn` in `region`.
async fn tag_resource(
    polly: &Polly,
    region: &str,
    arn: &str,
    tags: &Value,
) -> http::Response<Bytes> {
    let body = json!({ "ResourceArn": arn, "Tags": tags });
    polly
        .call_in_region(region, "POST", "/v1/tagResource", &body.to_string())
        .await
}

/// `UntagResource` for `arn` in `region`.
async fn untag_resource(
    polly: &Polly,
    region: &str,
    arn: &str,
    keys: &[&str],
) -> http::Response<Bytes> {
    let body = json!({ "ResourceArn": arn, "TagKeys": keys });
    polly
        .call_in_region(region, "POST", "/v1/untagResource", &body.to_string())
        .await
}

/// The tags `ListTagsForResource` reports, or `None` when it reports no `Tags` member at
/// all — the difference an `as_object().unwrap_or_default()` reader cannot see.
fn reported_tags(case: &str, response: &http::Response<Bytes>) -> Option<BTreeMap<String, String>> {
    assert_eq!(
        response.status(),
        200,
        "case {case:?}: ListTagsForResource must answer, body {:?}",
        body_of(response)
    );
    let body = body_of(response);
    let tags = body.get("Tags")?;
    if tags.is_null() {
        return None;
    }
    Some(
        tags.as_object()
            .unwrap_or_else(|| panic!("case {case:?}: Tags is not an object, got {tags}"))
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect(),
    )
}

/// RULE: `CreateVoice` records a custom voice and answers with it in `pending` status;
/// `GetVoice` reports every member of it afterwards, and `DescribeVoices` reports the
/// subset the model documents for a custom voice — never the recordings' audio, which the
/// service does not keep.
///
/// The rows differ along what the rule generalises over: the language the recordings are
/// in, how many recordings there are, which audio source each one names, and whether the
/// request names a customer managed key.
#[tokio::test]
async fn a_created_voice_round_trips_through_get_voice_and_describe_voices() {
    struct Row {
        case: &'static str,
        name: &'static str,
        language: &'static str,
        recordings: Value,
        /// The `S3Url`s the voice must report back, in order — a recording passed as
        /// `Bytes` reports no field in its `Data` at all.
        reported_urls: Vec<Option<&'static str>>,
        key: Option<&'static str>,
    }

    let rows = vec![
        Row {
            case: "one recording read from Amazon S3",
            name: "Narrator",
            language: "en-US",
            recordings: json!([s3_recording("wav", "s3://recordings/one.wav")]),
            reported_urls: vec![Some("s3://recordings/one.wav")],
            key: None,
        },
        Row {
            case: "one recording whose audio travels in the request",
            name: "In_Band-1",
            language: "de-DE",
            recordings: json!([bytes_recording("mp3")]),
            reported_urls: vec![None],
            key: None,
        },
        Row {
            case: "three recordings of three formats, one of them in band",
            name: "Mixed",
            language: "pt-BR",
            recordings: json!([
                s3_recording("flac", "s3://recordings/a.flac"),
                s3_recording("ogg_vorbis", "s3://recordings/b.ogg"),
                bytes_recording("wav"),
            ]),
            reported_urls: vec![
                Some("s3://recordings/a.flac"),
                Some("s3://recordings/b.ogg"),
                None,
            ],
            key: None,
        },
        Row {
            case: "a voice encrypted with a customer managed key",
            name: "Keyed",
            language: "hi-IN",
            recordings: json!([s3_recording("wav", "s3://recordings/keyed.wav")]),
            reported_urls: vec![Some("s3://recordings/keyed.wav")],
            key: Some("arn:aws:kms:us-east-1:000000000000:key/abcd-1234"),
        },
    ];

    for row in rows {
        let case = row.case;
        // One emulator per row, so a row's own quota and store are its own.
        let polly = Polly::new();
        let mut request = create_request(row.name, row.language, row.recordings.clone());
        if let Some(key) = row.key {
            request["CustomerManagedEncryption"] = json!({ "KmsKeyArn": key });
        }

        let created = created_voice(case, &create_voice(&polly, REGION, &request).await);
        let voice_id = created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {created}"))
            .to_string();
        assert!(
            voice_id.starts_with("vc-"),
            "case {case:?}: a minted custom voice id starts with vc-, got {voice_id:?}"
        );
        assert_eq!(
            created["Status"].as_str(),
            Some(VOICE_STATUS_PENDING),
            "case {case:?}: the new voice is pending, got {created}"
        );
        assert_eq!(
            created["Origin"].as_str(),
            Some("customer"),
            "case {case:?}: a voice the caller created has customer origin, got {created}"
        );
        assert_eq!(
            created["Arn"].as_str(),
            Some(format!("arn:aws:polly:{REGION}:{ACCOUNT}:voice/{voice_id}").as_str()),
            "case {case:?}: wrong ARN, got {created}"
        );
        assert_eq!(
            created["Name"].as_str(),
            Some(row.name),
            "case {case:?}: the voice keeps the name the request chose, got {created}"
        );
        assert_eq!(
            created["LanguageCode"].as_str(),
            Some(row.language),
            "case {case:?}: wrong language, got {created}"
        );
        assert!(
            created["LanguageName"].as_str().is_some_and(|name| !name.is_empty()),
            "case {case:?}: the language must be named in English, got {created}"
        );
        assert_eq!(
            created["SupportedEngines"]
                .as_array()
                .map(|engines| engines.len()),
            Some(CUSTOM_VOICE_ENGINES.len()),
            "case {case:?}: wrong supported engines, got {created}"
        );
        assert!(
            created["Gender"].is_null(),
            "case {case:?}: a custom voice carries no gender, got {created}"
        );

        // The read reports the same voice, with the four members `DescribeVoices` leaves
        // to this operation.
        let read = get_voice(&polly, REGION, &voice_id).await;
        assert_eq!(
            read.status(),
            200,
            "case {case:?}: GetVoice must answer, body {:?}",
            body_of(&read)
        );
        let got = body_of(&read)["Voice"].clone();
        assert_eq!(
            got["Id"].as_str(),
            Some(voice_id.as_str()),
            "case {case:?}: GetVoice must report the created voice, got {got}"
        );
        assert_eq!(
            got["Arn"], created["Arn"],
            "case {case:?}: the two reports must agree about the ARN, got {got}"
        );
        let recordings = got["VoiceCreationData"]["Recordings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            recordings.len(),
            row.reported_urls.len(),
            "case {case:?}: wrong number of recordings, got {got}"
        );
        for (index, expected) in row.reported_urls.iter().enumerate() {
            let data = &recordings[index]["Data"];
            assert_eq!(
                data["S3Url"].as_str(),
                *expected,
                "case {case:?}: recording {index} reports the wrong source, got {data}"
            );
            assert!(
                data["Bytes"].is_null(),
                "case {case:?}: recording {index} must not report the audio, got {data}"
            );
        }
        assert_eq!(
            got["CustomerManagedEncryption"]["KmsKeyArn"].as_str(),
            row.key,
            "case {case:?}: wrong key, got {got}"
        );

        // And the listing reports it, with exactly the members documented for a custom
        // voice there.
        let listed = described(&polly, REGION, "").await;
        let entry = listed
            .get(&voice_id)
            .unwrap_or_else(|| panic!("case {case:?}: DescribeVoices omits the created voice"));
        for member in ["Origin", "Id", "Name", "Status", "LanguageCode", "LanguageName", "CreationTime"] {
            assert!(
                !entry[member].is_null(),
                "case {case:?}: DescribeVoices must report {member} for a custom voice, got {entry}"
            );
        }
        for member in ["Arn", "StatusReason", "VoiceCreationData", "CustomerManagedEncryption"] {
            assert!(
                entry[member].is_null(),
                "case {case:?}: DescribeVoices leaves {member} to GetVoice, got {entry}"
            );
        }
        assert!(
            listed.len() > 1,
            "case {case:?}: the provided voices must still be reported beside the custom one"
        );
    }
}

/// RULE: a voice `CreateVoice` answered with in `pending` status reaches `active` without
/// any further call, and every read of it reports the same status at the same time.
#[tokio::test]
async fn a_pending_voice_becomes_active_without_another_call() {
    let polly = Polly::new();
    let request = create_request(
        "Awaited",
        "fr-FR",
        json!([s3_recording("wav", "s3://recordings/awaited.wav")]),
    );
    let created = created_voice("activation", &create_voice(&polly, REGION, &request).await);
    let voice_id = created["Id"].as_str().expect("a created voice has an id").to_string();

    // The transition is owed by the clock, so the status is still the one the create
    // call answered with when a caller reads straight away.
    let listed = described(&polly, REGION, "").await;
    assert_eq!(
        listed[&voice_id]["Status"].as_str(),
        Some(VOICE_STATUS_PENDING),
        "case \"activation\": a voice read straight after the create is still pending, got {:?}",
        listed[&voice_id]
    );

    let active = awaited_active(&polly, "activation", &voice_id).await;
    assert_eq!(
        active["Status"].as_str(),
        Some(VOICE_STATUS_ACTIVE),
        "case \"activation\": the voice must become active, got {active}"
    );
    let listed = described(&polly, REGION, "").await;
    assert_eq!(
        listed[&voice_id]["Status"].as_str(),
        Some(VOICE_STATUS_ACTIVE),
        "case \"activation\": the listing must report the settled status too, got {:?}",
        listed[&voice_id]
    );
}

/// RULE: a custom voice can be deleted once it is out of `pending` status, and a delete
/// of a voice still in it conflicts; after a delete, every read answers as it does for an
/// id that names no voice.
#[tokio::test]
async fn a_voice_is_deletable_once_it_is_no_longer_pending() {
    let polly = Polly::new();
    let request = create_request(
        "Deleted",
        "it-IT",
        json!([s3_recording("wav", "s3://recordings/deleted.wav")]),
    );
    let created = created_voice("delete", &create_voice(&polly, REGION, &request).await);
    let voice_id = created["Id"].as_str().expect("a created voice has an id").to_string();

    let conflict = delete_voice(&polly, REGION, &voice_id).await;
    assert_eq!(
        conflict.status(),
        409,
        "case \"pending\": deleting a pending voice conflicts, body {:?}",
        body_of(&conflict)
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "case \"pending\": wrong error code, body {:?}",
        body_of(&conflict)
    );
    let document = body_of(&conflict);
    assert_eq!(
        (
            document["resourceId"].as_str(),
            document["resourceType"].as_str()
        ),
        (Some(voice_id.as_str()), Some("voice")),
        "case \"pending\": the refusal must name the resource it conflicted with, got {document}"
    );

    awaited_active(&polly, "delete", &voice_id).await;
    let deleted = delete_voice(&polly, REGION, &voice_id).await;
    assert_eq!(
        deleted.status(),
        200,
        "case \"active\": an active voice is deletable, body {:?}",
        body_of(&deleted)
    );

    let gone = get_voice(&polly, REGION, &voice_id).await;
    assert_eq!(
        gone.status(),
        404,
        "case \"deleted\": GetVoice stops returning the voice, body {:?}",
        body_of(&gone)
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "case \"deleted\": wrong error code, body {:?}",
        body_of(&gone)
    );
    assert!(
        !described(&polly, REGION, "").await.contains_key(&voice_id),
        "case \"deleted\": DescribeVoices stops returning the voice"
    );
    let twice = delete_voice(&polly, REGION, &voice_id).await;
    assert_eq!(
        twice.status(),
        404,
        "case \"deleted twice\": a deleted voice is gone, body {:?}",
        body_of(&twice)
    );
}

/// RULE: a custom voice belongs to the account AND Region its create call was signed for,
/// so a call signed for another Region neither reads it, lists it nor deletes it.
#[tokio::test]
async fn a_custom_voice_belongs_to_the_region_it_was_created_in() {
    let polly = Polly::new();
    let other = "eu-west-1";
    let request = create_request(
        "Regional",
        "es-US",
        json!([s3_recording("wav", "s3://recordings/regional.wav")]),
    );
    let created = created_voice("regional", &create_voice(&polly, REGION, &request).await);
    let voice_id = created["Id"].as_str().expect("a created voice has an id").to_string();

    for (case, response) in [
        ("GetVoice", get_voice(&polly, other, &voice_id).await),
        ("DeleteVoice", delete_voice(&polly, other, &voice_id).await),
    ] {
        assert_eq!(
            response.status(),
            404,
            "case {case:?}: another Region holds no such voice, body {:?}",
            body_of(&response)
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code, body {:?}",
            body_of(&response)
        );
    }
    assert!(
        !described(&polly, other, "").await.contains_key(&voice_id),
        "case \"DescribeVoices\": another Region does not list the voice"
    );
    assert!(
        described(&polly, REGION, "").await.contains_key(&voice_id),
        "case \"DescribeVoices\": the creating Region still lists it"
    );
}

/// RULE: the `LanguageCode` and `Engine` filters of `DescribeVoices` narrow custom voices
/// the same way they narrow the voices Amazon Polly provides.
///
/// The rows span the class: the voice's own language, another language it does not speak,
/// the engine custom voices are offered on and each engine they are not.
#[tokio::test]
async fn the_listing_filters_narrow_a_custom_voice_the_same_way() {
    let polly = Polly::new();
    let language = "de-DE";
    let request = create_request(
        "Filtered",
        language,
        json!([s3_recording("wav", "s3://recordings/filtered.wav")]),
    );
    let created = created_voice("filters", &create_voice(&polly, REGION, &request).await);
    let voice_id = created["Id"].as_str().expect("a created voice has an id").to_string();

    let engine = CUSTOM_VOICE_ENGINES[0];
    let rows: Vec<(String, String, bool)> = vec![
        ("no filter".to_string(), String::new(), true),
        (
            "its own language".to_string(),
            format!("?LanguageCode={language}"),
            true,
        ),
        (
            "a language it does not speak".to_string(),
            "?LanguageCode=ja-JP".to_string(),
            false,
        ),
        (
            "the engine custom voices are offered on".to_string(),
            format!("?Engine={engine}"),
            true,
        ),
        (
            "an engine they are not offered on".to_string(),
            "?Engine=standard".to_string(),
            false,
        ),
        (
            "another engine they are not offered on".to_string(),
            "?Engine=long-form".to_string(),
            false,
        ),
        (
            "its language and its engine together".to_string(),
            format!("?LanguageCode={language}&Engine={engine}"),
            true,
        ),
        (
            "its language and an engine it is not offered on".to_string(),
            format!("?LanguageCode={language}&Engine=neural"),
            false,
        ),
    ];

    for (case, query, expected) in rows {
        let listed = described(&polly, REGION, &query).await;
        assert_eq!(
            listed.contains_key(&voice_id),
            expected,
            "case {case:?}: query {query:?} reported the custom voice wrongly, got {} rows",
            listed.len()
        );
    }
}

/// RULE: a request the model admits but the service's own rules refuse is a 400
/// `ValidationException` naming the member that failed — never a success and never a 501.
///
/// The rows span the two rules `CreateVoice` states beyond the model: the terms have to be
/// accepted, and a recording names exactly one audio source.
#[tokio::test]
async fn a_request_outside_the_services_own_rules_is_refused_by_member() {
    let polly = Polly::new();
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "the terms left at their default",
            json!({
                "Name": "Refused",
                "LanguageCode": "en-US",
                "TermsAccepted": false,
                "VoiceCreationData": {
                    "Recordings": [s3_recording("wav", "s3://recordings/a.wav")],
                },
            }),
            "termsAccepted",
        ),
        (
            "a recording naming no audio source",
            json!({
                "Name": "Refused",
                "LanguageCode": "en-US",
                "TermsAccepted": true,
                "VoiceCreationData": { "Recordings": [{ "Format": "wav", "Data": {} }] },
            }),
            "voiceCreationData.recordings.1.member.data",
        ),
        (
            "a recording naming both audio sources",
            json!({
                "Name": "Refused",
                "LanguageCode": "en-US",
                "TermsAccepted": true,
                "VoiceCreationData": { "Recordings": [{
                    "Format": "wav",
                    "Data": { "S3Url": "s3://recordings/a.wav", "Bytes": "AAECAw==" },
                }] },
            }),
            "voiceCreationData.recordings.1.member.data",
        ),
        (
            "the second recording of three, naming no audio source",
            json!({
                "Name": "Refused",
                "LanguageCode": "en-US",
                "TermsAccepted": true,
                "VoiceCreationData": { "Recordings": [
                    s3_recording("wav", "s3://recordings/a.wav"),
                    { "Format": "wav", "Data": {} },
                    bytes_recording("wav"),
                ] },
            }),
            "voiceCreationData.recordings.2.member.data",
        ),
    ];

    for (case, request, member) in rows {
        let response = create_voice(&polly, REGION, &request).await;
        assert_eq!(
            response.status(),
            400,
            "case {case:?}: the request must be refused, body {:?}",
            body_of(&response)
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: wrong error code, body {:?}",
            body_of(&response)
        );
        let document = body_of(&response);
        assert_eq!(
            document["reason"].as_str(),
            Some("fieldValidationFailed"),
            "case {case:?}: a member outside the rules is a field validation failure, got \
             {document}"
        );
        let named: Vec<String> = document["fields"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|field| field["name"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            named,
            vec![member.to_string()],
            "case {case:?}: the refusal must name the member that failed, got {document}"
        );
        let text = message(&response);
        assert!(
            text.contains(member) && text.starts_with("1 validation error detected:"),
            "case {case:?}: the refusal must read as the boundary words one, got {text:?}"
        );
    }
    assert!(
        described(&polly, REGION, "")
            .await
            .keys()
            .all(|id| !id.starts_with("vc-")),
        "a refused create must leave no voice behind"
    );
}

/// RULE: every language `CreateVoice` admits creates a voice, and the voice reports that
/// language with its English name.
#[tokio::test]
async fn every_modelled_custom_voice_language_creates_a_voice() {
    // The whole value set of the model's `CustomVoiceLanguageCode`
    // (`smithy_query(query="shape", shape="CustomVoiceLanguageCode")`), through the
    // `*_VALUES` constant rendered from it — so a language the model adds is covered
    // here the moment it is rendered.
    for language in CUSTOM_VOICE_LANGUAGE_CODE_VALUES {
        let polly = Polly::new();
        let request = create_request(
            "Polyglot",
            language,
            json!([s3_recording("wav", "s3://recordings/polyglot.wav")]),
        );
        let created = created_voice(language, &create_voice(&polly, REGION, &request).await);
        assert_eq!(
            created["LanguageCode"].as_str(),
            Some(*language),
            "case {language:?}: wrong language, got {created}"
        );
        assert!(
            created["LanguageName"]
                .as_str()
                .is_some_and(|name| !name.is_empty()),
            "case {language:?}: the language must be named in English, got {created}"
        );
    }
}

/// RULE: the number of custom voices one account holds in one Region is capped, and the
/// create that would pass the cap is refused with the quota the model names — in that
/// Region only.
#[tokio::test]
async fn the_custom_voice_quota_refuses_the_voice_past_it() {
    let polly = Polly::new();
    for index in 0..MAX_CUSTOM_VOICES {
        let request = create_request(
            "Quota",
            "en-US",
            json!([s3_recording("wav", "s3://recordings/quota.wav")]),
        );
        let response = create_voice(&polly, REGION, &request).await;
        assert_eq!(
            response.status(),
            200,
            "case \"voice {index}\": a voice within the quota must be created, body {:?}",
            body_of(&response)
        );
    }

    let request = create_request(
        "Quota",
        "en-US",
        json!([s3_recording("wav", "s3://recordings/quota.wav")]),
    );
    let refused = create_voice(&polly, REGION, &request).await;
    assert_eq!(
        refused.status(),
        402,
        "case \"past the quota\": the create must be refused, body {:?}",
        body_of(&refused)
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case \"past the quota\": wrong error code, body {:?}",
        body_of(&refused)
    );
    let document = body_of(&refused);
    assert_eq!(
        (
            document["quotaCode"].as_str(),
            document["serviceCode"].as_str()
        ),
        (Some("custom-voice-max-count"), Some("polly")),
        "case \"past the quota\": the refusal must name the quota, got {document}"
    );

    // The quota is the REGION's own, so another Region still takes a voice.
    let elsewhere = create_voice(&polly, "eu-west-1", &request).await;
    assert_eq!(
        elsewhere.status(),
        200,
        "case \"another Region\": a Region at its cap must not stop another, body {:?}",
        body_of(&elsewhere)
    );
}

/// RULE: a retried create carrying the client token of a voice this Region already
/// created answers with that voice and creates no second one; the same token with other
/// parameters is refused, because one token cannot name two voices.
#[tokio::test]
async fn one_client_token_creates_one_voice() {
    let polly = Polly::new();
    let token = "a-client-token-1";
    let mut request = create_request(
        "Idempotent",
        "en-US",
        json!([s3_recording("wav", "s3://recordings/idempotent.wav")]),
    );
    request["ClientToken"] = json!(token);

    let first = created_voice("first call", &create_voice(&polly, REGION, &request).await);
    let retry = created_voice("retried call", &create_voice(&polly, REGION, &request).await);
    assert_eq!(
        retry["Id"], first["Id"],
        "case \"retried call\": the retry must answer with the voice the first call created, \
         got {retry}"
    );
    let custom: Vec<String> = described(&polly, REGION, "")
        .await
        .into_keys()
        .filter(|id| id.starts_with("vc-"))
        .collect();
    assert_eq!(
        custom.len(),
        1,
        "case \"retried call\": one token must leave one voice, got {custom:?}"
    );

    let mut other = create_request(
        "Different",
        "en-US",
        json!([s3_recording("wav", "s3://recordings/other.wav")]),
    );
    other["ClientToken"] = json!(token);
    let refused = create_voice(&polly, REGION, &other).await;
    assert_eq!(
        refused.status(),
        400,
        "case \"same token, other parameters\": the create must be refused, body {:?}",
        body_of(&refused)
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case \"same token, other parameters\": wrong error code, body {:?}",
        body_of(&refused)
    );

    // A token this Region has not seen creates its own voice.
    let mut fresh = create_request(
        "Fresh",
        "en-US",
        json!([s3_recording("wav", "s3://recordings/fresh.wav")]),
    );
    fresh["ClientToken"] = json!("a-client-token-2");
    let second = created_voice("another token", &create_voice(&polly, REGION, &fresh).await);
    assert_ne!(
        second["Id"], first["Id"],
        "case \"another token\": a token of its own creates its own voice, got {second}"
    );
}

/// RULE: a tag set lives on the resource its ARN addresses, for every resource type this
/// service tags: the creating call's tags are what the listing reports, `TagResource`
/// merges into them and replaces a key it already carries, `UntagResource` removes the
/// keys it names, and a resource with no tags reports no `Tags` member at all.
///
/// The rows differ along the resource TYPE, which is what the rule generalises over: a
/// lexicon tagged by `PutLexicon` and a custom voice tagged by `CreateVoice`.
#[tokio::test]
async fn tags_round_trip_for_every_resource_this_service_tags() {
    let polly = Polly::new();
    let document = pls("ipa", "en-US", 1);

    // A lexicon created with tags.
    let lexicon_body = json!({
        "Content": document,
        "Tags": { "Project": "Parrot", "Tier": "test" },
    });
    let put = polly
        .call_in_region(REGION, "PUT", "/v1/lexicons/Tagged", &lexicon_body.to_string())
        .await;
    assert_eq!(
        put.status(),
        200,
        "PutLexicon with tags must answer, body {:?}",
        body_of(&put)
    );

    // A custom voice created with tags.
    let mut voice_request = create_request(
        "Tagged",
        "en-US",
        json!([s3_recording("wav", "s3://recordings/tagged.wav")]),
    );
    voice_request["Tags"] = json!({ "Project": "Parrot", "Tier": "test" });
    let voice = created_voice("voice", &create_voice(&polly, REGION, &voice_request).await);
    let voice_id = voice["Id"].as_str().expect("a created voice has an id").to_string();

    // An untagged resource of each type, for the absent-member half of the rule.
    let bare = polly
        .call_in_region(
            REGION,
            "PUT",
            "/v1/lexicons/Bare",
            &json!({ "Content": document }).to_string(),
        )
        .await;
    assert_eq!(bare.status(), 200, "PutLexicon: body {:?}", body_of(&bare));
    let bare_voice = created_voice(
        "bare voice",
        &create_voice(
            &polly,
            REGION,
            &create_request(
                "Bare",
                "en-US",
                json!([s3_recording("wav", "s3://recordings/bare.wav")]),
            ),
        )
        .await,
    );
    let bare_voice_id = bare_voice["Id"].as_str().expect("an id").to_string();

    let rows: Vec<(&str, String, String)> = vec![
        ("lexicon", lexicon_arn(REGION, "Tagged"), lexicon_arn(REGION, "Bare")),
        (
            "voice",
            format!("arn:aws:polly:{REGION}:{ACCOUNT}:voice/{voice_id}"),
            format!("arn:aws:polly:{REGION}:{ACCOUNT}:voice/{bare_voice_id}"),
        ),
    ];

    for (case, arn, untagged) in rows {
        let created_with: BTreeMap<String, String> = [
            ("Project".to_string(), "Parrot".to_string()),
            ("Tier".to_string(), "test".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            reported_tags(case, &list_tags(&polly, REGION, &arn).await),
            Some(created_with),
            "case {case:?}: the creating call's tags must be what the listing reports"
        );

        // A merge: one new key, one key the resource already carries.
        let tagged = tag_resource(
            &polly,
            REGION,
            &arn,
            &json!({ "Tier": "staging", "Owner": "voices" }),
        )
        .await;
        assert_eq!(
            tagged.status(),
            200,
            "case {case:?}: TagResource must answer, body {:?}",
            body_of(&tagged)
        );
        let expected: BTreeMap<String, String> = [
            ("Project".to_string(), "Parrot".to_string()),
            ("Tier".to_string(), "staging".to_string()),
            ("Owner".to_string(), "voices".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            reported_tags(case, &list_tags(&polly, REGION, &arn).await),
            Some(expected),
            "case {case:?}: a tag call must merge and replace, not replace the whole set"
        );

        // An untag of one stored key and one the resource does not carry.
        let untagged_response = untag_resource(&polly, REGION, &arn, &["Tier", "Absent"]).await;
        assert_eq!(
            untagged_response.status(),
            200,
            "case {case:?}: UntagResource must answer, body {:?}",
            body_of(&untagged_response)
        );
        let expected: BTreeMap<String, String> = [
            ("Project".to_string(), "Parrot".to_string()),
            ("Owner".to_string(), "voices".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            reported_tags(case, &list_tags(&polly, REGION, &arn).await),
            Some(expected),
            "case {case:?}: an untag must remove the keys it names and nothing else"
        );

        // And a resource nobody tagged reports no map at all.
        assert_eq!(
            reported_tags(case, &list_tags(&polly, REGION, &untagged).await),
            None,
            "case {case:?}: a resource with no tags reports no Tags member"
        );
    }

    // A later put of the same lexicon leaves its tags where they are.
    let again = polly
        .call_in_region(
            REGION,
            "PUT",
            "/v1/lexicons/Tagged",
            &json!({ "Content": document, "Tags": { "Replaced": "no" } }).to_string(),
        )
        .await;
    assert_eq!(
        again.status(),
        200,
        "case \"overwritten lexicon\": the put must answer, body {:?}",
        body_of(&again)
    );
    let after = reported_tags(
        "overwritten lexicon",
        &list_tags(&polly, REGION, &lexicon_arn(REGION, "Tagged")).await,
    );
    assert_eq!(
        after
            .as_ref()
            .map(|tags| tags.keys().cloned().collect::<Vec<String>>()),
        Some(vec!["Owner".to_string(), "Project".to_string()]),
        "case \"overwritten lexicon\": a put must not replace the tags of an existing lexicon, \
         got {after:?}"
    );
}

/// RULE: an ARN that addresses no resource of this service is a bad REQUEST
/// (`InvalidTagParameterException`), and one that addresses a resource this account and
/// Region does not hold is `ResourceNotFoundException` — the same two answers from every
/// tagging operation.
///
/// The rows span the forms an ARN can be wrong in: not an ARN, too few fields, another
/// vendor, another account, another Region, a resource type this service does not tag,
/// and the type with no identifier.
#[tokio::test]
async fn an_arn_that_addresses_nothing_is_refused_by_every_tagging_operation() {
    let polly = Polly::new();
    let document = pls("ipa", "en-US", 1);
    let put = polly
        .call_in_region(
            REGION,
            "PUT",
            "/v1/lexicons/Addressed",
            &json!({ "Content": document }).to_string(),
        )
        .await;
    assert_eq!(put.status(), 200, "PutLexicon: body {:?}", body_of(&put));

    let invalid: Vec<(&str, String)> = vec![
        ("not an ARN at all", "Addressed".to_string()),
        (
            "an ARN missing its fields",
            "arn:aws:polly:lexicon/Addressed".to_string(),
        ),
        (
            "another vendor's resource",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:lexicon/Addressed"),
        ),
        (
            "another account's resource",
            format!("arn:aws:polly:{REGION}:111122223333:lexicon/Addressed"),
        ),
        (
            "another Region's resource",
            lexicon_arn("eu-west-1", "Addressed"),
        ),
        (
            "a resource type this service does not tag",
            format!("arn:aws:polly:{REGION}:{ACCOUNT}:synthesisTask/abc"),
        ),
        (
            "the resource type with no identifier",
            format!("arn:aws:polly:{REGION}:{ACCOUNT}:lexicon"),
        ),
    ];

    for (case, arn) in invalid {
        for (operation, response) in [
            ("ListTagsForResource", list_tags(&polly, REGION, &arn).await),
            (
                "TagResource",
                tag_resource(&polly, REGION, &arn, &json!({ "Project": "Parrot" })).await,
            ),
            (
                "UntagResource",
                untag_resource(&polly, REGION, &arn, &["Project"]).await,
            ),
        ] {
            let case = format!("{operation}: {case}");
            assert_eq!(
                response.status(),
                400,
                "case {case:?}: ARN {arn:?} must be refused, body {:?}",
                body_of(&response)
            );
            assert_eq!(
                error_code(&response),
                "InvalidTagParameterException",
                "case {case:?}: wrong error code, body {:?}",
                body_of(&response)
            );
        }
    }

    // Well formed, and naming a resource this Region does not hold.
    for (operation, response) in [
        (
            "ListTagsForResource",
            list_tags(&polly, REGION, &lexicon_arn(REGION, "Absent")).await,
        ),
        (
            "TagResource",
            tag_resource(
                &polly,
                REGION,
                &lexicon_arn(REGION, "Absent"),
                &json!({ "Project": "Parrot" }),
            )
            .await,
        ),
        (
            "UntagResource",
            untag_resource(&polly, REGION, &lexicon_arn(REGION, "Absent"), &["Project"]).await,
        ),
    ] {
        let case = format!("{operation}: a resource this Region does not hold");
        assert_eq!(
            response.status(),
            404,
            "case {case:?}: the resource must be reported missing, body {:?}",
            body_of(&response)
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code, body {:?}",
            body_of(&response)
        );
        let document = body_of(&response);
        assert_eq!(
            (
                document["resourceId"].as_str(),
                document["resourceType"].as_str()
            ),
            (Some("Absent"), Some("lexicon")),
            "case {case:?}: the refusal must name what was looked for, got {document}"
        );
    }
}

/// RULE: how much one stored voice can hold is bounded by the MODEL, so the request
/// boundary refuses a recording list longer than `RecordingList`'s maximum, or a recording
/// URL longer than `RecordingData$S3Url`'s, before the handler stores anything — and
/// accepts each of them exactly at its published bound.
///
/// The rows are each published bound and one past it, which is what a bound has to be
/// checked on.
#[tokio::test]
async fn the_modelled_recording_bounds_are_enforced_before_a_voice_is_stored() {
    // The two maxima the model publishes for the recordings a create may carry
    // (`smithy_query(query="shape", shape="RecordingList")` -> @length 1..10;
    // `smithy_query(query="shape", shape="RecordingData")` -> S3Url @length 0..1024),
    // named through the constants the handler cites for them.
    assert_eq!(
        (MAX_STORED_RECORDINGS, MAX_STORED_RECORDING_URL_CHARACTERS),
        (10, 1024),
        "the constants the handler cites must be the model's own maxima"
    );

    /// An `s3://` URL of exactly `characters` characters, which the modelled pattern
    /// admits (a bucket label and a non-empty key).
    fn url_of(characters: usize) -> String {
        const PREFIX: &str = "s3://recordings/";
        format!("{PREFIX}{}", "x".repeat(characters - PREFIX.len()))
    }

    let recordings_of = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| s3_recording("wav", &format!("s3://recordings/{index}.wav")))
                .collect(),
        )
    };

    let rows: Vec<(String, Value, bool)> = vec![
        (
            format!("{MAX_STORED_RECORDINGS} recordings, the published maximum"),
            recordings_of(MAX_STORED_RECORDINGS),
            true,
        ),
        (
            format!("{} recordings, one past it", MAX_STORED_RECORDINGS + 1),
            recordings_of(MAX_STORED_RECORDINGS + 1),
            false,
        ),
        (
            "one recording, the published minimum".to_string(),
            recordings_of(1),
            true,
        ),
        (
            "no recordings at all, one below it".to_string(),
            recordings_of(0),
            false,
        ),
        (
            format!("a URL of {MAX_STORED_RECORDING_URL_CHARACTERS} characters, the published maximum"),
            json!([s3_recording("wav", &url_of(MAX_STORED_RECORDING_URL_CHARACTERS))]),
            true,
        ),
        (
            format!(
                "a URL of {} characters, one past it",
                MAX_STORED_RECORDING_URL_CHARACTERS + 1
            ),
            json!([s3_recording("wav", &url_of(MAX_STORED_RECORDING_URL_CHARACTERS + 1))]),
            false,
        ),
    ];

    for (case, recordings, accepted) in rows {
        let polly = Polly::new();
        let request = create_request("Bounded", "en-US", recordings);
        let response = create_voice(&polly, REGION, &request).await;
        if accepted {
            assert_eq!(
                response.status(),
                200,
                "case {case:?}: a request at the published bound must be served, body {:?}",
                body_of(&response)
            );
            continue;
        }
        assert_eq!(
            response.status(),
            400,
            "case {case:?}: a request past the published bound must be refused, body {:?}",
            body_of(&response)
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: wrong error code, body {:?}",
            body_of(&response)
        );
        assert!(
            described(&polly, REGION, "")
                .await
                .keys()
                .all(|id| !id.starts_with("vc-")),
            "case {case:?}: a refused create must store no voice"
        );
    }
}

/// RULE: a tag key in the prefix AWS reserves is refused wherever tags are applied, and a
/// request that would leave a resource holding more tags than one may carry is refused
/// with the tag-count error — neither leaving any tag behind.
#[tokio::test]
async fn a_reserved_key_and_a_set_past_the_limit_are_refused_wherever_tags_are_applied() {
    let polly = Polly::new();
    let document = pls("ipa", "en-US", 1);
    let put = polly
        .call_in_region(
            REGION,
            "PUT",
            "/v1/lexicons/Limits",
            &json!({ "Content": document }).to_string(),
        )
        .await;
    assert_eq!(put.status(), 200, "PutLexicon: body {:?}", body_of(&put));
    let arn = lexicon_arn(REGION, "Limits");

    // One tag per key, as many as the limit allows plus one.
    let over: Value = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key{index}"), Value::String(index.to_string())))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let at_limit: Value = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key{index}"), Value::String(index.to_string())))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let reserved = json!({ "aws:created-by": "someone" });

    // On the creating calls.
    let rows: Vec<(&str, Value, &str, u16)> = vec![
        (
            "PutLexicon with a reserved key",
            json!({ "Content": document, "Tags": reserved }),
            "InvalidTagParameterException",
            400,
        ),
        (
            "PutLexicon past the tag limit",
            json!({ "Content": document, "Tags": over }),
            "TooManyTagsException",
            400,
        ),
    ];
    for (case, body, expected_code, status) in rows {
        let response = polly
            .call_in_region(REGION, "PUT", "/v1/lexicons/Refused", &body.to_string())
            .await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case:?}: the put must be refused, body {:?}",
            body_of(&response)
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: wrong error code, body {:?}",
            body_of(&response)
        );
        let listed = list_tags(&polly, REGION, &lexicon_arn(REGION, "Refused")).await;
        assert_eq!(
            listed.status(),
            404,
            "case {case:?}: a refused put must store no lexicon, body {:?}",
            body_of(&listed)
        );
    }

    let mut voice_request = create_request(
        "Limits",
        "en-US",
        json!([s3_recording("wav", "s3://recordings/limits.wav")]),
    );
    voice_request["Tags"] = json!({ "aws:created-by": "someone" });
    let refused = create_voice(&polly, REGION, &voice_request).await;
    assert_eq!(
        refused.status(),
        400,
        "case \"CreateVoice with a reserved key\": the create must be refused, body {:?}",
        body_of(&refused)
    );
    assert_eq!(
        error_code(&refused),
        "InvalidTagParameterException",
        "case \"CreateVoice with a reserved key\": wrong error code, body {:?}",
        body_of(&refused)
    );

    // And on the tagging call, where the limit counts the union with what is stored.
    for (case, tags, expected_code) in [
        (
            "TagResource with a reserved key",
            reserved.clone(),
            "InvalidTagParameterException",
        ),
        (
            "TagResource past the tag limit",
            over.clone(),
            "TooManyTagsException",
        ),
    ] {
        let response = tag_resource(&polly, REGION, &arn, &tags).await;
        assert_eq!(
            response.status(),
            400,
            "case {case:?}: the tag call must be refused, body {:?}",
            body_of(&response)
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: wrong error code, body {:?}",
            body_of(&response)
        );
        assert_eq!(
            reported_tags(case, &list_tags(&polly, REGION, &arn).await),
            None,
            "case {case:?}: a refused tag call must leave no tag behind"
        );
    }

    // Exactly as many tags as a resource may carry is accepted, and one more is not.
    let accepted = tag_resource(&polly, REGION, &arn, &at_limit).await;
    assert_eq!(
        accepted.status(),
        200,
        "case \"at the tag limit\": the tag call must be accepted, body {:?}",
        body_of(&accepted)
    );
    assert_eq!(
        reported_tags("at the tag limit", &list_tags(&polly, REGION, &arn).await)
            .map(|tags| tags.len()),
        Some(MAX_TAGS_PER_RESOURCE),
        "case \"at the tag limit\": every requested tag must be stored"
    );
    let one_more = tag_resource(&polly, REGION, &arn, &json!({ "extra": "one" })).await;
    assert_eq!(
        error_code(&one_more),
        "TooManyTagsException",
        "case \"one past the tag limit\": wrong error code, body {:?}",
        body_of(&one_more)
    );
    // Replacing a key the resource already carries adds no entry, so it still holds.
    let replaced = tag_resource(&polly, REGION, &arn, &json!({ "key0": "again" })).await;
    assert_eq!(
        replaced.status(),
        200,
        "case \"replacing a stored key at the limit\": the tag call must be accepted, body {:?}",
        body_of(&replaced)
    );
}
