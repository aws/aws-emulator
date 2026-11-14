// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wording of the Glue JOB-DEFINITION miss: the `Message` of the
//! `EntityNotFoundException` every operation that resolves a `JobName` answers when the
//! Region it is served in holds no job of that name.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a request that addresses a job definition by a name no job of the
//! request's Region holds is refused with HTTP 400 and `EntityNotFoundException`, and the
//! refusal's `Message` is ONE fixed, request-independent sentence — it carries no variable
//! part, so it discloses neither the requested name, nor the Region, nor the resource that
//! referred to the job, and every operation that resolves a job name reports the same
//! sentence.
//!
//! The recorded case — one `GetJob` for a name whose job a `DeleteJob` had just removed —
//! is ONE row of the tables below. The rows differ along the requested name's value,
//! length, casing and character set, along WHICH operation resolves the name (a read, an
//! update, a run listing, a batch's per-run detail and a trigger's action), and along HOW
//! the name came to hold no job (never created, created then deleted, or held only by
//! another Region's endpoint).
//!
//! `GetJobRun` is the one job operation that does NOT report this sentence: it addresses a
//! RUN by the (job name, run id) pair, so an absent job is its RUN miss, which
//! [`a_run_read_of_an_absent_job_is_the_run_miss`] pins here and
//! [`crate::job_run_miss_message`] pins for a job that exists.
//!
//! The expected sentence is written out here as a literal rather than read from the
//! service's own constant, so this suite pins the wording instead of restating it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::JOB_RUN_ID_PREFIX;

use crate::error_messages::{assert_refusal, assert_withholds, JOB_RUN_NOT_FOUND_MESSAGE};
use crate::wire::{call, call_in_region, emulator, error_code, error_message, ok, ERROR_DETAIL_TAG};

/// The refusal every job-definition miss answers with: fixed text, trailing period
/// included, with nothing taken from the request.
const JOB_NOT_FOUND: &str = "Job not found.";

/// The modelled error a job-definition miss is reported through: "A specified entity does
/// not exist" (`EntityNotFoundException`, HTTP 400, from `smithy_query` on `GetJob`).
const ENTITY_NOT_FOUND: &str = "EntityNotFoundException";

/// The role ARN the jobs created here carry. An IAM role is account-global, so the same
/// one backs a job in every Region.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// Names that span the axis the refusal must be independent of: the recorded fixture's own
/// shape, a single character, a mixed-case name, one carrying punctuation and digits, one
/// carrying the space `NameString`'s pattern admits, and one at the member's documented
/// ceiling.
///
/// Source: `GetJobRequest$JobName` targets `NameString`, `length 1..255`, pattern
/// `^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$`
/// (`smithy_query(query="operation",operation="GetJob")`), so every row below is a name the
/// member admits and the 255-character row sits exactly on the published maximum.
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        (
            "recorded fixture shape",
            "sim2real-behav-glue-job-0af56235612c".to_string(),
        ),
        ("single character", "x".to_string()),
        ("mixed case", "MissingETLJob".to_string()),
        ("punctuation and digits", "team.data_lake-2026/raw".to_string()),
        ("a space the pattern admits", "nightly load".to_string()),
        ("at the 255 ceiling", "n".repeat(255)),
    ]
}

/// A `CreateJob` body for `name`, pointed at a script in Amazon S3.
fn job_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    })
}

/// The `JobUpdate` an `UpdateJob` submits: the two members that shape documents as
/// required, so the update is refused for the missing JOB and not for its own input.
fn job_update() -> Value {
    json!({
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    })
}

/// A run id of the shape a start mints that no start ever minted, for the read that
/// addresses a RUN rather than a job definition: `GetJobRun` resolves a run by the (job
/// name, run id) pair and words its miss as the RUN miss, so it is not one of the
/// job-definition lookups below — [`crate::job_run_miss_message`] pins its sentence, and
/// [`a_run_read_of_an_absent_job_is_the_run_miss`] pins it for a job nothing holds.
fn unminted_run_id() -> String {
    format!("{JOB_RUN_ID_PREFIX}{:064x}", 1)
}

/// The operations that resolve a `JobName` and answer the miss as their own top-level
/// error.
fn job_name_requests(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetJob", json!({ "JobName": name })),
        (
            "UpdateJob",
            json!({ "JobName": name, "JobUpdate": job_update() }),
        ),
        ("GetJobRuns", json!({ "JobName": name })),
    ]
}

/// `GetJobRun` is NOT one of them: it addresses a RUN by the (job name, run id) pair, and
/// the service answers its one fixed run-miss sentence whenever that pair resolves to no
/// run — including for a `JobName` this Region holds no job under, which must not be
/// reported as the JOB being missing.
///
/// The rows span the name axis of [`absent_names`] crossed with how the name misses.
#[tokio::test]
async fn a_run_read_of_an_absent_job_is_the_run_miss() {
    let router = emulator();
    ok(&router, "CreateJob", &job_body("held-job")).await;
    // A name whose job was created and then deleted is as absent as one nothing created.
    ok(&router, "CreateJob", &job_body("deleted-run-job")).await;
    ok(
        &router,
        "DeleteJob",
        &json!({ "JobName": "deleted-run-job" }),
    )
    .await;

    let mut rows: Vec<(String, String)> = absent_names()
        .into_iter()
        .map(|(case, name)| (format!("never created / {case}"), name))
        .collect();
    rows.push((
        "created then deleted".to_string(),
        "deleted-run-job".to_string(),
    ));

    for (case, name) in rows {
        let body = assert_refusal(
            &router,
            &case,
            "GetJobRun",
            &json!({ "JobName": name, "RunId": unminted_run_id() }),
            ENTITY_NOT_FOUND,
            JOB_RUN_NOT_FOUND_MESSAGE,
        )
        .await;
        let message = error_message(&body);
        assert_ne!(
            message, JOB_NOT_FOUND,
            "case {case}: a run read must not blame the job definition, got {message:?}"
        );
        assert_withholds(&case, &message, &name);
    }

    // The job this Region holds still serves the run read's own miss for an id it does not
    // hold, so the sentence above is the one miss of the pair and not a broken lookup.
    let body = assert_refusal(
        &router,
        "a job that exists and a run it does not hold",
        "GetJobRun",
        &json!({ "JobName": "held-job", "RunId": unminted_run_id() }),
        ENTITY_NOT_FOUND,
        JOB_RUN_NOT_FOUND_MESSAGE,
    )
    .await;
    assert_withholds(
        "a job that exists and a run it does not hold",
        &error_message(&body),
        "held-job",
    );
}

/// Every operation that resolves a job name answers one and the same fixed sentence for a
/// name the Region holds no job under, whatever that name looks like.
#[tokio::test]
async fn every_job_lookup_answers_one_fixed_sentence() {
    let router = emulator();
    // A job the Region DOES hold, so a refusal below is about the requested name and not
    // about the operation refusing everything.
    ok(&router, "CreateJob", &job_body("held-job")).await;

    let mut sentences = BTreeSet::new();
    for (name_case, name) in absent_names() {
        for (operation, request) in job_name_requests(&name) {
            let case = format!("{name_case} / {operation}");
            let body = assert_refusal(
                &router,
                &case,
                operation,
                &request,
                ENTITY_NOT_FOUND,
                JOB_NOT_FOUND,
            )
            .await;
            // The sentence has no variable part, so the requested name cannot appear in
            // it — the assertion above would already fail, and this one says WHY.
            assert_withholds(&case, &error_message(&body), &name);
            sentences.insert(error_message(&body));
        }
    }
    assert_eq!(
        sentences.len(),
        1,
        "the job-definition miss is one sentence across operations and names, got {sentences:?}"
    );

    // The job the store holds is still served, so the refusals above are about the name.
    let served = ok(&router, "GetJob", &json!({ "JobName": "held-job" })).await;
    assert_eq!(
        served["Job"]["Name"].as_str(),
        Some("held-job"),
        "the job this Region holds must still be served, got {served}"
    );
}

/// The sentence does not depend on HOW the name came to hold no job: a name nothing
/// created, a name whose job a delete removed (the recorded path), and a name only another
/// Region's endpoint holds a job for are all one refusal.
#[tokio::test]
async fn the_sentence_is_the_same_however_the_name_misses() {
    let router = emulator();

    // A name whose job was created and then deleted, in this Region.
    let deleted = "deleted-etl-job";
    ok(&router, "CreateJob", &job_body(deleted)).await;
    let before = ok(&router, "GetJob", &json!({ "JobName": deleted })).await;
    assert_eq!(
        before["Job"]["Name"].as_str(),
        Some(deleted),
        "the created job must be readable before the delete, got {before}"
    );
    ok(&router, "DeleteJob", &json!({ "JobName": deleted })).await;

    // A name only another Region's endpoint holds a job for: a job exists only in the
    // Region it was created in.
    let elsewhere = "other-region-etl-job";
    let (status, created) =
        call_in_region(&router, "eu-west-1", "CreateJob", &job_body(elsewhere)).await;
    assert_eq!(
        status, 200,
        "the other Region's create must answer 200, got {status} with {created}"
    );

    let cases = [
        ("never created", "no-such-etl-job".to_string()),
        ("created then deleted", deleted.to_string()),
        ("held by another Region", elsewhere.to_string()),
    ];
    for (miss_case, name) in cases {
        for (operation, request) in job_name_requests(&name) {
            let case = format!("{miss_case} / {operation}");
            let body = assert_refusal(
                &router,
                &case,
                operation,
                &request,
                ENTITY_NOT_FOUND,
                JOB_NOT_FOUND,
            )
            .await;
            assert_withholds(&case, &error_message(&body), &name);
        }
    }

    // The other Region still serves its own job, so the refusal above is about WHERE the
    // definition lives and not about the definition being gone.
    let (status, served) =
        call_in_region(&router, "eu-west-1", "GetJob", &json!({ "JobName": elsewhere })).await;
    assert_eq!(
        status, 200,
        "the owning Region must still serve its job, got {status} with {served}"
    );
}

/// The same sentence is what a job miss reads as where it is NOT the top-level error: the
/// per-run `ErrorDetail` of a batch stop addressed to a job nothing holds, and the
/// `EntityNotFoundException` of a trigger create whose action would run such a job.
#[tokio::test]
async fn a_job_miss_reported_inside_an_output_carries_the_same_sentence() {
    let router = emulator();
    let absent = "absent-etl-job";

    // `BatchStopJobRun` models no entity-not-found error of its own, so the miss is
    // reported once per named run. Both bounds of `JobRunIds` (length 1..25) are named, so
    // nothing here depends on the batch's size.
    for named in [1usize, 25] {
        let run_ids: Vec<String> = (0..named)
            .map(|nonce| format!("{JOB_RUN_ID_PREFIX}{nonce:064x}"))
            .collect();
        let case = format!("BatchStopJobRun naming {named} runs");
        let answer = ok(
            &router,
            "BatchStopJobRun",
            &json!({ "JobName": absent, "JobRunIds": run_ids }),
        )
        .await;
        let errors = answer
            .get("Errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected Errors, got {answer}"));
        assert_eq!(
            errors.len(),
            named,
            "case {case}: every named run of an absent job fails, got {answer}"
        );
        for entry in errors {
            let detail = entry
                .get("ErrorDetail")
                .unwrap_or_else(|| panic!("case {case}: an error carries a detail, got {entry}"));
            assert_eq!(
                detail.get("__type").and_then(Value::as_str),
                Some(ERROR_DETAIL_TAG),
                "case {case}: a nested ErrorDetail carries its own type tag, got {detail}"
            );
            assert_eq!(
                detail.get("ErrorCode").and_then(Value::as_str),
                Some(ENTITY_NOT_FOUND),
                "case {case}: expected {ENTITY_NOT_FOUND}, got {detail}"
            );
            assert_eq!(
                detail.get("ErrorMessage").and_then(Value::as_str),
                Some(JOB_NOT_FOUND),
                "case {case}: expected ErrorMessage {JOB_NOT_FOUND:?}, got {detail}"
            );
            // The `ErrorMessage` discloses nothing, even though the entry AROUND it names
            // the job the request addressed (`BatchStopJobRunError$JobName`).
            assert_withholds(
                &case,
                detail
                    .get("ErrorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                absent,
            );
        }
    }

    // A trigger whose action would run a job nothing holds resolves the same job name, so
    // it is refused with the same sentence.
    let trigger_request = json!({
        "Name": "trigger-of-an-absent-job",
        "Type": "ON_DEMAND",
        "Actions": [{ "JobName": absent }],
    });
    let body = assert_refusal(
        &router,
        "CreateTrigger naming an absent job",
        "CreateTrigger",
        &trigger_request,
        ENTITY_NOT_FOUND,
        JOB_NOT_FOUND,
    )
    .await;
    assert_withholds(
        "CreateTrigger naming an absent job",
        &error_message(&body),
        absent,
    );

    // The refused create stored nothing: the trigger name is still free, which a second
    // create of the same name over a job that DOES exist shows.
    ok(&router, "CreateJob", &job_body(absent)).await;
    let created = ok(&router, "CreateTrigger", &trigger_request).await;
    assert_eq!(
        created["Name"].as_str(),
        Some("trigger-of-an-absent-job"),
        "the trigger name the refusal left free must be creatable, got {created}"
    );
}

/// A `JobName` the member itself does not admit is refused at the boundary and never
/// reaches the lookup, so the not-found sentence is not what an invalid name answers.
#[tokio::test]
async fn a_name_the_member_rejects_is_not_answered_with_the_miss() {
    let router = emulator();
    // `NameString`'s `length 1..255`: one character past the published maximum, and the
    // empty name one below the published minimum.
    let cases = [
        ("one past the 255 ceiling", "n".repeat(256)),
        ("below the 1 floor", String::new()),
    ];
    for (case, name) in cases {
        let (status, body) = call(&router, "GetJob", &json!({ "JobName": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            !error_code(&body).contains(ENTITY_NOT_FOUND),
            "case {case}: a name the member rejects is an input refusal, got {body}"
        );
        assert_ne!(
            error_message(&body),
            JOB_NOT_FOUND,
            "case {case}: an input refusal must not read as the lookup's miss, got {body}"
        );
    }
}
