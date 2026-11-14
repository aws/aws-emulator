// SPDX-License-Identifier: CC0-1.0

//! The media a job start is pointed at, resolved against the emulator's own Amazon S3,
//! with that service mounted beside transcribe on one engine — exactly the way the
//! assembled emulator runs them, and the way the real service takes a job's audio on
//! when the job is started.
//!
//! THE RULE UNDER TEST: a job start names its input audio as an object in Amazon S3
//! ("you must first upload your media file into an Amazon S3 bucket; you can then
//! specify the Amazon S3 location of the file using the Media parameter"), so that
//! location is resolved against the object store before anything is stored: a start
//! whose media the store reports it does not hold is refused with the operation's
//! modelled client error (`BadRequestException`, HTTP 400) and NOTHING is persisted, so
//! no later read or listing can report the job and the name is still free. A start
//! whose media the store does answer for is accepted, and is then judged by WHAT that
//! media is: a declared `MediaFormat` the stored object contradicts ends the job in the
//! service's own modelled `FAILED` state with the `FailureReason` the model enumerates
//! for it, while a job the service found nothing wrong with completes.
//!
//! Both halves are asserted over a TABLE whose rows differ along what the rule
//! generalises over: which start operation of the service names the media (all four of
//! them), which member names it (`MediaFileUri`, and the `RedactedMediaFileUri` Call
//! Analytics accepts instead of it), which part of the address does not resolve (the
//! bucket, the key, a key that only prefixes or only extends a stored one), how the key
//! is shaped (a bare name, a nested prefix), and which declared format the stored object
//! agrees or disagrees with.
//!
//! The last test pins the other half of the rule: a refusal has to be GROUNDED in what
//! the object store answered. With no Amazon S3 mounted to ask, nothing is known about
//! the media, so nothing is refused and no job is failed — which is why every other
//! suite in this crate may start jobs whose buckets no S3 holds.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::transcription_jobs::{dispatch, Reply};

/// The Region both services are addressed in. Amazon S3 places a bucket created with no
/// `LocationConstraint` in us-east-1 ("If you don't specify a Region, the bucket is
/// created in the US East (N. Virginia) Region (us-east-1) by default"), which is also
/// the Region transcribe serves a request carrying no credential scope in, so one Region
/// label serves every call here.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the
/// object store. No authenticator is registered, so the signature is never checked; the
/// scope is what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The bucket every staged media object below lives in.
pub(crate) const BUCKET: &str = "example-media";

/// A bucket name no `CreateBucket` below ever creates.
pub(crate) const ABSENT_BUCKET: &str = "example-media-missing";

/// The keys staged in [`BUCKET`]: one at the bucket root and one under a nested prefix,
/// each named with one of the modelled `MediaFormat` extensions
/// (`smithy_query(query="shape", shape="MediaFormat")`: amr | flac | m4a | mp3 | mp4 |
/// ogg | wav | webm), which is what the format of a stored object is read from.
pub(crate) const STAGED_KEYS: &[&str] = &["dictation.wav", "clinic/visits/consult.flac"];

/// The bytes every staged object holds. Nothing reads a media file's content — no audio
/// is transcribed — so one short payload serves every row.
///
/// `pub(crate)` because the shared harness stages media through it too
/// ([`crate::transcription_jobs::Transcribe::given_media_objects`]).
pub(crate) const MEDIA_BYTES: &[u8] = b"RIFF....WAVEfmt emulator media object";

/// Long enough for the `IN_PROGRESS -> COMPLETED` (or `-> FAILED`) transition of a job
/// started during the test to have fallen due: one span of
/// `crate::state::JOB_TRANSITION_DELAY` (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
pub(crate) const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The `FailureReason` a job whose declared `MediaFormat` the stored media contradicts
/// reports, verbatim as the model's own enumeration of that member spells it
/// (`smithy_query(query="doc", shape="MedicalTranscriptionJob", member="FailureReason")`).
const FORMAT_MISMATCH: &str = "The media format provided does not match the detected media format";

/// The role a Medical Scribe start names (`DataAccessRoleArn`, a required member of that
/// operation).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthScribeAccess";

/// One emulator: transcribe and Amazon S3 on a single shared engine behind the
/// production router — what `build_router` assembles, narrowed to the services under
/// test.
pub(crate) fn emulator() -> Arc<MultiServiceRouter> {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_transcribe_transcribe::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    Arc::new(router)
}

/// An emulator with transcribe alone: no object store to ask about any media.
pub(crate) fn emulator_without_the_object_store() -> Arc<MultiServiceRouter> {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_transcribe_transcribe::EmulatedService::new(engine),
    );
    Arc::new(router)
}

/// One transcribe call in the service's own wire envelope (awsJson1_1), signed for
/// [`REGION`].
pub(crate) async fn call(emulator: &MultiServiceRouter, operation: &str, input: Value) -> Reply {
    dispatch(emulator, operation, input, Some(REGION)).await
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes, so this is a raw request
/// rather than a modelled call. The credential scope names `s3`, which is what the
/// router claims on.
///
/// `pub(crate)`, like the harness around it: the job-output suite
/// (`crate::job_output_objects`) fetches the locations a finished job reports through
/// this very helper rather than spelling the raw request a second time.
pub(crate) async fn s3(
    emulator: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, bytes::Bytes) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    (response.status().as_u16(), response.body().clone())
}

/// `PUT /<bucket>` — `CreateBucket` — then `PUT /<bucket>/<key>` — `PutObject`, so the
/// object store really holds `bytes` at that address — the upload a caller performs
/// before naming the location in a request.
///
/// The ONE staging path for [`REGION`]: [`given_staged_media`] stages this suite's media
/// through it, and the shared harness stages whatever a suite of its own names through
/// the same function ([`crate::transcription_jobs::Transcribe::given_object`]), so two
/// suites cannot disagree about what staging an object means. The language-model suite
/// keeps its own pair (`crate::language_model_training_data::create_bucket` and `stage`)
/// because it signs for another Region and creates its bucket with that Region's
/// `LocationConstraint`.
pub(crate) async fn given_object(
    emulator: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    bytes: &[u8],
) {
    let (status, body) = s3(emulator, "PUT", &format!("/{bucket}"), &[]).await;
    assert_eq!(
        status,
        200,
        "the bucket {bucket} must be created first, got HTTP {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
    let (status, body) = s3(emulator, "PUT", &format!("/{bucket}/{key}"), bytes).await;
    assert_eq!(
        status,
        200,
        "the object {bucket}/{key} must be stored, got HTTP {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `PUT /<bucket>` — `CreateBucket` — then `PUT /<bucket>/<key>` — `PutObject` — for
/// every key of [`STAGED_KEYS`], so the object store really holds the media the
/// accepted rows below name.
pub(crate) async fn given_staged_media(emulator: &MultiServiceRouter) {
    for key in STAGED_KEYS {
        given_object(emulator, BUCKET, key, MEDIA_BYTES).await;
    }
}

/// The same staging, for an object that DECLARES the natural language of its content:
/// `PutObject` with `Content-Language`, which [`given_object`] does not name.
///
/// The one thing the shared staging path cannot express, and the suite over the
/// language a job identifies (`crate::transcription_job_identified_language`) needs it:
/// a stored media object's own declaration of its language is the first evidence
/// identification reads.
pub(crate) async fn given_object_declaring_language(
    emulator: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    bytes: &[u8],
    content_language: &str,
) {
    given_object(emulator, bucket, key, bytes).await;
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("https://s3.{REGION}.amazonaws.com/{bucket}/{key}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .header("content-language", content_language)
        .body(bytes::Bytes::copy_from_slice(bytes))
        .expect("the request builds");
    let status = emulator.dispatch(request).await.status().as_u16();
    assert_eq!(
        status, 200,
        "the object {bucket}/{key} must be stored declaring {content_language:?}, got HTTP \
         {status}"
    );
}

/// Which family a row drives, which is what decides the start request's own members and
/// the operation that reads the job back.
///
/// `pub(crate)`, like the harness around it: the suite over the format a finished job
/// determined (`crate::transcription_job_media_format`) drives these same families
/// through the same start requests rather than spelling a second copy of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    /// `StartTranscriptionJob` -> `GetTranscriptionJob`.
    Transcription,
    /// `StartMedicalTranscriptionJob` -> `GetMedicalTranscriptionJob`.
    Medical,
    /// `StartCallAnalyticsJob` -> `GetCallAnalyticsJob`.
    CallAnalytics,
    /// `StartMedicalScribeJob` -> `GetMedicalScribeJob`.
    Scribe,
}

impl Family {
    /// The operation that starts a job of this family.
    pub(crate) fn start(self) -> &'static str {
        match self {
            Self::Transcription => "StartTranscriptionJob",
            Self::Medical => "StartMedicalTranscriptionJob",
            Self::CallAnalytics => "StartCallAnalyticsJob",
            Self::Scribe => "StartMedicalScribeJob",
        }
    }

    /// The operation that reads one job of this family back.
    pub(crate) fn get(self) -> &'static str {
        match self {
            Self::Transcription => "GetTranscriptionJob",
            Self::Medical => "GetMedicalTranscriptionJob",
            Self::CallAnalytics => "GetCallAnalyticsJob",
            Self::Scribe => "GetMedicalScribeJob",
        }
    }

    /// The member both the start request and the read name the job by.
    pub(crate) fn name_member(self) -> &'static str {
        match self {
            Self::Transcription => "TranscriptionJobName",
            Self::Medical => "MedicalTranscriptionJobName",
            Self::CallAnalytics => "CallAnalyticsJobName",
            Self::Scribe => "MedicalScribeJobName",
        }
    }

    /// The member of a successful read that carries the job.
    pub(crate) fn job_member(self) -> &'static str {
        match self {
            Self::Transcription => "TranscriptionJob",
            Self::Medical => "MedicalTranscriptionJob",
            Self::CallAnalytics => "CallAnalyticsJob",
            Self::Scribe => "MedicalScribeJob",
        }
    }

    /// The smallest valid start request of this family naming `name` and `media`.
    pub(crate) fn start_request(self, name: &str, media: Value) -> Value {
        match self {
            Self::Transcription => json!({
                "TranscriptionJobName": name,
                "LanguageCode": "en-US",
                "Media": media
            }),
            Self::Medical => json!({
                "MedicalTranscriptionJobName": name,
                "LanguageCode": "en-US",
                "Media": media,
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
            Self::CallAnalytics => json!({
                "CallAnalyticsJobName": name,
                "Media": media
            }),
            Self::Scribe => json!({
                "MedicalScribeJobName": name,
                "Media": media,
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
            }),
        }
    }
}

/// One row of the media table: which family starts the job, what its `Media` says, and
/// what the row is named in every assertion.
struct MediaCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    family: Family,
    name: &'static str,
    /// The `Media` the start request carries.
    media: fn() -> Value,
}

/// Rows whose media the object store reports it does not hold: the start is refused and
/// nothing is stored. Every start operation of the service appears, and so does each
/// part of the address that can fail to resolve.
fn unresolvable() -> Vec<MediaCase> {
    vec![
        MediaCase {
            case: "a transcription job whose bucket the store does not hold",
            family: Family::Transcription,
            name: "missing-bucket",
            media: || json!({"MediaFileUri": format!("s3://{ABSENT_BUCKET}/dictation.wav")}),
        },
        MediaCase {
            case: "a medical transcription job whose key nothing was ever written to",
            family: Family::Medical,
            name: "missing-key",
            media: || json!({"MediaFileUri": format!("s3://{BUCKET}/never-uploaded.flac")}),
        },
        MediaCase {
            case: "a Call Analytics job whose key only prefixes a stored one",
            family: Family::CallAnalytics,
            name: "prefixes-stored-key",
            media: || json!({"MediaFileUri": format!("s3://{BUCKET}/clinic/visits")}),
        },
        MediaCase {
            case: "a Medical Scribe job whose key only extends a stored one",
            family: Family::Scribe,
            name: "extends-stored-key",
            media: || json!({"MediaFileUri": format!("s3://{BUCKET}/dictation.wav.bak")}),
        },
        MediaCase {
            case: "a Call Analytics job naming its unresolvable input audio in \
                   RedactedMediaFileUri",
            family: Family::CallAnalytics,
            name: "redacted-missing-key",
            media: || {
                json!({"RedactedMediaFileUri": format!("s3://{BUCKET}/never-uploaded-redacted.wav")})
            },
        },
    ]
}

/// Rows whose media the object store answers for: the start is accepted and the job
/// reads back. The key shapes span a bare name at the bucket root and a nested prefix,
/// and the Call Analytics row names its input audio in the other member.
fn resolvable() -> Vec<MediaCase> {
    vec![
        MediaCase {
            case: "a transcription job over an object at the bucket root",
            family: Family::Transcription,
            name: "root-object",
            media: || json!({"MediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[0])}),
        },
        MediaCase {
            case: "a medical transcription job over an object under a nested prefix",
            family: Family::Medical,
            name: "nested-object",
            media: || json!({"MediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[1])}),
        },
        MediaCase {
            case: "a Call Analytics job naming its input audio in RedactedMediaFileUri",
            family: Family::CallAnalytics,
            name: "redacted-root-object",
            media: || {
                json!({"RedactedMediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[0])})
            },
        },
        MediaCase {
            case: "a Medical Scribe job over an object under a nested prefix",
            family: Family::Scribe,
            name: "scribe-nested-object",
            media: || json!({"MediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[1])}),
        },
        MediaCase {
            case: "a transcription job whose media URI addresses a host rather than an \
                   S3 object, so there is nothing to ask the store about",
            family: Family::Transcription,
            name: "https-media",
            media: || json!({"MediaFileUri": "https://example.invalid/media/clip.mp4"}),
        },
    ]
}

/// Read one job of `family` back by name.
async fn read_job(emulator: &MultiServiceRouter, family: Family, name: &str) -> Reply {
    call(
        emulator,
        family.get(),
        json!({family.name_member(): name}),
    )
    .await
}

#[tokio::test]
async fn a_start_whose_media_the_object_store_does_not_hold_is_refused_and_stores_nothing() {
    let emulator = emulator();
    given_staged_media(&emulator).await;

    for case in unresolvable() {
        let request = case.family.start_request(case.name, (case.media)());
        let answered = call(&emulator, case.family.start(), request).await;
        answered.refused(case.case, 400, "BadRequestException");
        let message = answered.error_message(case.case);
        assert_eq!(
            message,
            "The URI that you provided doesn't point to an S3 object. Make sure that the \
             object exists and try your request again.",
            "case {:?}: expected the service's wording for a media URI that names no \
             object, got {message:?}",
            case.case
        );

        // Nothing was stored: the read of that name reports no such job, so no listing,
        // no delete and no uniqueness check can see one either.
        let read = read_job(&emulator, case.family, case.name).await;
        read.refused(case.case, 400, "BadRequestException");
        assert!(
            read.body[case.family.job_member()].is_null(),
            "case {:?}: a refused start must leave no job to read, got {:?}",
            case.case,
            read.body
        );
    }

    // No refused start left a medical transcription job behind, so the listing of that
    // family is empty — the family the refusal was filed against.
    let listed = call(
        &emulator,
        "ListMedicalTranscriptionJobs",
        json!({"MaxResults": 100}),
    )
    .await;
    let summaries = listed.ok("ListMedicalTranscriptionJobs after the refusals")
        ["MedicalTranscriptionJobSummaries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    assert_eq!(
        summaries, 0,
        "a refused start must reach no listing, got {:?}",
        listed.body["MedicalTranscriptionJobSummaries"]
    );

    // The name a refused start used is still free: a retry that points at media the
    // store holds is accepted under it rather than colliding with a phantom job.
    for case in unresolvable() {
        let request = case.family.start_request(
            case.name,
            json!({"MediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[0])}),
        );
        let retried = call(&emulator, case.family.start(), request).await;
        retried.ok_status(case.case, 200);
    }
}

#[tokio::test]
async fn a_start_whose_media_the_object_store_answers_for_is_accepted_and_reads_back() {
    let emulator = emulator();
    given_staged_media(&emulator).await;

    for case in resolvable() {
        let media = (case.media)();
        let request = case.family.start_request(case.name, media.clone());
        let answered = call(&emulator, case.family.start(), request).await;
        let started = answered.ok(case.case);
        assert_eq!(
            started[case.family.job_member()]["Media"],
            media,
            "case {:?}: the started job must carry the media it was given, got {:?}",
            case.case,
            started[case.family.job_member()]
        );

        // Round trip: the stored job reads back by name, with that same media.
        let read = read_job(&emulator, case.family, case.name).await;
        let job = &read.ok(case.case)[case.family.job_member()];
        assert_eq!(
            job["Media"], media,
            "case {:?}: the stored job must read back with its media, got {job:?}",
            case.case
        );
    }

    // The clock finishes with a job whose media nothing was found wrong with: it
    // completes rather than failing.
    std::thread::sleep(PAST_THE_TRANSITION);
    for case in resolvable() {
        let read = read_job(&emulator, case.family, case.name).await;
        let job = &read.ok(case.case)[case.family.job_member()];
        let status = job["TranscriptionJobStatus"]
            .as_str()
            .or_else(|| job["CallAnalyticsJobStatus"].as_str())
            .or_else(|| job["MedicalScribeJobStatus"].as_str())
            .unwrap_or_default();
        assert_eq!(
            status, "COMPLETED",
            "case {:?}: a job whose media resolved must complete, got {job:?}",
            case.case
        );
        assert!(
            job["FailureReason"].is_null(),
            "case {:?}: a completed job reports no FailureReason, got {job:?}",
            case.case
        );
    }
}

/// One row of the format table: the key the media is staged under, the `MediaFormat` the
/// request declares, and the status the job must end in.
struct FormatCase {
    case: &'static str,
    family: Family,
    name: &'static str,
    key: &'static str,
    /// The declared `MediaFormat`, or `None` for a request that declares none.
    declared: Option<&'static str>,
    /// `TranscriptionJobStatus` the job must settle in.
    settles: &'static str,
}

/// The format rows: the stored object's own name evidences its format (the modelled
/// `MediaFormat` extensions), so a declared format either agrees with it, contradicts it
/// — which is the one per-media failure this emulator can establish — or is not declared
/// at all, leaving nothing to contradict. Both job families whose start carries a
/// `MediaFormat` member appear.
const FORMAT_CASES: &[FormatCase] = &[
    FormatCase {
        case: "a transcription job declaring the format its stored media really is",
        family: Family::Transcription,
        name: "format-agrees",
        key: "dictation.wav",
        declared: Some("wav"),
        settles: "COMPLETED",
    },
    FormatCase {
        case: "a transcription job declaring a format its stored media contradicts",
        family: Family::Transcription,
        name: "format-contradicted",
        key: "dictation.wav",
        declared: Some("mp3"),
        settles: "FAILED",
    },
    FormatCase {
        case: "a medical transcription job declaring a format its stored media \
               contradicts, under a nested prefix",
        family: Family::Medical,
        name: "medical-format-contradicted",
        key: "clinic/visits/consult.flac",
        declared: Some("ogg"),
        settles: "FAILED",
    },
    FormatCase {
        case: "a medical transcription job declaring the format its stored media really \
               is",
        family: Family::Medical,
        name: "medical-format-agrees",
        key: "clinic/visits/consult.flac",
        declared: Some("flac"),
        settles: "COMPLETED",
    },
    FormatCase {
        case: "a medical transcription job declaring no format at all, so there is \
               nothing for the stored media to contradict",
        family: Family::Medical,
        name: "medical-format-undeclared",
        key: "dictation.wav",
        declared: None,
        settles: "COMPLETED",
    },
];

#[tokio::test]
async fn a_job_whose_stored_media_contradicts_the_declared_format_fails_with_the_modelled_reason() {
    let emulator = emulator();
    given_staged_media(&emulator).await;

    for case in FORMAT_CASES {
        let mut request = case.family.start_request(
            case.name,
            json!({"MediaFileUri": format!("s3://{BUCKET}/{}", case.key)}),
        );
        if let Some(declared) = case.declared {
            request["MediaFormat"] = json!(declared);
        }
        let answered = call(&emulator, case.family.start(), request).await;
        let started = &answered.ok(case.case)[case.family.job_member()];
        // Processing is what fails, so every accepted start reports the job as being
        // worked on and names no failure yet.
        assert_eq!(
            started["TranscriptionJobStatus"], "IN_PROGRESS",
            "case {:?}: an accepted start reports the job as in progress, got {started:?}",
            case.case
        );
        assert!(
            started["FailureReason"].is_null(),
            "case {:?}: a start names no FailureReason, got {started:?}",
            case.case
        );
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    for case in FORMAT_CASES {
        let read = read_job(&emulator, case.family, case.name).await;
        let job = &read.ok(case.case)[case.family.job_member()];
        assert_eq!(
            job["TranscriptionJobStatus"], case.settles,
            "case {:?}: expected the job to settle {}, got {job:?}",
            case.case, case.settles
        );
        if case.settles == "FAILED" {
            assert_eq!(
                job["FailureReason"], FORMAT_MISMATCH,
                "case {:?}: a failed job must report the modelled FailureReason, got \
                 {job:?}",
                case.case
            );
            assert!(
                job["Transcript"].is_null(),
                "case {:?}: a failed job names no transcript, got {job:?}",
                case.case
            );
        } else {
            assert!(
                job["FailureReason"].is_null(),
                "case {:?}: a completed job reports no FailureReason, got {job:?}",
                case.case
            );
        }
    }

    // The failure half of each family's lifecycle is reachable through its listing too:
    // a `Status` filter of FAILED reports exactly the jobs that failed.
    for (family, list, summaries, status_member) in [
        (
            Family::Medical,
            "ListMedicalTranscriptionJobs",
            "MedicalTranscriptionJobSummaries",
            "TranscriptionJobStatus",
        ),
        (
            Family::Transcription,
            "ListTranscriptionJobs",
            "TranscriptionJobSummaries",
            "TranscriptionJobStatus",
        ),
    ] {
        let listed = call(&emulator, list, json!({"Status": "FAILED", "MaxResults": 100})).await;
        let body = listed.ok(list);
        let rows = body[summaries]
            .as_array()
            .unwrap_or_else(|| panic!("{list} must report a {summaries} list, got {body}"))
            .clone();
        let expected: Vec<&str> = FORMAT_CASES
            .iter()
            .filter(|case| case.family == family && case.settles == "FAILED")
            .map(|case| case.name)
            .collect();
        let listed_names: Vec<String> = rows
            .iter()
            .map(|row| {
                row[family.name_member()]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed_names, expected,
            "case {list:?}: a FAILED listing must report exactly the failed jobs, got \
             {rows:?}"
        );
        for row in &rows {
            assert_eq!(
                row[status_member], "FAILED",
                "case {list:?}: every row of a FAILED listing is FAILED, got {row:?}"
            );
            assert_eq!(
                row["FailureReason"], FORMAT_MISMATCH,
                "case {list:?}: every failed row reports why, got {row:?}"
            );
        }
    }
}

#[tokio::test]
async fn with_no_object_store_to_ask_no_start_is_refused_and_no_job_is_failed() {
    // A refusal has to be grounded in what the object store answered. Here there is no
    // Amazon S3 mounted at all, so nothing is known about the media and the start is
    // accepted — the emulator never refuses, or fails a job, on an assumption.
    let emulator = emulator_without_the_object_store();

    for case in unresolvable() {
        let request = case.family.start_request(case.name, (case.media)());
        let answered = call(&emulator, case.family.start(), request).await;
        answered.ok_status(case.case, 200);

        // Accepted means STORED, not merely un-refused: the job reads back by name.
        let read = read_job(&emulator, case.family, case.name).await;
        let job = &read.ok(case.case)[case.family.job_member()];
        assert_eq!(
            job[case.family.name_member()], case.name,
            "case {:?}: the accepted job must read back under its name, got {job:?}",
            case.case
        );
    }

    // And a declared format is not contradicted by media nothing was asked about: the
    // job completes.
    for case in FORMAT_CASES {
        let mut request = case.family.start_request(
            case.name,
            json!({"MediaFileUri": format!("s3://{BUCKET}/{}", case.key)}),
        );
        if let Some(declared) = case.declared {
            request["MediaFormat"] = json!(declared);
        }
        call(&emulator, case.family.start(), request)
            .await
            .ok_status(case.case, 200);
    }
    std::thread::sleep(PAST_THE_TRANSITION);
    for case in FORMAT_CASES {
        let read = read_job(&emulator, case.family, case.name).await;
        let job = &read.ok(case.case)[case.family.job_member()];
        assert_eq!(
            job["TranscriptionJobStatus"], "COMPLETED",
            "case {:?}: with no store to ask, a job must complete, got {job:?}",
            case.case
        );
        assert!(
            job["FailureReason"].is_null(),
            "case {:?}: with no store to ask, no job reports a FailureReason, got {job:?}",
            case.case
        );
    }
}
