// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of starting a STORED-VIDEO face detection over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: RekognitionService.StartFaceDetection`),
//! dispatched through the same [`MultiServiceRouter`] the production emulator mounts, with
//! this service and the emulator's own Amazon S3 on ONE engine — the video a start operation
//! analyses is always an object of the object store
//! (`smithy_query(query="shape",shape="Video")` models `S3Object` as its only member).
//!
//! THE RULE UNDER TEST: a `Start…` operation of this API records a stored-video analysis
//! JOB. It establishes that the object store serves the video the request names, mints a
//! `JobId` for the job, persists the request under it and answers with that identifier; a
//! `ClientRequestToken` makes the start idempotent, so the same token with the same request
//! answers with the SAME job and the same token with a different request is refused; and a
//! video it cannot reach is refused with the modelled error for the reason — an object the
//! store does not serve is an unreadable S3 object, a request that identifies no object at
//! all is an invalid parameter.
//!
//! Each test asserts that rule over a TABLE whose rows differ along what the rule
//! generalises over: which object the video names (bucket, key, prefix depth, extension,
//! version), which optional members the start carries and at what values
//! (`FaceAttributes`, `JobTag`, `NotificationChannel`), which member a reused token differs
//! in, and how an unreachable video is unreachable. The recorded case the issue was filed
//! from — a start on an S3 object nothing was staged at — is one row of the refusal table.
//!
//! Nothing of the media is ever read, so the staged objects are opaque bytes: a start
//! establishes that the object is SERVED and reports nothing about what is in it.
//!
//! The last test is about the one thing the clock owes a job and no wire read reports yet
//! (`GetFaceDetection` is a scaffold stub of its own, because reporting the FACES an
//! analysis found needs a video decoder this process does not have): it drives the job
//! record's own status rule at instants it is handed, so it is decided by arithmetic and
//! never by how fast the host runs.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::state::video_jobs::{
    JobStatus, VideoJob, VideoJobs, COMPLETION_SPAN, MAX_CONCURRENT_JOBS,
};
use service_com_amazonaws_rekognition_rekognitionservice::state::video_source::VideoLocation;
use smithy_server_runtime::MultiServiceRouter;

// The router harness with the object store mounted, the dispatch helpers and the reduced
// reply all come from the suites that already own them rather than being built again here.
use crate::face_collections::{call, Reply};
use crate::face_detection::{harness_with_object_store, s3};

/// The bucket the staged videos live in, and one the store never holds.
const BUCKET: &str = "rekognition-videos";
const ABSENT_BUCKET: &str = "rekognition-videos-missing";

/// The keys this suite stages a video at, one per container format `Video` documents as
/// supported ("the supported file formats are .mp4, .mov and .avi",
/// `smithy_query(query="shape",shape="Video")`), with one under a key prefix because a key
/// is a key whatever it looks like.
const STAGED_KEYS: &[&str] = &[
    "clip.mp4",
    "clip.mov",
    "clip.avi",
    "recordings/2026/01/clip.mp4",
];

/// The wording AWS answers an object it could not read the metadata of with. Fixed text for
/// the class: no part of it comes from the request or from this emulator's state.
const UNREADABLE_OBJECT_MESSAGE: &str =
    "Unable to get object metadata from S3. Check object key, region and/or access permissions.";

/// The wording AWS answers an `InvalidParameterException` with. Fixed text for the class.
const INVALID_PARAMETERS_MESSAGE: &str = "Request has invalid parameters";

/// The wording a reused `ClientRequestToken` that names a different request is refused with.
const IDEMPOTENT_MISMATCH_MESSAGE: &str = "Service Unavailable";

/// A notification channel whose two required members satisfy the patterns the model puts on
/// them (`smithy_query(query="shape",shape="NotificationChannel")`).
fn channel(topic: &str) -> Value {
    json!({
        "SNSTopicArn": format!("arn:aws:sns:us-east-1:{}:{topic}", engine::emulator_account_id()),
        "RoleArn": format!(
            "arn:aws:iam::{}:role/AmazonRekognitionVideoRole",
            engine::emulator_account_id()
        ),
    })
}

/// The emulator with the videos of [`STAGED_KEYS`] staged in [`BUCKET`].
///
/// The bytes are opaque: nothing in this process decodes video, so what a start operation
/// can establish about them is that the store serves the object.
async fn seeded() -> MultiServiceRouter {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the video bucket must be created first"
    );
    for key in STAGED_KEYS {
        assert_eq!(
            s3(&router, &format!("/{BUCKET}/{key}"), b"stored-video-object").await,
            200,
            "the video object {key:?} must be stored"
        );
    }
    router
}

/// `Video` naming the staged object `key`.
fn staged_video(key: &str) -> Value {
    json!({ "S3Object": { "Bucket": BUCKET, "Name": key } })
}

async fn start(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "StartFaceDetection", body).await
}

/// The `JobId` of a start that was carried out, with the constraints the model puts on a
/// `JobId` already checked (`smithy_query(query="shape",shape="JobId")`: `length 1..64`,
/// `pattern ^[a-zA-Z0-9-_]+$`).
fn job_id_of(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the start must be carried out, got {} with {:?}",
        reply.status, reply.body
    );
    let job_id = reply.string("JobId").unwrap_or_else(|| {
        panic!("case {case:?}: a start answers with a JobId, got {:?}", reply.body)
    });
    assert!(
        (1..=64).contains(&job_id.chars().count()),
        "case {case:?}: a JobId is 1..64 characters, got {job_id:?}"
    );
    assert!(
        job_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'),
        "case {case:?}: a JobId matches ^[a-zA-Z0-9-_]+$, got {job_id:?}"
    );
    job_id
}

/// What every refusal of a start must carry: the status, the envelope's `__type`, the body's
/// own `Code` and the class's fixed wording.
fn assert_refusal(case: &str, reply: &Reply, code: &str, message: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected __type {code}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Code").as_deref(),
        Some(code),
        "case {case:?}: expected Code {code}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Message").as_deref(),
        Some(message),
        "case {case:?}: expected Message {message:?}, got {:?}",
        reply.body
    );
    assert!(
        reply.field("JobId").is_none(),
        "case {case:?}: a refused start hands out no job identifier, got {:?}",
        reply.body
    );
}

/// A start on a video the object store serves is carried out whatever else the request
/// carries, and every job it records gets an identifier of its own.
#[tokio::test]
async fn a_start_on_a_served_video_records_a_job_and_answers_its_identifier() {
    let router = seeded().await;

    // The rows differ along the two things a start varies in: which object the video names,
    // and which of the operation's optional members the request carries, at which values
    // (`smithy_query(query="operation",operation="StartFaceDetection")`:
    // `FaceAttributes` is one of DEFAULT | ALL, `JobTag` and `NotificationChannel` are free).
    let mut extras: Vec<(&str, Value)> = vec![
        ("nothing else", json!({})),
        (
            "the default attribute set",
            json!({ "FaceAttributes": "DEFAULT" }),
        ),
        (
            "every optional member at once, with a job tag of every character class the \
             pattern admits",
            json!({
                "FaceAttributes": "ALL",
                "JobTag": "group_1.a-b:c+d=e/f",
                "NotificationChannel": channel("AmazonRekognitionAll"),
            }),
        ),
    ];
    // A start carrying no token is a fresh job every time, so the same request twice is two
    // jobs rather than one: the row is repeated to assert exactly that.
    extras.push(("nothing else", json!({})));

    // Every job this table starts is still running while the table runs, so the table stays
    // below the ceiling on concurrently running jobs — which is what the ceiling's own test
    // below drives, at instants rather than over the wire.
    assert!(
        STAGED_KEYS.len() * extras.len() < MAX_CONCURRENT_JOBS,
        "the table must stay below the {MAX_CONCURRENT_JOBS} running jobs a start is \
         refused past, got {} rows",
        STAGED_KEYS.len() * extras.len()
    );

    let mut handed_out: Vec<String> = Vec::new();
    for key in STAGED_KEYS {
        for (extra_case, extra) in &extras {
            let mut body = extra.clone();
            body.as_object_mut()
                .expect("the extra members are an object")
                .insert("Video".to_string(), staged_video(key));
            let case = format!("{key} with {extra_case}");
            let job_id = job_id_of(&case, &start(&router, body).await);
            assert!(
                !handed_out.contains(&job_id),
                "case {case:?}: every start with no token records its own job, but \
                 {job_id:?} was handed out before"
            );
            handed_out.push(job_id);
        }
    }
    assert_eq!(
        handed_out.len(),
        STAGED_KEYS.len() * extras.len(),
        "every row must have been answered with a job identifier, got {handed_out:?}"
    );
}

/// A `ClientRequestToken` makes a start idempotent: "if you use the same token with multiple
/// StartFaceDetection requests, the same JobId is returned", and a token reused for a
/// request that differs in any other member is the modelled refusal — which records nothing,
/// so the job the token already started is still the one it answers with afterwards.
#[tokio::test]
async fn a_reused_client_request_token_answers_with_the_job_it_already_started() {
    let router = seeded().await;
    let token = "start-face-detection-token";
    let request = json!({
        "Video": staged_video("clip.mp4"),
        "ClientRequestToken": token,
        "FaceAttributes": "ALL",
        "JobTag": "idempotent",
        "NotificationChannel": channel("AmazonRekognitionIdempotent"),
    });

    let first = job_id_of("the first start", &start(&router, request.clone()).await);

    // Another start in between records its own job; the token still names the first one.
    let other = job_id_of(
        "an unrelated start in between",
        &start(&router, json!({ "Video": staged_video("clip.mov") })).await,
    );
    assert_ne!(
        other, first,
        "a start under no token records its own job, got {other:?} for both"
    );

    for attempt in ["the second start", "the third start"] {
        let repeated = job_id_of(attempt, &start(&router, request.clone()).await);
        assert_eq!(
            repeated, first,
            "case {attempt:?}: the same token with the same request answers with the same \
             job, expected {first:?}, got {repeated:?}"
        );
    }

    // A fresh token is a fresh request, whatever else it repeats.
    let fresh = json!({
        "Video": staged_video("clip.mp4"),
        "ClientRequestToken": "start-face-detection-token-2",
        "FaceAttributes": "ALL",
        "JobTag": "idempotent",
        "NotificationChannel": channel("AmazonRekognitionIdempotent"),
    });
    let under_fresh_token = job_id_of("a fresh token", &start(&router, fresh).await);
    assert_ne!(
        under_fresh_token, first,
        "a different token starts a different job, got {first:?} for both"
    );

    // One row per member "at least one of the other input parameters" can differ in: the
    // video's bucket, key and version, the analysis's own parameter, the job tag and the
    // notification channel, each differing in exactly one member from the committed request.
    let mismatches: Vec<(&str, Value)> = vec![
        (
            "a different object key",
            json!({
                "Video": staged_video("clip.mov"),
                "ClientRequestToken": token,
                "FaceAttributes": "ALL",
                "JobTag": "idempotent",
                "NotificationChannel": channel("AmazonRekognitionIdempotent"),
            }),
        ),
        (
            "a pinned object version",
            json!({
                "Video": { "S3Object": { "Bucket": BUCKET, "Name": "clip.mp4", "Version": "0123456789abcdef" } },
                "ClientRequestToken": token,
                "FaceAttributes": "ALL",
                "JobTag": "idempotent",
                "NotificationChannel": channel("AmazonRekognitionIdempotent"),
            }),
        ),
        (
            "a different attribute set",
            json!({
                "Video": staged_video("clip.mp4"),
                "ClientRequestToken": token,
                "FaceAttributes": "DEFAULT",
                "JobTag": "idempotent",
                "NotificationChannel": channel("AmazonRekognitionIdempotent"),
            }),
        ),
        (
            "no attribute set at all",
            json!({
                "Video": staged_video("clip.mp4"),
                "ClientRequestToken": token,
                "JobTag": "idempotent",
                "NotificationChannel": channel("AmazonRekognitionIdempotent"),
            }),
        ),
        (
            "a different job tag",
            json!({
                "Video": staged_video("clip.mp4"),
                "ClientRequestToken": token,
                "FaceAttributes": "ALL",
                "JobTag": "different",
                "NotificationChannel": channel("AmazonRekognitionIdempotent"),
            }),
        ),
        (
            "a different notification topic",
            json!({
                "Video": staged_video("clip.mp4"),
                "ClientRequestToken": token,
                "FaceAttributes": "ALL",
                "JobTag": "idempotent",
                "NotificationChannel": channel("AmazonRekognitionOther"),
            }),
        ),
        (
            "no notification channel",
            json!({
                "Video": staged_video("clip.mp4"),
                "ClientRequestToken": token,
                "FaceAttributes": "ALL",
                "JobTag": "idempotent",
            }),
        ),
    ];
    for (case, body) in mismatches {
        let reply = start(&router, body).await;
        assert_refusal(
            case,
            &reply,
            "IdempotentParameterMismatchException",
            IDEMPOTENT_MISMATCH_MESSAGE,
        );
        let still = job_id_of("the token after a refused reuse", &start(&router, request.clone()).await);
        assert_eq!(
            still, first,
            "case {case:?}: a refused reuse records nothing, so the token still names \
             {first:?}, got {still:?}"
        );
    }
}

/// A video the object store does not serve is refused as an unreadable S3 object, however it
/// fails to name one the store holds.
#[tokio::test]
async fn a_video_the_store_does_not_serve_is_refused_as_an_unreadable_object() {
    let router = seeded().await;
    let unreachable: Vec<(&str, Value)> = vec![
        // The recorded case this issue was filed from: a start on an object nothing staged.
        (
            "a bucket the store does not hold",
            json!({ "S3Object": { "Bucket": ABSENT_BUCKET, "Name": "clip.mp4" } }),
        ),
        (
            "a key the bucket does not hold",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "absent.mp4" } }),
        ),
        (
            "a key that only prefixes a stored one",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "clip" } }),
        ),
        (
            "a key that only extends a stored one",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "clip.mp4.part" } }),
        ),
        (
            "a version the bucket never held",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "clip.mp4", "Version": "0123456789abcdef" } }),
        ),
        // An object named incompletely is an object nothing could be read from: there is no
        // address to read without both a bucket and a key.
        (
            "a bucket with no key",
            json!({ "S3Object": { "Bucket": BUCKET } }),
        ),
        (
            "a key with no bucket",
            json!({ "S3Object": { "Name": "clip.mp4" } }),
        ),
        ("an object naming neither", json!({ "S3Object": {} })),
    ];
    for (case, video) in unreachable {
        let reply = start(&router, json!({ "Video": video })).await;
        assert_refusal(
            case,
            &reply,
            "InvalidS3ObjectException",
            UNREADABLE_OBJECT_MESSAGE,
        );
    }
}

/// A request whose `Video` identifies no object at all is refused as the invalid parameter it
/// is — never as an unreadable object, so the two failures stay distinguishable — whatever
/// else the request carries.
#[tokio::test]
async fn a_request_that_identifies_no_video_is_refused_as_an_invalid_parameter() {
    let router = seeded().await;
    let extras: Vec<(&str, Value)> = vec![
        ("nothing else", json!({})),
        ("an attribute set", json!({ "FaceAttributes": "ALL" })),
        ("a job tag", json!({ "JobTag": "no-video" })),
        (
            "a token and a notification channel",
            json!({
                "ClientRequestToken": "no-video-token",
                "NotificationChannel": channel("AmazonRekognitionNoVideo"),
            }),
        ),
    ];
    for (case, extra) in extras {
        let mut body = extra.clone();
        body.as_object_mut()
            .expect("the extra members are an object")
            .insert("Video".to_string(), json!({}));
        let reply = start(&router, body).await;
        assert_refusal(
            case,
            &reply,
            "InvalidParameterException",
            INVALID_PARAMETERS_MESSAGE,
        );
    }
}

/// One job record, started at `started_at` on a video of the staged bucket.
fn job(job_id: &str, started_at: SystemTime) -> VideoJob {
    VideoJob::new(
        job_id.to_string(),
        "StartFaceDetection",
        VideoLocation {
            bucket: BUCKET.to_string(),
            name: "clip.mp4".to_string(),
            version: None,
        },
        started_at,
    )
}

/// A recorded job is running until the instant the clock owes its completion, and finished
/// from that instant on — the transition the start operation's own answer leaves pending.
/// The instants are handed to the record, so the rule is decided by arithmetic and nothing
/// here waits, sleeps or reads the host's clock.
#[tokio::test]
async fn a_job_is_running_until_the_clock_reaches_its_completion() {
    let started_at = UNIX_EPOCH + Duration::from_secs(1_767_225_600);
    let recorded = job("job-under-test", started_at);

    let cases: Vec<(&str, SystemTime, JobStatus, &str)> = vec![
        ("the instant it started", started_at, JobStatus::InProgress, "IN_PROGRESS"),
        (
            "a nanosecond in",
            started_at + Duration::from_nanos(1),
            JobStatus::InProgress,
            "IN_PROGRESS",
        ),
        (
            "a nanosecond before it is due",
            started_at + COMPLETION_SPAN - Duration::from_nanos(1),
            JobStatus::InProgress,
            "IN_PROGRESS",
        ),
        (
            "the instant it is due",
            started_at + COMPLETION_SPAN,
            JobStatus::Succeeded,
            "SUCCEEDED",
        ),
        (
            "long after it is due",
            started_at + COMPLETION_SPAN + Duration::from_secs(3_600),
            JobStatus::Succeeded,
            "SUCCEEDED",
        ),
    ];
    for (case, now, expected, modeled) in cases {
        let actual = recorded.status_at(now);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            actual.modeled(),
            modeled,
            "case {case:?}: expected the modelled value {modeled:?}, got {:?}",
            actual.modeled()
        );
    }
    assert!(
        COMPLETION_SPAN > Duration::ZERO,
        "a start answers while its analysis is still running, so the span is never zero"
    );
}

/// The store counts as running exactly the jobs whose completion the clock has not reached,
/// so the concurrency ceiling is reached by jobs that are still running and released by the
/// same jobs once they are due.
#[tokio::test]
async fn the_concurrency_ceiling_counts_only_the_jobs_still_running() {
    let started_at = UNIX_EPOCH + Duration::from_secs(1_767_225_600);
    let due = started_at + COMPLETION_SPAN;
    let mut jobs = VideoJobs::default();

    for started in 0..MAX_CONCURRENT_JOBS {
        let job_id = format!("job-{started}");
        jobs.record(job(&job_id, started_at));
        let running = jobs.running_at(started_at);
        assert_eq!(
            running,
            started + 1,
            "case {job_id:?}: every recorded job is running at the instant it started, \
             expected {}, got {running}",
            started + 1
        );
        assert_eq!(
            jobs.get(&job_id).map(|job| job.job_id.as_str()),
            Some(job_id.as_str()),
            "case {job_id:?}: a recorded job is the one its identifier names, got {:?}",
            jobs.get(&job_id).map(|job| job.job_id.clone())
        );
        let at_limit = jobs.at_concurrency_limit(started_at);
        assert_eq!(
            at_limit,
            started + 1 >= MAX_CONCURRENT_JOBS,
            "case {job_id:?}: the ceiling is {MAX_CONCURRENT_JOBS} running jobs, with \
             {} recorded it reported {at_limit}",
            started + 1
        );
        // Due is due whatever else is running: a job that is finished is not running.
        assert_eq!(
            jobs.running_at(due),
            0,
            "case {job_id:?}: no job is still running once the clock has reached its \
             completion, got {}",
            jobs.running_at(due)
        );
        assert!(
            !jobs.at_concurrency_limit(due),
            "case {job_id:?}: a ceiling of running jobs is released by their completion"
        );
    }

    // The token lookup is per analysis AND per token: a job records the token it was started
    // under, and nothing else answers for it.
    let mut tokened = job("job-with-token", started_at);
    tokened.client_request_token = Some("a-token".to_string());
    jobs.record(tokened);
    for (case, analysis, token, expected) in [
        (
            "the analysis and token it was started under",
            "StartFaceDetection",
            "a-token",
            Some("job-with-token"),
        ),
        (
            "another token of the same analysis",
            "StartFaceDetection",
            "another-token",
            None,
        ),
        (
            "the same token of another analysis",
            "StartLabelDetection",
            "a-token",
            None,
        ),
    ] {
        let found = jobs
            .started_under_token(analysis, token)
            .map(|job| job.job_id.clone());
        assert_eq!(
            found.as_deref(),
            expected,
            "case {case:?}: expected {expected:?}, got {found:?}"
        );
    }
}
