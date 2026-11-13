// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's model-card export surface:
//! `CreateModelCardExportJob` and the `DescribeModelCardExportJob` that reads a job back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via
//! the `Emulator` harness `inference_deployment_lifecycle` owns, so what these tests
//! exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call. The rows
//! differ along what each rule generalises over: the job name's shape, the `S3OutputPath`
//! scheme and key, whether the request names a card version and which one, how many
//! versions the card has, the signed region, and name-vs-ARN for the card reference.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state::MODEL_CARD_EXPORT_JOB_SETTLES_AFTER;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::model_card_governance::{content_with_description, create_body, number_field};

/// The span the clock owes a job's terminal status, as `crate::state` states it: a read
/// that reaches the job before it has elapsed sees `InProgress`, one after it sees the
/// terminal status.
const SETTLES_AFTER: Duration = MODEL_CARD_EXPORT_JOB_SETTLES_AFTER;

/// Long enough to pass [`SETTLES_AFTER`], so the terminal status the clock owes is due by
/// the time the next call runs.
pub(crate) const PAST_SETTLE_DUE: Duration = SETTLES_AFTER.saturating_mul(3);

/// A deadline far longer than the span the state gives a job's export, so a loaded runner
/// does not decide whether a poll loop terminates.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What a `FailureReason` says for an export that wrote no document, as
/// `crate::state::export_failed` words it: every row's `S3OutputPath` names a bucket this
/// account does not hold, so the write is refused and the job fails.
const EXPORT_FAILED_PREFIX: &str = "Model card export failed";

/// The bucket every [`OUTPUT_PATHS`] entry names, in both spellings — and one this account
/// does not hold, so every export of this suite is refused by Amazon S3 and the job fails.
const EXPORT_BUCKET: &str = "export-bucket";

/// `ModelCardExportOutputConfig$S3OutputPath` is an `S3Uri`, modelled
/// `^(https|s3)://([^/]+)/?(.*)$` (smithy_query query="shape"
/// shape="ModelCardExportOutputConfig") — so both schemes, with and without a key, name
/// a path a caller may export to, and the job stores whichever it was given.
pub(crate) const OUTPUT_PATHS: [&str; 4] = [
    "s3://export-bucket",
    "s3://export-bucket/model-cards/",
    "s3://export-bucket/deeply/nested/prefix",
    "https://export-bucket.s3.us-east-1.amazonaws.com/cards/",
];

/// `ModelCardExportJobName` is an `EntityName`, `len 1..63` with pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` (smithy_query query="operation"
/// operation="CreateModelCardExportJob"): one character, digits, hyphens, and the
/// modelled 63-character maximum.
pub(crate) const JOB_NAMES: [&str; 4] = [
    "e",
    "export1",
    "nightly-export-of-the-card",
    "a234567890123456789012345678901234567890123456789012345678901b",
];

/// A `CreateModelCardExportJob` body exporting `card` under the job name `job` to `path`,
/// naming `version` when the case pins one.
pub(crate) fn export_body(card: &str, job: &str, path: &str, version: Option<i32>) -> Value {
    let mut body = json!({
        "ModelCardName": card,
        "ModelCardExportJobName": job,
        "OutputConfig": { "S3OutputPath": path },
    });
    if let Some(version) = version {
        body["ModelCardVersion"] = json!(version);
    }
    body
}

/// The ARN a job of `job` exporting `card` carries in `region`.
pub(crate) fn job_arn(region: &str, card: &str, job: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-card/{card}/export-job/{job}")
}

/// Create `card` in `emulator` with `versions` versions: the create mints version 1 and
/// each further version is appended by an `UpdateModelCard` carrying new content.
pub(crate) async fn with_card(emulator: &Emulator, card: &str, versions: i32) {
    emulator
        .ok(
            "CreateModelCard",
            &create_body(card, "Draft", &content_with_description("version 1")),
        )
        .await;
    for version in 2..=versions {
        emulator
            .ok(
                "UpdateModelCard",
                &json!({
                    "ModelCardName": card,
                    "Content": content_with_description(&format!("version {version}")),
                }),
            )
            .await;
    }
}

/// RULE: `CreateModelCardExportJob` registers the job against the card version it
/// resolves — the requested one, or the card's latest when the request names none — under
/// the ARN `<card ARN>/export-job/<job name>`, and `DescribeModelCardExportJob` replays
/// exactly what was stored: the job's name, that card and version, the `OutputConfig` as
/// given, and the `InProgress` status it starts in.
#[tokio::test]
async fn created_export_jobs_round_trip_through_describe() {
    // Rows differ along what the rule generalises over: the job name's shape, the output
    // path's scheme and key, how many versions the card holds, and which version (if any)
    // the request pins.
    let cases: [(&str, i32, Option<i32>, i32); 8] = [
        // (case, versions the card holds, requested version, version exported)
        ("single-version-latest", 1, None, 1),
        ("single-version-pinned", 1, Some(1), 1),
        ("three-versions-latest", 3, None, 3),
        ("three-versions-first", 3, Some(1), 1),
        ("three-versions-middle", 3, Some(2), 2),
        ("three-versions-last", 3, Some(3), 3),
        ("five-versions-latest", 5, None, 5),
        ("five-versions-fourth", 5, Some(4), 4),
    ];

    for (index, (case, held, requested, exported)) in cases.into_iter().enumerate() {
        let job = JOB_NAMES[index % JOB_NAMES.len()];
        let path = OUTPUT_PATHS[index % OUTPUT_PATHS.len()];
        let card = "governed-card";
        let case = format!("{case} job={job} path={path} held={held} requested={requested:?}");

        let emulator = Emulator::new();
        with_card(&emulator, card, held).await;

        // When the create ran, so the row can tell a describe that reached the job before
        // its terminal status fell due from one a loaded runner delayed past it.
        let started = Instant::now();
        let (status, created) = emulator
            .call(
                "CreateModelCardExportJob",
                &export_body(card, job, path, requested),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected CreateModelCardExportJob to succeed, got {status} with \
             {created}"
        );
        let expected_arn = job_arn("us-east-1", card, job);
        assert_eq!(
            str_field(&created, "ModelCardExportJobArn"),
            expected_arn,
            "case {case}: expected ModelCardExportJobArn {expected_arn}, got {created:?}"
        );

        let described = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": expected_arn }),
            )
            .await;
        assert_eq!(
            str_field(&described, "ModelCardExportJobArn"),
            expected_arn,
            "case {case}: expected the described ARN to be the created one, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ModelCardExportJobName"),
            job,
            "case {case}: expected ModelCardExportJobName {job}, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ModelCardName"),
            card,
            "case {case}: expected ModelCardName {card}, got {described:?}"
        );
        assert_eq!(
            number_field(&described, "ModelCardVersion"),
            f64::from(exported),
            "case {case}: expected the exported version to be {exported}, got {described:?}"
        );
        assert_eq!(
            described.get("OutputConfig").cloned(),
            Some(json!({ "S3OutputPath": path })),
            "case {case}: expected OutputConfig to round trip, got {described:?}"
        );
        // The status and the `FailureReason` always agree: `InProgress` with no reason
        // until the clock's due instant, and after it the terminal status the job's own
        // export produced — here the modelled `Failed`, because the row's output location
        // names a bucket this account does not hold. Which of the two this read sees
        // depends only on how long the two calls took.
        let job_status = str_field(&described, "Status");
        let reason = described
            .get("FailureReason")
            .and_then(Value::as_str)
            .map(str::to_string);
        let before_due = started.elapsed() < SETTLES_AFTER;
        assert!(
            (job_status == "InProgress" && reason.is_none())
                || (job_status == "Failed"
                    && reason
                        .as_deref()
                        .is_some_and(|reason| reason.contains(EXPORT_FAILED_PREFIX))),
            "case {case}: a job is InProgress with no FailureReason until its end is due \
             and Failed with the reason after it, got {described:?}"
        );
        if before_due {
            assert_eq!(
                job_status, "InProgress",
                "case {case}: a job describes as running before its end is due, got \
                 {described:?}"
            );
        }
        assert_eq!(
            described.get("ExportArtifacts"),
            None,
            "case {case}: no document was written, so the job names no artifact, got \
             {described:?}"
        );
        assert!(
            number_field(&described, "CreatedAt") > 0.0,
            "case {case}: expected a CreatedAt after the epoch, got {described:?}"
        );
        if before_due {
            // Nothing has modified the job since it was accepted, so the two instants are
            // the one instant the create recorded.
            assert_eq!(
                number_field(&described, "LastModifiedAt"),
                number_field(&described, "CreatedAt"),
                "case {case}: expected LastModifiedAt to equal CreatedAt on a running job, \
                 got {described:?}"
            );
        }

        // The version this job exports is the one the create resolved: appending another
        // version to the card afterwards does not change what the job reports.
        emulator
            .ok(
                "UpdateModelCard",
                &json!({
                    "ModelCardName": card,
                    "Content": content_with_description("appended after the export started"),
                }),
            )
            .await;
        let reread = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": expected_arn }),
            )
            .await;
        assert_eq!(
            number_field(&reread, "ModelCardVersion"),
            f64::from(exported),
            "case {case}: a newer card version must not change the exported one, got \
             {reread:?}"
        );
    }
}

/// RULE: an export job's ARN is `arn:aws:sagemaker:<signed region>:<account>:model-card/
/// <card>/export-job/<job>` (the emulator's default region when the request names none),
/// and the card may be addressed by its name or by its ARN — `ModelCardName` is a
/// `ModelCardNameOrArn`, so both forms resolve to the one stored card.
#[tokio::test]
async fn export_job_arns_carry_the_signed_region_and_the_card_is_named_either_way() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        for (index, card) in ["c", "regional-card"].into_iter().enumerate() {
            let job = JOB_NAMES[index % JOB_NAMES.len()];
            let case = format!("region={region} card={card} job={job}");
            let emulator = Emulator::new();
            let (card_status, created_card) = emulator
                .call_in(
                    region,
                    "CreateModelCard",
                    &create_body(card, "Draft", &content_with_description("regional")),
                )
                .await;
            assert_eq!(
                card_status, 200,
                "case {case}: expected CreateModelCard to succeed, got {card_status} with \
                 {created_card}"
            );
            let card_arn = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-card/{card}");

            // Both forms of `ModelCardNameOrArn` name the same card, so each exports it;
            // the two jobs differ only in the name they were started under.
            for (reference_index, reference) in [card.to_string(), card_arn.clone()]
                .into_iter()
                .enumerate()
            {
                let job_name = format!("{job}-{reference_index}");
                let (status, created) = emulator
                    .call_in(
                        region,
                        "CreateModelCardExportJob",
                        &export_body(&reference, &job_name, OUTPUT_PATHS[0], None),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case} reference={reference}: expected the export job to be \
                     created, got {status} with {created}"
                );
                let expected_arn = job_arn(region, card, &job_name);
                assert_eq!(
                    str_field(&created, "ModelCardExportJobArn"),
                    expected_arn,
                    "case {case} reference={reference}: expected {expected_arn}, got \
                     {created:?}"
                );

                let (read_status, described) = emulator
                    .call_in(
                        region,
                        "DescribeModelCardExportJob",
                        &json!({ "ModelCardExportJobArn": expected_arn }),
                    )
                    .await;
                assert_eq!(
                    read_status, 200,
                    "case {case} reference={reference}: expected the job to be readable, \
                     got {read_status} with {described}"
                );
                assert_eq!(
                    str_field(&described, "ModelCardName"),
                    card,
                    "case {case} reference={reference}: expected the card's own name, got \
                     {described:?}"
                );
            }
        }
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModelCard",
            &create_body("unsigned-card", "Draft", "{}"),
        )
        .await;
    let (status, created) = emulator
        .call_unsigned(
            "CreateModelCardExportJob",
            &export_body("unsigned-card", "unsigned-export", OUTPUT_PATHS[1], None),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected the export job to be created, got {status} with {created}"
    );
    assert_eq!(
        str_field(&created, "ModelCardExportJobArn"),
        job_arn("us-east-1", "unsigned-card", "unsigned-export"),
        "case unsigned: expected the default-region ARN, got {created:?}"
    );
}

/// RULE: an export job is created `InProgress` and the clock owes it a terminal status a
/// span later; because every row here exports to a bucket this account does not hold, that
/// terminal status is the modelled `Failed` with a `FailureReason` naming the location, no
/// `ExportArtifacts`, and a `LastModifiedAt` that is stable across later reads. Holds for
/// every job, whatever its shape.
#[tokio::test]
async fn an_export_job_reaches_the_terminal_status_the_clock_owes_it() {
    // Rows differ along the shape of the job: the version pinned, the output path and the
    // job's name — none of which changes the status the clock owes.
    let cases: [(&str, Option<i32>); 4] = [
        ("latest-version", None),
        ("pinned-first-version", Some(1)),
        ("pinned-second-version", Some(2)),
        ("pinned-latest-version", Some(3)),
    ];

    for (index, (case, requested)) in cases.into_iter().enumerate() {
        let job = JOB_NAMES[index % JOB_NAMES.len()];
        let path = OUTPUT_PATHS[index % OUTPUT_PATHS.len()];
        let case = format!("{case} job={job} path={path}");
        let emulator = Emulator::new();
        with_card(&emulator, "settling-card", 3).await;
        let started = Instant::now();
        emulator
            .ok(
                "CreateModelCardExportJob",
                &export_body("settling-card", job, path, requested),
            )
            .await;
        let arn = job_arn("us-east-1", "settling-card", job);

        let running = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": arn }),
            )
            .await;
        // Only when the two calls really did land before the due instant: on a loaded
        // runner they can take longer than the span, and then the terminal status below is
        // what this read correctly reports.
        if started.elapsed() < SETTLES_AFTER {
            assert_eq!(
                str_field(&running, "Status"),
                "InProgress",
                "case {case}: a job describes as running before its end is due, got \
                 {running:?}"
            );
        }

        // Wait for the CONDITION the clock owes — the move out of `InProgress` — by
        // looking again until it holds or a generous deadline passes.
        let deadline = Instant::now() + TERMINAL_DEADLINE;
        let settled = loop {
            let read = emulator
                .ok(
                    "DescribeModelCardExportJob",
                    &json!({ "ModelCardExportJobArn": arn }),
                )
                .await;
            if str_field(&read, "Status") != "InProgress" {
                break read;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: the job must leave InProgress, last read {read:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            str_field(&settled, "Status"),
            "Failed",
            "case {case}: the terminal status the clock owes, got {settled:?}"
        );
        let reason = str_field(&settled, "FailureReason");
        assert!(
            reason.contains(EXPORT_FAILED_PREFIX) && reason.contains(EXPORT_BUCKET),
            "case {case}: FailureReason must say the export failed and name the object it \
             was writing under the output path, got {reason:?}"
        );
        assert_eq!(
            settled.get("ExportArtifacts"),
            None,
            "case {case}: a job that exported nothing names no artifact, got {settled:?}"
        );
        // `LastModifiedAt` is the instant the move was DUE, not the instant it was read:
        // the due instant is the stored `CreatedAt + SETTLES_AFTER`, which is exact
        // because it is stored at create time rather than slept on — while the read itself
        // polled until the move had happened.
        let created_at = number_field(&settled, "CreatedAt");
        let modified_at = number_field(&settled, "LastModifiedAt");
        let due_at = created_at + SETTLES_AFTER.as_secs_f64();
        assert!(
            (modified_at - due_at).abs() < 1e-3,
            "case {case}: expected LastModifiedAt to be the due instant {due_at} \
             (CreatedAt {created_at} + {}s), got {modified_at}",
            SETTLES_AFTER.as_secs_f64()
        );

        // The outcome was owed at a stored instant, so reading it again reports the same
        // body: nothing about the read moves the clock on.
        let reread = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": arn }),
            )
            .await;
        assert_eq!(
            reread, settled,
            "case {case}: a second describe must report the same job"
        );
    }
}

/// RULE: an export job naming a model card, or a version of a card, that the store does
/// not hold is refused with the modelled `ResourceNotFound` (HTTP 400) whose message
/// names the card's ARN as built from the request, and nothing is persisted — never a 200
/// for an export of a card nobody created.
#[tokio::test]
async fn exporting_a_card_or_version_that_is_not_stored_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    with_card(&emulator, "stored-card", 2).await;

    // Rows: an unknown card by name and in ARN form, and versions either side of the two
    // the stored card has. The fourth column is the card the reference names, which the
    // refusal's message is built from.
    let cases: [(&str, &str, Option<i32>, &str); 6] = [
        ("unknown card", "no-such-card", None, "no-such-card"),
        (
            "unknown card, pinned version",
            "no-such-card",
            Some(1),
            "no-such-card",
        ),
        (
            "unknown card as ARN",
            "arn:aws:sagemaker:us-east-1:000000000000:model-card/no-such-card",
            None,
            "no-such-card",
        ),
        (
            "version above the latest",
            "stored-card",
            Some(3),
            "stored-card",
        ),
        ("version zero", "stored-card", Some(0), "stored-card"),
        ("negative version", "stored-card", Some(-1), "stored-card"),
    ];

    for (case, card, version, named) in cases {
        let (status, refused) = emulator
            .call(
                "CreateModelCardExportJob",
                &export_body(card, "refused-export", OUTPUT_PATHS[0], version),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound, got {refused:?}"
        );
        // The message is built from the request: SageMaker names the card's ARN and says
        // no more, so a version the card never had reads the same as a card nobody
        // created.
        let expected_message = format!(
            "Modelcard arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/{named} does not exist"
        );
        assert_eq!(
            message(&refused),
            expected_message,
            "case {case}: expected the refusal to name the card's ARN, got {:?}",
            message(&refused)
        );
        // Nothing was persisted: the reader refuses the ARN the job would have carried.
        let (read_status, missing) = emulator
            .call(
                "DescribeModelCardExportJob",
                &json!({
                    "ModelCardExportJobArn":
                        job_arn("us-east-1", "stored-card", "refused-export"),
                }),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: expected the refused create to persist nothing, got {read_status} \
             with {missing}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound after a refused create, got {missing:?}"
        );
    }

    // The version the card does have is exportable: the refusals above changed nothing.
    let created = emulator
        .ok(
            "CreateModelCardExportJob",
            &export_body("stored-card", "accepted-export", OUTPUT_PATHS[0], Some(2)),
        )
        .await;
    assert_eq!(
        str_field(&created, "ModelCardExportJobArn"),
        job_arn("us-east-1", "stored-card", "accepted-export"),
        "case stored version: expected the job's ARN, got {created:?}"
    );
}

/// RULE: a reader that names an export job the store does not hold is refused with the
/// modelled `ResourceNotFound` (HTTP 400) whose message is built from the request — the
/// ARN it asked about, and no more — never a 200 with empty members. A job is scoped to
/// the partition, Region and account its ARN names, so an ARN that differs only in one of
/// those does not address it.
#[tokio::test]
async fn describing_an_export_job_that_is_not_stored_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    with_card(&emulator, "stored-card", 1).await;
    emulator
        .ok(
            "CreateModelCardExportJob",
            &export_body("stored-card", "stored-export", OUTPUT_PATHS[0], None),
        )
        .await;

    // Rows: an unknown job of a stored card, a job of an unknown card, and the STORED
    // job's own names under another Region, another account and another partition — a
    // resource is scoped to the Region and account its ARN names, so none of those
    // addresses the stored job. Every row is a value the modelled
    // `ModelCardExportJobArn` pattern admits.
    let cases: [(&str, String); 5] = [
        (
            "unknown job of a stored card",
            job_arn("us-east-1", "stored-card", "no-such-export"),
        ),
        (
            "job of an unknown card",
            job_arn("us-east-1", "no-such-card", "stored-export"),
        ),
        (
            "the stored job's names in another region",
            job_arn("eu-west-1", "stored-card", "stored-export"),
        ),
        (
            "the stored job's names in another account",
            "arn:aws:sagemaker:us-east-1:999999999999:model-card/stored-card/\
             export-job/stored-export"
                .to_string(),
        ),
        (
            "the stored job's names in another partition",
            format!(
                "arn:aws-cn:sagemaker:us-east-1:{ACCOUNT}:model-card/stored-card/\
                 export-job/stored-export"
            ),
        ),
    ];

    for (case, arn) in cases {
        let (status, refused) = emulator
            .call(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": arn }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!("{arn} does not exist"),
            "case {case}: expected the refusal to name the ARN asked about, got {:?}",
            message(&refused)
        );
    }

    // The job that is stored is still readable: the refusals above changed nothing.
    let described = emulator
        .ok(
            "DescribeModelCardExportJob",
            &json!({
                "ModelCardExportJobArn": job_arn("us-east-1", "stored-card", "stored-export"),
            }),
        )
        .await;
    assert_eq!(
        str_field(&described, "ModelCardExportJobName"),
        "stored-export",
        "case stored job: expected the stored job, got {described:?}"
    );
}

/// RULE: an export-job name already used for ONE card is refused with the modelled
/// `ConflictException` (HTTP 400) and the job already stored is left exactly as it was;
/// the name is scoped to the card its ARN carries, so the same name on another card is a
/// different job and is accepted.
#[tokio::test]
async fn a_job_name_already_used_for_a_card_is_refused_but_free_on_another_card() {
    // Rows differ in how far the second request's body differs from the first's: the
    // refusal is keyed to the name, not to what the second call carries.
    let cases = [
        (
            "identical-body",
            None,
            OUTPUT_PATHS[0],
            None,
            OUTPUT_PATHS[0],
        ),
        (
            "different-output-path",
            None,
            OUTPUT_PATHS[1],
            None,
            OUTPUT_PATHS[2],
        ),
        (
            "different-version-and-path",
            Some(1),
            OUTPUT_PATHS[0],
            Some(2),
            OUTPUT_PATHS[3],
        ),
    ];

    for (case, first_version, first_path, second_version, second_path) in cases {
        let emulator = Emulator::new();
        with_card(&emulator, "card-one", 2).await;
        with_card(&emulator, "card-two", 2).await;
        emulator
            .ok(
                "CreateModelCardExportJob",
                &export_body("card-one", "shared-name", first_path, first_version),
            )
            .await;

        let (status, refused) = emulator
            .call(
                "CreateModelCardExportJob",
                &export_body("card-one", "shared-name", second_path, second_version),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a duplicate job name, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case}: expected ConflictException, got {refused:?}"
        );
        assert!(
            message(&refused).contains("shared-name"),
            "case {case}: expected the refusal to name the job, got {:?}",
            message(&refused)
        );

        // The job the first call created survives untouched.
        let described = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({
                    "ModelCardExportJobArn":
                        job_arn("us-east-1", "card-one", "shared-name"),
                }),
            )
            .await;
        assert_eq!(
            described.get("OutputConfig").cloned(),
            Some(json!({ "S3OutputPath": first_path })),
            "case {case}: expected the first job's OutputConfig to survive, got {described:?}"
        );
        assert_eq!(
            number_field(&described, "ModelCardVersion"),
            f64::from(first_version.unwrap_or(2)),
            "case {case}: expected the first job's exported version to survive, got \
             {described:?}"
        );

        // The same name on another card is another job, and it is accepted.
        let (other_status, other) = emulator
            .call(
                "CreateModelCardExportJob",
                &export_body("card-two", "shared-name", second_path, second_version),
            )
            .await;
        assert_eq!(
            other_status, 200,
            "case {case}: expected the name to be free on another card, got {other_status} \
             with {other}"
        );
        assert_eq!(
            str_field(&other, "ModelCardExportJobArn"),
            job_arn("us-east-1", "card-two", "shared-name"),
            "case {case}: expected the other card's job ARN, got {other:?}"
        );
        let other_described = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({
                    "ModelCardExportJobArn":
                        job_arn("us-east-1", "card-two", "shared-name"),
                }),
            )
            .await;
        assert_eq!(
            str_field(&other_described, "ModelCardName"),
            "card-two",
            "case {case}: expected the other card's job to name it, got {other_described:?}"
        );
        assert_eq!(
            other_described.get("OutputConfig").cloned(),
            Some(json!({ "S3OutputPath": second_path })),
            "case {case}: expected the other job's own OutputConfig, got {other_described:?}"
        );
    }
}
