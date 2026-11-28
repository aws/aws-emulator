// SPDX-License-Identifier: CC0-1.0

//! What `StartSpeechSynthesisTask` records, and what the task reads then report.
//!
//! RULE: a started synthesis task is HELD by the service, so the task reads report it
//! with the record the start call made from the request — its minted `TaskId`, the
//! engine, output format, text type, voice and billed character count the request
//! resolved to, the output location in the bucket the request named, the creation time,
//! and the optional members only where the request set them — until its retention
//! window runs out. The task is created in a non-terminal status and the CLOCK is what
//! moves it to its terminal one; a request the service refuses records no task at all.
//!
//! The rule is checked over a TABLE rather than the one request a fixture quoted: the
//! rows differ along what it generalises over — the output format (and so the output
//! object's suffix), the engine, the text type (and so what is billed), the voice and
//! its language, the key prefix, and the Region the call is signed for.
//!
//! RULE: the output location a task reports (`SynthesisTask$OutputUri`, the "Pathway for
//! the output speech file") is an address the CALLER follows, so it is rooted at an address
//! this emulator is REACHED at — the emulator's own endpoint the answering request arrived
//! on, else the address this process is served at — and never at a host AWS publishes, with
//! the object addressed path-style beneath it, `/<bucket>/<key-prefix><task-id>.<suffix>`.
//! Every serialization of the task envelope reports that one location, and following it
//! reaches the object the completed task wrote. The rows differ along the address the caller
//! reached the emulator at, the base this process is served at, the output format, the key
//! prefix and the Region.

use std::collections::BTreeSet;
use std::time::Duration;

use bytes::Bytes;
use service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES;
use smithy_server_runtime::UNSERVED_URL_BASE;

use crate::lexicons_and_speech::{error_code, json, pls, Polly};
use crate::support::get_task;
use crate::synthesis_tasks::{list, task_ids};
use service_com_amazonaws_polly_parrot_v1::state::TASK_COMPLETION_DELAY;

/// The status a task is created in, out of the model's `TaskStatus` values
/// (`scheduled | inProgress | completed | failed`).
const CREATED_STATUS: &str = "scheduled";

/// The status the clock moves a started task to.
const TERMINAL_STATUS: &str = "completed";

/// The bucket the rows write their output to.
const OUTPUT_BUCKET: &str = "polly-output";

/// One `StartSpeechSynthesisTask` call, signed for `region`.
async fn start(
    polly: &Polly,
    region: &str,
    request: &serde_json::Value,
) -> http::Response<Bytes> {
    polly
        .call_in_region(region, "POST", "/v1/synthesisTasks", &request.to_string())
        .await
}

/// What one row expects the task record to carry.
struct Expected {
    /// The Region the call is signed for, which the output location names.
    region: &'static str,
    /// `SynthesisTask$OutputFormat`.
    output_format: &'static str,
    /// The suffix the output object carries for that format.
    suffix: &'static str,
    /// The key prefix the request asked for, as the object key begins with.
    key_prefix: &'static str,
    /// `SynthesisTask$Engine`.
    engine: &'static str,
    /// `SynthesisTask$TextType`.
    text_type: &'static str,
    /// `SynthesisTask$VoiceId`.
    voice_id: &'static str,
    /// `SynthesisTask$RequestCharacters` — the BILLED characters, which for `ssml`
    /// input excludes the markup.
    request_characters: i64,
}

/// Assert one reported `SynthesisTask` against what the row expects, except for its
/// status (which the clock owns) — and answer the `TaskId` it reported.
fn assert_record(case: &str, reported: &serde_json::Value, expected: &Expected) -> String {
    let task_id = reported["TaskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {reported:?}"))
        .to_string();
    assert!(
        !task_id.is_empty(),
        "case {case:?}: the task must carry a minted TaskId, got {reported:?}"
    );
    // The location is announced on the endpoint the call reached, which for a request
    // addressed to one of this service's published hosts on an emulator served on no port
    // is the base `OperationContext::endpoint` reports. The announced-location suites below
    // are the ones that vary that address and the base this process is served at.
    let expected_uri = format!(
        "{UNSERVED_URL_BASE}/{OUTPUT_BUCKET}/{}{task_id}.{}",
        expected.key_prefix, expected.suffix
    );
    for (member, actual, want) in [
        ("OutputFormat", reported["OutputFormat"].as_str(), Some(expected.output_format)),
        ("Engine", reported["Engine"].as_str(), Some(expected.engine)),
        ("TextType", reported["TextType"].as_str(), Some(expected.text_type)),
        ("VoiceId", reported["VoiceId"].as_str(), Some(expected.voice_id)),
        ("OutputUri", reported["OutputUri"].as_str(), Some(expected_uri.as_str())),
    ] {
        assert_eq!(
            actual, want,
            "case {case:?}: member {member} is {actual:?}, expected {want:?}"
        );
    }
    assert_eq!(
        reported["RequestCharacters"].as_i64(),
        Some(expected.request_characters),
        "case {case:?}: wrong RequestCharacters, got {reported:?}"
    );
    assert!(
        reported["CreationTime"].as_f64().is_some_and(|t| t > 0.0),
        "case {case:?}: the task must report when it was started, got {reported:?}"
    );
    task_id
}

/// The rows: one request each, with what the task record must then carry.
fn rows() -> Vec<(&'static str, serde_json::Value, Expected)> {
    vec![
        (
            "plain text to mp3, no prefix",
            serde_json::json!({
                "Text": "Hello from the emulator.",
                "VoiceId": "Matthew",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            Expected {
                region: "us-east-1",
                output_format: "mp3",
                suffix: "mp3",
                key_prefix: "",
                // `StartSpeechSynthesisTaskInput$Engine` defaults to the standard
                // engine when the request names none.
                engine: "standard",
                // `TextType` "defaults to plain text".
                text_type: "text",
                voice_id: "Matthew",
                request_characters: 24,
            },
        ),
        (
            "a neural voice, an Ogg format and a key prefix, in another Region",
            serde_json::json!({
                "Text": "Twelve chars",
                "VoiceId": "Joanna",
                "Engine": "neural",
                "OutputFormat": "ogg_opus",
                "OutputS3BucketName": OUTPUT_BUCKET,
                "OutputS3KeyPrefix": "audio/2026/",
            }),
            Expected {
                region: "eu-west-1",
                output_format: "ogg_opus",
                suffix: "ogg",
                key_prefix: "audio/2026/",
                engine: "neural",
                text_type: "text",
                voice_id: "Joanna",
                request_characters: 12,
            },
        ),
        (
            "ssml input, whose markup is not billed",
            serde_json::json!({
                "Text": "<speak>Hello there.</speak>",
                "TextType": "ssml",
                "VoiceId": "Matthew",
                "OutputFormat": "pcm",
                "SampleRate": "8000",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            Expected {
                region: "us-east-1",
                output_format: "pcm",
                suffix: "pcm",
                key_prefix: "",
                engine: "standard",
                text_type: "ssml",
                voice_id: "Matthew",
                // "SSML tags are not counted as billed characters": only "Hello there."
                request_characters: 12,
            },
        ),
        (
            "speech marks, which ride the json output",
            serde_json::json!({
                "Text": "Hello world. Bye.",
                "VoiceId": "Joanna",
                "OutputFormat": "json",
                "SpeechMarkTypes": ["sentence", "word"],
                "OutputS3BucketName": OUTPUT_BUCKET,
                "OutputS3KeyPrefix": "marks-",
            }),
            Expected {
                region: "us-east-1",
                output_format: "json",
                suffix: "marks",
                key_prefix: "marks-",
                engine: "standard",
                text_type: "text",
                voice_id: "Joanna",
                request_characters: 17,
            },
        ),
        (
            "a bilingual voice with its other language",
            serde_json::json!({
                "Text": "Namaste",
                "VoiceId": "Aditi",
                "LanguageCode": "hi-IN",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            Expected {
                region: "us-east-1",
                output_format: "mp3",
                suffix: "mp3",
                key_prefix: "",
                engine: "standard",
                text_type: "text",
                voice_id: "Aditi",
                request_characters: 7,
            },
        ),
    ]
}

/// RULE: the start call records the task, both task reads report that record, and the
/// identifiers it mints are distinct.
#[tokio::test]
async fn a_started_task_is_held_and_reported_by_both_reads() {
    let polly = Polly::new();
    let mut started: Vec<(String, String)> = Vec::new();

    for (case, request, expected) in rows() {
        let response = start(&polly, expected.region, &request).await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: the task must be created, body {:?}",
            json(&response)
        );
        let body = json(&response);
        let task_id = assert_record(case, &body["SynthesisTask"], &expected);
        assert_eq!(
            body["SynthesisTask"]["TaskStatus"].as_str(),
            Some(CREATED_STATUS),
            "case {case:?}: a just-started task is not synthesized yet, got {body:?}"
        );

        // The same record reads back through GetSpeechSynthesisTask: the task is HELD,
        // not just echoed by the call that made it.
        let read = get_task(&polly, &task_id).await;
        assert_eq!(
            read.status(),
            200,
            "case {case:?}: the started task must read back, body {:?}",
            json(&read)
        );
        assert_record(case, &json(&read)["SynthesisTask"], &expected);
        started.push((case.to_string(), task_id));
    }

    // Every identifier differs, or the rows above would be one task read five times.
    for (index, (case, task_id)) in started.iter().enumerate() {
        assert!(
            started
                .iter()
                .enumerate()
                .all(|(other, (_, id))| other == index || id != task_id),
            "case {case:?}: the minted TaskId {task_id:?} was handed out twice"
        );
    }

    // And the listing reports every one of them.
    let listed = task_ids(&json(&list(&polly, "").await));
    for (case, task_id) in &started {
        assert!(
            listed.contains(task_id),
            "case {case:?}: the listing must report the started task {task_id:?}, \
             got {listed:?}"
        );
    }
    assert_eq!(
        listed.len(),
        started.len(),
        "the listing must report exactly the started tasks, got {listed:?}"
    );
}

/// RULE: the status of a started task is the CLOCK's: it is non-terminal when the task
/// is created and terminal once the completion the record carries has fallen due — and
/// the reads report the same status, because they read the same record.
#[tokio::test]
async fn the_clock_completes_a_started_task() {
    // The destination bucket exists, so the completion this test is about is the one that
    // delivers its output: `crate::synthesis_task_side_effects` is the suite about the
    // object itself, and about the task whose bucket is not there.
    let polly = Polly::with_s3();
    polly
        .s3("us-east-1")
        .create_bucket()
        .bucket(OUTPUT_BUCKET)
        .send()
        .await
        .expect("the destination bucket is created");
    let request = serde_json::json!({
        "Text": "Hello from the emulator.",
        "VoiceId": "Matthew",
        "OutputFormat": "mp3",
        "OutputS3BucketName": OUTPUT_BUCKET,
    });
    let response = start(&polly, "us-east-1", &request).await;
    assert_eq!(
        response.status(),
        200,
        "the task must be created, body {:?}",
        json(&response)
    );
    let body = json(&response);
    let task_id = body["SynthesisTask"]["TaskId"]
        .as_str()
        .expect("a created task reports its id")
        .to_string();
    assert_eq!(
        body["SynthesisTask"]["TaskStatus"].as_str(),
        Some(CREATED_STATUS),
        "a just-started task is not synthesized yet, got {body:?}"
    );

    // Past the instant the record says the completion fell due, both reads report it —
    // and a status filter for it now matches, which is the listing reading the same
    // record.
    tokio::time::sleep(TASK_COMPLETION_DELAY + Duration::from_millis(50)).await;
    let read = get_task(&polly, &task_id).await;
    assert_eq!(
        json(&read)["SynthesisTask"]["TaskStatus"].as_str(),
        Some(TERMINAL_STATUS),
        "case {task_id:?}: the completion the clock owed must be reported, body {:?}",
        json(&read)
    );
    let listed = json(&list(&polly, &format!("?Status={TERMINAL_STATUS}")).await);
    assert_eq!(
        task_ids(&listed),
        vec![task_id.clone()],
        "case {task_id:?}: the listing must report the completed task, got {listed:?}"
    );
}

/// RULE: a request the service refuses records NOTHING — one row per refusal the
/// asynchronous path answers, and the store is empty after all of them.
#[tokio::test]
async fn a_refused_request_records_no_task() {
    let polly = Polly::new();
    // The asynchronous API's own text bounds: "the maximum is 200,000 characters, of
    // which no more than 100,000 can be billed characters"
    // (`TextLengthExceededException`). One past each bound is refused.
    let rows: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "speech marks with an audio format",
            serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "SpeechMarkTypes": ["word"],
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "MarksNotSupportedForFormatException",
        ),
        (
            "ssml speech marks over plain text",
            serde_json::json!({
                "Text": "hello world",
                "TextType": "text",
                "VoiceId": "Joanna",
                "OutputFormat": "json",
                "SpeechMarkTypes": ["ssml"],
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "SsmlMarksNotSupportedForTextTypeException",
        ),
        (
            "one character past the billed bound",
            serde_json::json!({
                "Text": "a".repeat(100_001),
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "TextLengthExceededException",
        ),
        (
            "one character past the total bound",
            serde_json::json!({
                // Markup is not billed, so this text is inside the billed bound and
                // past the total one.
                "Text": format!("<speak>{}</speak>", "a".repeat(199_987)),
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "TextLengthExceededException",
        ),
        (
            "a lexicon this Region does not store",
            serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "LexiconNames": ["Missing"],
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            404,
            "LexiconNotFoundException",
        ),
        (
            "a sample rate the format does not accept",
            serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "pcm",
                "SampleRate": "22050",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "InvalidSampleRateException",
        ),
        (
            "an ssml document that is not well-formed",
            serde_json::json!({
                "Text": "<speak>unclosed",
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "InvalidSsmlException",
        ),
        (
            "an engine the voice does not support",
            serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Aditi",
                "Engine": "neural",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, status, code) in &rows {
        let response = start(&polly, "us-east-1", request).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (*status, (*code).to_string()),
            "case {case:?}: wrong refusal, body {:?}",
            json(&response)
        );
    }

    let listed = task_ids(&json(&list(&polly, "").await));
    assert!(
        listed.is_empty(),
        "no refused request may record a task, got {listed:?}"
    );

    // A request inside every bound still starts a task, so the rows above are not
    // passing on a path that refuses everything.
    let accepted = start(
        &polly,
        "us-east-1",
        &serde_json::json!({
            "Text": "a".repeat(100_000),
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
            "OutputS3BucketName": OUTPUT_BUCKET,
        }),
    )
    .await;
    assert_eq!(
        accepted.status(),
        200,
        "a request at the billed bound must be accepted, body {:?}",
        json(&accepted)
    );
    assert_eq!(
        json(&accepted)["SynthesisTask"]["RequestCharacters"].as_i64(),
        Some(100_000),
        "the billed count must be the text's own, body {:?}",
        json(&accepted)
    );
}

/// How long a row waits for a task to reach a terminal status, and how often it looks.
/// Driven off what has HAPPENED rather than off one fixed wait, so a slow machine delays a
/// row instead of breaking it.
const TERMINAL_WAIT: Duration = Duration::from_secs(30);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// One addressing a caller reaches this emulator at, and the address an announcement made to
/// it must carry.
struct Addressed {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The `Host` the request carries, or none at all — which leaves the request's own
    /// absolute URI, one of this service's published hosts, as the address it named.
    host: Option<&'static str>,
    /// What a forwarding front end states about the address the client dialled IT at, for a
    /// row that stands for a request that reached this process through one.
    forwarded: &'static [(&'static str, &'static str)],
    /// The EMULATOR'S OWN endpoint this request arrived on, which is the authority an
    /// announcement made to such a caller carries; `None` for a caller that resolves AWS's
    /// own hostnames and states no address of this emulator at all, which is announced the
    /// address this process is SERVED at instead.
    reached: Option<&'static str>,
}

impl Addressed {
    /// The base a location announced to this caller is rooted at, for a process serving at
    /// `published`: the endpoint this request arrived on, else the published base. Every
    /// address of this emulator is dialled over the scheme its listeners present.
    fn announced_base(&self, published: &str) -> String {
        match self.reached {
            Some(authority) => format!("https://{authority}"),
            None => published.to_string(),
        }
    }
}

/// The addresses the rows reach the emulator at: the emulator's own endpoint as a loopback
/// literal, as a loopback name on another port and as a bracketed IPv6 literal — each the
/// only address such a client resolves, so each is handed back the very one it is talking to
/// — the two callers that resolve AWS's own hostnames, which are answered the address this
/// process is served at, and a request a front end re-addressed to this process, which
/// carries the address the client dialled in what that front end states about it.
fn addressed() -> [Addressed; 6] {
    [
        Addressed {
            case: "the emulator's own loopback endpoint",
            host: Some("127.0.0.1:39461"),
            forwarded: &[],
            reached: Some("127.0.0.1:39461"),
        },
        Addressed {
            case: "a loopback name on another port",
            host: Some("localhost:9297"),
            forwarded: &[],
            reached: Some("localhost:9297"),
        },
        Addressed {
            case: "a bracketed IPv6 literal",
            host: Some("[::1]:9297"),
            forwarded: &[],
            reached: Some("[::1]:9297"),
        },
        Addressed {
            case: "a caller that resolves this service's own published host",
            host: None,
            forwarded: &[],
            reached: None,
        },
        Addressed {
            case: "a caller that addressed the object store's own published host",
            host: Some("s3.us-east-1.amazonaws.com"),
            forwarded: &[],
            reached: None,
        },
        Addressed {
            case: "a request a front end re-addressed to this process",
            host: Some("127.0.0.1:39461"),
            forwarded: &[("x-forwarded-host", "127.0.0.1:37623")],
            reached: Some("127.0.0.1:37623"),
        },
    ]
}

/// One output location a row asks for: where the object goes and what its key looks like.
struct Output {
    case: &'static str,
    /// The Region the call is signed for — which the announced address does NOT follow.
    region: &'static str,
    /// The bucket the request names, which exists in that Region.
    bucket: &'static str,
    /// `OutputFormat`, whose own suffix the key carries.
    output_format: &'static str,
    /// `OutputS3KeyPrefix`, where the row names one.
    key_prefix: Option<&'static str>,
    /// The suffix the key must end at for that format.
    suffix: &'static str,
}

/// The output locations the rows ask for: the root of a bucket in one Region, and a
/// folder-like key prefix in another, in another format.
fn outputs() -> [Output; 2] {
    [
        Output {
            case: "mp3 at the root of the bucket",
            region: "us-east-1",
            bucket: "polly-announced-output",
            output_format: "mp3",
            key_prefix: None,
            suffix: "mp3",
        },
        Output {
            case: "pcm under a folder-like prefix, in another Region",
            region: "eu-west-1",
            bucket: "polly-announced-output-eu",
            output_format: "pcm",
            key_prefix: Some("out/2026/"),
            suffix: "pcm",
        },
    ]
}

/// One task call signed for `region` and addressed as `addressing` has it: the `Host` the
/// row names, where it names one, and whatever a front end states about the address the
/// client dialled it at.
async fn call_from(
    polly: &Polly,
    region: &str,
    addressing: &Addressed,
    method: &str,
    path: &str,
    body: &str,
) -> http::Response<Bytes> {
    let mut headers = vec![("content-type", "application/json")];
    if let Some(host) = addressing.host {
        headers.push(("host", host));
    }
    headers.extend_from_slice(addressing.forwarded);
    polly
        .call_bytes_in_region(region, method, path, &headers, body.as_bytes().to_vec())
        .await
}

/// The authority an announced URI names: what follows its scheme, up to the path.
fn announced_authority(uri: &str) -> &str {
    let after_scheme = uri.split_once("://").map_or(uri, |(_, rest)| rest);
    after_scheme.split('/').next().unwrap_or_default()
}

/// The object store addressed AT the base a location was announced on: the SDK's endpoint
/// URL is that base itself and the addressing is path-style, so the request this client
/// makes is a `GET` of exactly the URI the task reported.
///
/// Built like the in-process hop a completed task writes its object through
/// (`service_common::s3_client`), with the announced base in place of a Region's endpoint:
/// the transport is this emulator's own, so what a fetch through it establishes is that the
/// address the task handed out is served HERE.
fn store_at(polly: &Polly, base: &str, region: &str) -> ::aws_sdk_s3::Client {
    let callback = polly.callback();
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(base)
        .force_path_style(true)
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Create `bucket` in `region` through the object store itself.
///
/// Outside `us-east-1` a create has to name the Region it is making the bucket in — "If you
/// don't specify a Region, the bucket is created in the US East (N. Virginia) Region
/// (us-east-1) by default" (`CreateBucketConfiguration$LocationConstraint`).
async fn create_bucket(polly: &Polly, region: &str, bucket: &str) {
    let mut create = polly.s3(region).create_bucket().bucket(bucket);
    if region != "us-east-1" {
        create = create.create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        );
    }
    create
        .send()
        .await
        .unwrap_or_else(|error| panic!("the bucket {bucket:?} must be created: {error}"));
}

/// RULE: the output location a task reports is rooted at an address this emulator is
/// REACHED at — the emulator's own endpoint the answering request arrived on (for a request a
/// front end re-addressed to this process, the address that front end states the client
/// dialled it at), else the address this process is SERVED at, which is what a caller that
/// resolves AWS's own hostnames is answered — never at a host AWS publishes, with the object
/// addressed path-style beneath it; every serialization of the task envelope (the start, the
/// polled read and the listing) reports that one location, and following it reaches the
/// object the completed task wrote.
///
/// Driven over the whole addressing table crossed with both output locations, for an emulator
/// SERVED at `served` — the half of the reading a caller stating no address of this emulator
/// is answered, which only varying that base can show. The three tests below are the three
/// forms such a base takes.
async fn the_announced_locations_of_an_emulator_served_at(served: &str) {
    let polly = Polly::with_s3_served_at(served);
    for output in outputs() {
        create_bucket(&polly, output.region, output.bucket).await;
    }

    for addressing in addressed() {
        let base = addressing.announced_base(served);
        for output in outputs() {
            let case = format!("{served} / {} / {}", addressing.case, output.case);
            let mut request = serde_json::json!({
                "Text": "Hello from the emulator.",
                "VoiceId": "Joanna",
                "OutputFormat": output.output_format,
                "OutputS3BucketName": output.bucket,
            });
            if let Some(prefix) = output.key_prefix {
                request["OutputS3KeyPrefix"] = serde_json::Value::from(prefix);
            }
            let started = call_from(
                &polly,
                output.region,
                &addressing,
                "POST",
                "/v1/synthesisTasks",
                &request.to_string(),
            )
            .await;
            assert_eq!(
                started.status(),
                200,
                "case {case:?}: the task must be created, body {:?}",
                json(&started)
            );
            let reported = json(&started)["SynthesisTask"].clone();
            let task_id = reported["TaskId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {reported}"))
                .to_string();

            // The one location this task has: the announced base, then the bucket and the
            // key the request's prefix, the minted id and the format's suffix spell.
            let expected = format!(
                "{}/{}/{}{task_id}.{}",
                base,
                output.bucket,
                output.key_prefix.unwrap_or_default(),
                output.suffix
            );
            assert_eq!(
                reported["OutputUri"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: wrong announced output location, got {reported}"
            );
            // ... and its host is none of the zones AWS publishes its own endpoints in
            // (`service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES`), whatever
            // Region the call was signed for.
            let host = ::service_common::data_plane_addressing::host_only(announced_authority(
                &expected,
            ));
            assert!(
                !AWS_ENDPOINT_DNS_SUFFIXES
                    .iter()
                    .any(|suffix| host.ends_with(suffix)),
                "case {case:?}: an announced location must not name a host AWS publishes, \
                 got {host:?}"
            );

            // The polled read reports that same location on every look, up to the terminal
            // status its completion settles.
            let looks = TERMINAL_WAIT.as_millis() / LOOK_INTERVAL.as_millis();
            let mut terminal = serde_json::Value::Null;
            for _ in 0..looks {
                let read = call_from(
                    &polly,
                    output.region,
                    &addressing,
                    "GET",
                    &format!("/v1/synthesisTasks/{task_id}"),
                    "",
                )
                .await;
                assert_eq!(
                    read.status(),
                    200,
                    "case {case:?}: the started task must read back, body {:?}",
                    json(&read)
                );
                let polled = json(&read)["SynthesisTask"].clone();
                assert_eq!(
                    polled["OutputUri"].as_str(),
                    Some(expected.as_str()),
                    "case {case:?}: the read must report the task's own location, got {polled}"
                );
                if polled["TaskStatus"].as_str() == Some(TERMINAL_STATUS) {
                    terminal = polled;
                    break;
                }
                tokio::time::sleep(LOOK_INTERVAL).await;
            }
            assert_eq!(
                terminal["TaskStatus"].as_str(),
                Some(TERMINAL_STATUS),
                "case {case:?}: the task never completed, last report {terminal}"
            );

            // The listing reports the same location for the same task.
            let listed = call_from(
                &polly,
                output.region,
                &addressing,
                "GET",
                "/v1/synthesisTasks",
                "",
            )
            .await;
            let page = json(&listed);
            let entry = page["SynthesisTasks"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the listing carries no page: {page}"))
                .iter()
                .find(|task| task["TaskId"].as_str() == Some(task_id.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the listing must report {task_id:?}: {page}")
                })
                .clone();
            assert_eq!(
                entry["OutputUri"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: the listing must report the task's own location, got {entry}"
            );

            // Following the announced location reaches the object: the URI is split exactly
            // as it was handed out — its base is the announced endpoint, the bucket and key
            // are its path — and the object store of THIS emulator answers for that bucket
            // and key with the written output. (The fetch rides this emulator's own
            // in-process transport, the very seam the completion wrote the object through, so
            // what it establishes is that the bucket and key the location names hold the
            // output — not that the announced authority is dialable from outside.)
            let path = expected
                .strip_prefix(base.as_str())
                .and_then(|path| path.strip_prefix('/'))
                .unwrap_or_else(|| panic!("case {case:?}: {expected:?} is not rooted at the base"));
            let (bucket, key) = path
                .split_once('/')
                .unwrap_or_else(|| panic!("case {case:?}: {path:?} addresses no object"));
            let object = store_at(&polly, &base, output.region)
                .get_object()
                .bucket(bucket)
                .key(key)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: following {expected:?} must reach the object: {error}")
                });
            let body = object
                .body
                .collect()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: the object's body must be readable: {error}")
                })
                .into_bytes();
            assert!(
                !body.is_empty(),
                "case {case:?}: following {expected:?} must serve the task's output, got {} bytes",
                body.len()
            );
        }
    }
}

/// The whole table for an emulator served on the address the core publishes for a data port
/// bound to the loopback interface: a host literal with the port it bound.
#[tokio::test]
async fn the_output_location_is_announced_on_the_endpoint_the_request_reached() {
    // An arbitrary loopback port, so nothing here depends on which port a process happens to
    // have bound.
    the_announced_locations_of_an_emulator_served_at("https://127.0.0.1:46365").await;
}

/// The same table for an emulator served under the LOOPBACK ZONE rather than at a literal,
/// which is the other spelling of that interface a published base carries.
#[tokio::test]
async fn the_output_location_of_an_emulator_served_by_its_loopback_name() {
    the_announced_locations_of_an_emulator_served_at("https://localhost:9297").await;
}

/// And for an emulator served on NO PORT at all — embedded in another process, or a test —
/// whose published base is the one `smithy_server_runtime::UNSERVED_URL_BASE` names, a host
/// RFC 6761 reserves so that nothing resolves it. A caller that states an address of this
/// emulator is still announced that address; a caller that states none has none to be given.
#[tokio::test]
async fn the_output_location_of_an_emulator_served_on_no_port() {
    the_announced_locations_of_an_emulator_served_at(UNSERVED_URL_BASE).await;
}

/// The member the task-CREATION response carries beside the modelled ones: the SDK
/// feature toggles the service applied to the request.
///
/// Not a modelled member — `smithy_query(query="shape", shape="SynthesisTask")` lists
/// fifteen and none of them is this one, and `smithy_query(query="shape",
/// shape="SdkFeatureToggles")` finds no such shape — so the response Amazon Polly sends
/// is its only authority.
const TOGGLES_MEMBER: &str = "SdkFeatureToggles";

/// The members of `com.amazonaws.polly#SynthesisTask`, from
/// `smithy_query(query="shape", shape="SynthesisTask")`, that a start call resolves a
/// value for on EVERY request.
const ALWAYS_REPORTED_MEMBERS: &[&str] = &[
    "Engine",
    "TaskId",
    "TaskStatus",
    "OutputUri",
    "CreationTime",
    "RequestCharacters",
    "OutputFormat",
    "TextType",
    "VoiceId",
];

/// The modelled members a task reports only where the REQUEST set them, each named by
/// the input member it travels from — the same name on both shapes.
const MEMBERS_FROM_THE_REQUEST: &[&str] = &[
    "SampleRate",
    "SnsTopicArn",
    "LanguageCode",
    "LexiconNames",
    "SpeechMarkTypes",
];

/// Every member of the task object a creation response must carry for `request`: the
/// ones every start call resolves, the optional modelled ones this request set, and the
/// one member beyond the model. `TaskStatusReason` is in neither list: a just-created
/// task has no reason to report.
fn expected_members(request: &serde_json::Value) -> BTreeSet<&'static str> {
    let mut expected: BTreeSet<&'static str> = ALWAYS_REPORTED_MEMBERS.iter().copied().collect();
    for member in MEMBERS_FROM_THE_REQUEST {
        if request.get(member).is_some() {
            expected.insert(member);
        }
    }
    expected.insert(TOGGLES_MEMBER);
    expected
}

/// RULE: the task-creation response reports, BESIDE the task's modelled members, the SDK
/// feature toggles the service applied to the request that started the task — a list
/// member, serialized whether or not anything is in it, so a request that turns no
/// toggle on still reports it as the empty list. The task READS report the modelled
/// members alone.
///
/// The rows are the shared table — which spans the output format, the engine, the text
/// type, the voice and its language, the key prefix, the speech marks and the Region —
/// plus two that set the remaining optional members a task reports (the notification
/// topic, and a lexicon the Region stores), so the member set is checked over a request
/// that names every modelled member as well as over ones that name almost none.
#[tokio::test]
async fn a_created_task_reports_the_feature_toggles_the_service_applied() {
    let polly = Polly::new();
    let mut table: Vec<(String, &'static str, serde_json::Value)> = rows()
        .into_iter()
        .map(|(case, request, expected)| (case.to_string(), expected.region, request))
        .collect();

    let topic = format!(
        "arn:aws:sns:us-east-1:{}:polly-task-updates",
        engine::EMULATOR_ACCOUNT_ID
    );
    table.push((
        "a task that asks for a status notification".to_string(),
        "us-east-1",
        serde_json::json!({
            "Text": "Notify me.",
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
            "SampleRate": "16000",
            "SnsTopicArn": topic,
            "OutputS3BucketName": OUTPUT_BUCKET,
        }),
    ));
    // A task may name only a lexicon the Region it is started in stores, so the lexicon
    // is put there first.
    let stored = polly
        .put_lexicon_in("us-east-1", "TaskLexicon", &pls("ipa", "en-US", 1))
        .await;
    assert_eq!(
        stored.status(),
        200,
        "the lexicon the next row names must be stored, body {:?}",
        json(&stored)
    );
    table.push((
        "a task that names a stored lexicon and a language".to_string(),
        "us-east-1",
        serde_json::json!({
            "Text": "With a lexicon.",
            "VoiceId": "Aditi",
            "LanguageCode": "en-IN",
            "LexiconNames": ["TaskLexicon"],
            "OutputFormat": "mp3",
            "OutputS3BucketName": OUTPUT_BUCKET,
        }),
    ));

    let mut started: Vec<String> = Vec::new();
    for (case, region, request) in &table {
        let response = start(&polly, region, request).await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: the task must be created, body {:?}",
            json(&response)
        );
        let body = json(&response);
        let reported = body["SynthesisTask"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: no SynthesisTask object in {body}"))
            .clone();

        // The toggle list is there, and it is a LIST: empty, because nothing this
        // request names turns a toggle on.
        assert_eq!(
            reported.get(TOGGLES_MEMBER),
            Some(&serde_json::json!([])),
            "case {case:?}: the created task must report an empty {TOGGLES_MEMBER} list, \
             got {:?}",
            reported.get(TOGGLES_MEMBER)
        );
        // ... beside the modelled members this request has values for, and nothing else:
        // the extra member is added to the envelope, it does not displace or multiply
        // anything.
        let carried: BTreeSet<&str> = reported.keys().map(String::as_str).collect();
        let expected = expected_members(request);
        assert_eq!(
            carried,
            expected,
            "case {case:?}: wrong member set, got {carried:?}, expected {expected:?}"
        );

        // The task READS report the modelled members alone — the same values, member for
        // member, apart from the two the CLOCK owns (`TaskStatus` and its reason, which
        // the completion moves on its own schedule).
        let task_id = reported["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {reported:?}"))
            .to_string();
        let read = get_task(&polly, &task_id).await;
        assert_eq!(
            read.status(),
            200,
            "case {case:?}: the started task must read back, body {:?}",
            json(&read)
        );
        let read_body = json(&read);
        let read_task = &read_body["SynthesisTask"];
        assert!(
            read_task.get(TOGGLES_MEMBER).is_none(),
            "case {case:?}: a task read must report no {TOGGLES_MEMBER}, got {:?}",
            read_task.get(TOGGLES_MEMBER)
        );
        for member in ALWAYS_REPORTED_MEMBERS
            .iter()
            .chain(MEMBERS_FROM_THE_REQUEST)
            .filter(|member| **member != "TaskStatus")
        {
            assert_eq!(
                read_task.get(*member),
                reported.get(*member),
                "case {case:?}: member {member} differs between the creation response and \
                 the read, got {:?} and {:?}",
                read_task.get(*member),
                reported.get(*member)
            );
        }
        started.push(task_id);
    }

    // The listing is a task read too, so it reports the modelled members alone as well.
    let page = json(&list(&polly, "").await);
    let listed = page["SynthesisTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("the listing carries no page: {page}"))
        .clone();
    for task_id in &started {
        let entry = listed
            .iter()
            .find(|task| task["TaskId"].as_str() == Some(task_id.as_str()))
            .unwrap_or_else(|| panic!("the listing must report {task_id:?}: {page}"));
        assert!(
            entry.get(TOGGLES_MEMBER).is_none(),
            "task {task_id:?}: a listed task must report no {TOGGLES_MEMBER}, got {entry}"
        );
    }
}

/// The `OutputFormat` values the model enumerates, from `smithy_query(query="shape",
/// shape="OutputFormat")`: `alaw | json | mp3 | mulaw | ogg_opus | ogg_vorbis | pcm`,
/// each with the suffix its output object carries. These are the three the shared table
/// above does not drive — and none of them is the format the recorded case named.
const REMAINING_OUTPUT_FORMATS: &[(&str, &str)] = &[
    ("ogg_vorbis", "ogg"),
    ("alaw", "alaw"),
    ("mulaw", "mulaw"),
];

/// One row per remaining output format, each also differing in the Region the call is
/// signed for, the voice, the engine, the text type and the key prefix — so what the
/// rows share is only that a task is being started, not the shape of one request.
fn rows_over_the_remaining_formats() -> Vec<(String, &'static str, &'static str, serde_json::Value)>
{
    // Region, voice, engine (None leaves the request's own default), text type and key
    // prefix, zipped with the formats above.
    let shapes: &[(&str, &str, Option<&str>, &str, &str)] = &[
        ("us-east-1", "Matthew", None, "text", "vorbis/"),
        ("eu-west-1", "Joanna", Some("neural"), "text", ""),
        ("us-west-2", "Aditi", None, "ssml", "telephony-"),
    ];
    REMAINING_OUTPUT_FORMATS
        .iter()
        .zip(shapes)
        .map(|((format, suffix), (region, voice, engine, text_type, key_prefix))| {
            let mut request = serde_json::json!({
                "Text": match *text_type {
                    "ssml" => "<speak>Hello there.</speak>",
                    _ => "Hello from the emulator.",
                },
                "VoiceId": voice,
                "OutputFormat": format,
                "OutputS3BucketName": OUTPUT_BUCKET,
            });
            if *text_type == "ssml" {
                request["TextType"] = serde_json::json!(text_type);
            }
            if let Some(engine) = engine {
                request["Engine"] = serde_json::json!(engine);
            }
            if !key_prefix.is_empty() {
                request["OutputS3KeyPrefix"] = serde_json::json!(key_prefix);
            }
            (
                format!("a {format} task started in {region} as {text_type}"),
                *region,
                *suffix,
                request,
            )
        })
        .collect()
}

/// RULE, over the rest of the format catalog and on the RAW WIRE: the task-creation
/// response serializes the SDK feature toggle list unconditionally — a list member,
/// empty when the request turns no toggle on — whatever the request asks for, and it is
/// in the BYTES the service answers with rather than something a model-driven decoder
/// supplies, which is what a client comparing the raw body against an AWS capture reads.
/// Nothing else about the envelope moves: the modelled member set is exactly the one the
/// request has values for, the output location still names the requested format's own
/// suffix, and the task read still reports the modelled members alone.
#[tokio::test]
async fn the_wire_body_of_a_created_task_carries_the_toggle_list_for_every_output_format() {
    let polly = Polly::new();
    for (case, region, suffix, request) in rows_over_the_remaining_formats() {
        let response = start(&polly, region, &request).await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: the task must be created, body {:?}",
            json(&response)
        );

        // On the wire: the member's NAME is in the bytes the service wrote.
        let raw = String::from_utf8_lossy(response.body()).to_string();
        assert!(
            raw.contains(&format!("\"{TOGGLES_MEMBER}\"")),
            "case {case:?}: the creation body must carry {TOGGLES_MEMBER} on the wire, \
             got {raw:?}"
        );

        let body = json(&response);
        let reported = body["SynthesisTask"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: no SynthesisTask object in {body}"))
            .clone();
        assert_eq!(
            reported.get(TOGGLES_MEMBER),
            Some(&serde_json::json!([])),
            "case {case:?}: the created task must report an empty {TOGGLES_MEMBER} list, \
             got {:?}",
            reported.get(TOGGLES_MEMBER)
        );
        let carried: BTreeSet<&str> = reported.keys().map(String::as_str).collect();
        let expected = expected_members(&request);
        assert_eq!(
            carried, expected,
            "case {case:?}: wrong member set, got {carried:?}, expected {expected:?}"
        );

        // The modelled members the extra one sits beside are unaffected: the location
        // still addresses the object of the requested format under the requested prefix.
        let task_id = reported["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {reported:?}"))
            .to_string();
        let announced = reported["OutputUri"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no OutputUri in {reported:?}"))
            .to_string();
        let key_prefix = request["OutputS3KeyPrefix"].as_str().unwrap_or_default();
        let tail = format!("/{OUTPUT_BUCKET}/{key_prefix}{task_id}.{suffix}");
        assert!(
            announced.ends_with(&tail),
            "case {case:?}: the location must address {tail:?}, got {announced:?}"
        );

        // The task READ reports the modelled members alone.
        let read = get_task(&polly, &task_id).await;
        assert_eq!(
            read.status(),
            200,
            "case {case:?}: the started task must read back, body {:?}",
            json(&read)
        );
        let read_body = json(&read);
        let read_task = &read_body["SynthesisTask"];
        assert!(
            read_task.get(TOGGLES_MEMBER).is_none(),
            "case {case:?}: a task read must report no {TOGGLES_MEMBER}, got {:?}",
            read_task.get(TOGGLES_MEMBER)
        );
        assert_eq!(
            read_task["OutputUri"].as_str(),
            Some(announced.as_str()),
            "case {case:?}: the read must report the location the creation announced, \
             got {:?}",
            read_task["OutputUri"].as_str()
        );
    }
}
