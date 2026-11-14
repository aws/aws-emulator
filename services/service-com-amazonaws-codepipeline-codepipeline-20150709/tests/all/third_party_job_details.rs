// SPDX-License-Identifier: CC0-1.0

//! `GetThirdPartyJobDetails`: a partner action's job worker READING one third-party
//! job, over the wire this service actually speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule these tables pin, one row per way it generalises:
//!
//! * a `jobId` no queued job carries is not a job, whatever the id's FORM and
//!   whatever client token accompanies it — refused `JobNotFoundException` (HTTP 400)
//!   with the partner plane's statement for a job the service is not holding, the
//!   same statement the other third-party operations make for that id;
//! * a job reached with a `clientToken` other than the one it was handed out with is
//!   refused `InvalidClientTokenException` (HTTP 400), again worded exactly as the
//!   operations that WRITE the same job word it;
//! * otherwise the job is reported as the store holds it: its id, its current
//!   `nonce`, the action type whose queue it waits in, and the continuation token of
//!   the last result reported for it — and the `nonce` reported is the one
//!   `AcknowledgeThirdPartyJob` then admits, which is the read checked through
//!   another operation;
//! * it REPORTS: whatever a job's `JobStatus` is, reading it leaves the record where
//!   it was, so a second reader sees exactly what the first one saw.
//!
//! The action-execution plane that QUEUES a partner action's jobs is not part of this
//! slice, so — as in `tests/all/third_party_jobs.rs` and
//! `tests/all/acknowledge_third_party_job.rs` — these tests queue their jobs through
//! `ComAmazonawsCodepipelineCodepipeline20150709State` (`enqueue_third_party_job`),
//! the only authoring seam the queue has and the SAME store the handler reads. Every
//! call itself goes through the router.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::{
    ActionTypeId, JOB_STATUS_VALUES,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ComAmazonawsCodepipelineCodepipeline20150709State as CodePipelineState, ThirdPartyJobRecord,
    JOB_STATUS_IN_PROGRESS, JOB_STATUS_QUEUED,
};

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The operation under test.
const GET_DETAILS: &str = "GetThirdPartyJobDetails";

/// The operation that refuses the SAME two conditions while writing the job: the
/// statements the two make for one id have to be the same statement.
const ACKNOWLEDGE: &str = "AcknowledgeThirdPartyJob";

/// The `clientId` half of the pair a job is handed to, in the form
/// `ThirdPartyJob$clientId` models
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
const CLIENT_ID: &str = "7f1e2d3c-4b5a-4978-8a9b-0c1d2e3f4a5b";

/// `ThirdPartyJobId`'s modelled length bounds
/// (`smithy_query query="shape" shape="ThirdPartyJobId"` -> len 1..512).
const JOB_ID_MIN: usize = 1;
const JOB_ID_MAX: usize = 512;

/// `ClientToken`'s modelled length bounds
/// (`smithy_query query="shape" shape="ClientToken"` -> len 1..256).
const CLIENT_TOKEN_MIN: usize = 1;
const CLIENT_TOKEN_MAX: usize = 256;

/// The `ActionOwner` value a partner integration carries
/// (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`, `ThirdParty`,
/// `Custom`): the store queues a third-party job only for a `ThirdParty` action type.
const OWNER_THIRD_PARTY: &str = "ThirdParty";

/// When a seeded job's status last changed: a fixed past instant, so any write the
/// handler made would show as a strictly later stamp.
fn seeded_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)
}

/// The identity of a `ThirdParty` action type whose queue a seeded job waits in.
/// `category` and `provider` vary across the rows, since the identity the details
/// report is the queue the job is filed under rather than anything the request says.
fn partner_action_type(category: &str, provider: &str) -> ActionTypeId {
    ActionTypeId {
        category: category.to_string(),
        owner: OWNER_THIRD_PARTY.to_string(),
        provider: provider.to_string(),
        version: "1".to_string(),
    }
}

/// That identity as a response reports it: the four modelled members plus the
/// awsJson shape discriminator this service writes for `ActionTypeId` wherever it is
/// serialized (`src/service/shape_discriminator.rs`, which lists
/// `ThirdPartyJobData$actionTypeId` among them).
fn action_type_json(id: &ActionTypeId) -> Value {
    json!({
        "__type": "ActionTypeId",
        "category": id.category,
        "owner": id.owner,
        "provider": id.provider,
        "version": id.version,
    })
}

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { engine, router }
    }

    /// Queue a job for `action_type`, as the action-execution plane does when an
    /// execution reaches a partner action: with the `clientToken` and `nonce` a
    /// worker has to present, the `JobStatus` the job is in, and the continuation
    /// token the last result left on it.
    #[allow(clippy::too_many_arguments)]
    fn seed_job(
        &self,
        action_type: &ActionTypeId,
        job_id: &str,
        client_token: &str,
        nonce: &str,
        status: &str,
        continuation_token: Option<&str>,
    ) {
        let mut job = ThirdPartyJobRecord::new(job_id, CLIENT_ID);
        job.client_token = client_token.to_string();
        job.nonce = nonce.to_string();
        job.status = status.to_string();
        job.continuation_token = continuation_token.map(str::to_string);
        job.last_status_change = seeded_at();
        let queued = self
            .engine
            .state::<CodePipelineState>()
            .lock()
            .enqueue_third_party_job(action_type, job);
        assert!(
            queued,
            "seeding job {job_id}: the store must queue a job for a ThirdParty action type"
        );
    }

    /// The stored job, as the next caller would read it. A worker names a job by id
    /// alone, so the search spans every action type's queue.
    fn job(&self, job_id: &str) -> ThirdPartyJobRecord {
        self.engine
            .state::<CodePipelineState>()
            .lock()
            .third_party_jobs
            .values()
            .flatten()
            .find(|job| job.job_id == job_id)
            .cloned()
            .unwrap_or_else(|| panic!("job {job_id} must still be in the store"))
    }

    async fn details(&self, job_id: &str, client_token: &str) -> (u16, Value) {
        self.call(
            GET_DETAILS,
            &json!({"jobId": job_id, "clientToken": client_token}),
        )
        .await
    }

    async fn acknowledge(&self, job_id: &str, nonce: &str, client_token: &str) -> (u16, Value) {
        self.call(
            ACKNOWLEDGE,
            &json!({"jobId": job_id, "nonce": nonce, "clientToken": client_token}),
        )
        .await
    }

    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("host", "codepipeline.us-east-1.amazonaws.com")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|e| panic!("{operation}: response body is not JSON ({e})"))
        };
        (status, parsed)
    }
}

/// The awsJson error code (`__type`) an error body carries.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an error body, got {body}"))
        .to_string()
}

/// The `message` an error body carries, under either casing the error envelopes use.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an error message, got {body}"))
        .to_string()
}

/// The `jobDetails` member a successful response reports.
fn job_details(body: &Value) -> &Value {
    body.get("jobDetails")
        .unwrap_or_else(|| panic!("expected a jobDetails member, got {body}"))
}

/// A string of `len` characters, for exercising a modelled length bound.
fn sized(len: usize, fill: char) -> String {
    std::iter::repeat_n(fill, len).collect()
}

/// `value` with its last character replaced by `replacement`: a DIFFERENT value of
/// the same length, so altering a value that already sits at its modelled maximum
/// does not turn the case into a length violation.
fn altered(value: &str, replacement: char) -> String {
    let mut chars: Vec<char> = value.chars().collect();
    let last = chars.len() - 1;
    assert_ne!(
        chars[last], replacement,
        "the alteration must change {value:?}"
    );
    chars[last] = replacement;
    chars.into_iter().collect()
}

/// One row of the table below: a job to queue, and the case name every assertion of
/// that row names.
struct SeededJob {
    case: &'static str,
    job_id: String,
    client_token: String,
    nonce: String,
    continuation_token: Option<&'static str>,
    action_type: ActionTypeId,
}

/// An id no queued job carries is not a job: `JobNotFoundException` (HTTP 400) with
/// the statement built from the requested id — whatever the id's form and whatever
/// client token accompanies it, because there is no job yet for a token to be checked
/// against. The statement is the one the partner plane's other operations make for the
/// same id, which is checked here against `AcknowledgeThirdPartyJob` rather than
/// assumed.
#[tokio::test]
async fn an_id_no_queued_job_carries_is_not_a_job() {
    let emulator = Emulator::new();
    let stored_id = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    // One real job, so every refusal below is a lookup miss and not an empty store.
    emulator.seed_job(
        &partner_action_type("Build", "PartnerBuild"),
        stored_id,
        "stored-token",
        "stored-nonce",
        JOB_STATUS_QUEUED,
        None,
    );

    // (case, jobId, clientToken) — the rows differ along the id's form and length and
    // along the credentials offered with it.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "uuid-form id never queued",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "does-not-exist",
        ),
        (
            "shortest id the model allows",
            sized(JOB_ID_MIN, 'x'),
            "does-not-exist",
        ),
        (
            "longest id the model allows",
            sized(JOB_ID_MAX, 'j'),
            "does-not-exist",
        ),
        (
            "opaque partner id",
            "partner/job:7f3a".to_string(),
            "does-not-exist",
        ),
        (
            "the stored id differing only in case",
            stored_id.to_uppercase(),
            "stored-token",
        ),
        (
            "a prefix of the stored id",
            stored_id[..stored_id.len() - 1].to_string(),
            "stored-token",
        ),
        (
            "the stored credentials on an absent id",
            "another-job".to_string(),
            "stored-token",
        ),
    ];

    for (case, job_id, client_token) in cases {
        let (status, body) = emulator.details(&job_id, client_token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "JobNotFoundException",
            "case {case:?}: expected JobNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Job with id '{job_id}' does not exist."),
            "case {case:?}: expected the message to name the requested id, got {body}"
        );

        // The sibling operation that WRITES the same job states it identically: one
        // missing job, one statement, whichever operation of the plane answers.
        let (sibling_status, sibling_body) =
            emulator.acknowledge(&job_id, "any-nonce", client_token).await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (
                sibling_status,
                error_code(&sibling_body),
                error_message(&sibling_body)
            ),
            "case {case:?}: the read and {ACKNOWLEDGE} must refuse an absent job alike, \
             got {body} and {sibling_body}"
        );
    }

    let stored = emulator.job(stored_id);
    assert_eq!(
        stored.status, JOB_STATUS_QUEUED,
        "a refused read must leave the queued job where it was, got {:?}",
        stored.status
    );
}

/// A queued job's details are reported to the worker presenting the `clientToken` the
/// job was handed out with, and to nobody else — and what is reported is the STORED
/// job: its id, its current nonce, the action type whose queue it waits in and the
/// continuation token on it. The nonce reported is then the one
/// `AcknowledgeThirdPartyJob` admits, and nothing in `ThirdPartyJobData` is invented
/// for a job the store holds no dispatch capture for.
#[tokio::test]
async fn a_jobs_details_are_reported_to_the_worker_holding_its_client_token() {
    // The rows differ along the id's form and length, the token's length, the
    // nonce's form, whether a continuation token is held, and which action type's
    // queue the job waits in.
    let cases: Vec<SeededJob> = vec![
        SeededJob {
            case: "uuid-form id, shortest token, no continuation token",
            job_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_string(),
            client_token: sized(CLIENT_TOKEN_MIN, 't'),
            nonce: "0".to_string(),
            continuation_token: None,
            action_type: partner_action_type("Build", "PartnerBuild"),
        },
        SeededJob {
            case: "opaque id, token at its maximum, continuation token held",
            job_id: "partner/job:0001".to_string(),
            client_token: sized(CLIENT_TOKEN_MAX, 't'),
            nonce: "nonce-2f8c".to_string(),
            continuation_token: Some("deployment-42"),
            action_type: partner_action_type("Test", "PartnerTest"),
        },
        SeededJob {
            case: "longest id the model allows, mixed-case nonce",
            job_id: sized(JOB_ID_MAX, 'j'),
            client_token: "partner-client-token".to_string(),
            nonce: "NoNcE-7b1".to_string(),
            continuation_token: Some("1"),
            action_type: partner_action_type("Deploy", "Partner_Deploy"),
        },
        SeededJob {
            case: "unicode id, token and nonce",
            job_id: "job-ünïcode".to_string(),
            client_token: "token-ünïcode".to_string(),
            nonce: "nonce-ünïcode".to_string(),
            continuation_token: None,
            action_type: partner_action_type("Invoke", "PartnerInvoke"),
        },
    ];

    for SeededJob {
        case,
        job_id,
        client_token,
        nonce,
        continuation_token,
        action_type,
    } in cases
    {
        let emulator = Emulator::new();
        emulator.seed_job(
            &action_type,
            &job_id,
            &client_token,
            &nonce,
            JOB_STATUS_QUEUED,
            continuation_token,
        );

        // A client token the job was not handed out with reaches no job's details,
        // and the refusal is the one the writing sibling makes for the same pair.
        let wrong_token = altered(&client_token, 'z');
        let (status, body) = emulator.details(&job_id, &wrong_token).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidClientTokenException"),
            "case {case:?}: another client token must not reach the job, got {status} {body}"
        );
        let (sibling_status, sibling_body) =
            emulator.acknowledge(&job_id, &nonce, &wrong_token).await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (
                sibling_status,
                error_code(&sibling_body),
                error_message(&sibling_body)
            ),
            "case {case:?}: the read and {ACKNOWLEDGE} must refuse a foreign token alike, \
             got {body} and {sibling_body}"
        );

        // The job's own token: the details are the stored job.
        let (status, body) = emulator.details(&job_id, &client_token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the job's own token must be admitted, got {status} with {body}"
        );
        let details = job_details(&body);
        assert_eq!(
            details.get("id").and_then(Value::as_str),
            Some(job_id.as_str()),
            "case {case:?}: expected the job's id, got {details}"
        );
        assert_eq!(
            details.get("nonce").and_then(Value::as_str),
            Some(nonce.as_str()),
            "case {case:?}: expected the job's stored nonce, got {details}"
        );
        let data = details
            .get("data")
            .unwrap_or_else(|| panic!("case {case:?}: expected a data member, got {details}"));
        assert_eq!(
            data.get("actionTypeId"),
            Some(&action_type_json(&action_type)),
            "case {case:?}: expected the action type whose queue the job waits in, got {data}"
        );
        assert_eq!(
            data.get("continuationToken").and_then(Value::as_str),
            continuation_token,
            "case {case:?}: expected the continuation token the store holds, got {data}"
        );
        // Nothing the store holds no record of is reported: a job queued with no
        // artifacts carries no artifact credentials and no encryption key.
        for absent in ["artifactCredentials", "encryptionKey", "inputArtifacts"] {
            assert!(
                data.get(absent).is_none_or(Value::is_null),
                "case {case:?}: {absent} must not be invented, got {data}"
            );
        }

        // Reading does not change what a later caller sees: same answer, same record.
        let (repeat_status, repeat_body) = emulator.details(&job_id, &client_token).await;
        assert_eq!(
            (repeat_status, &repeat_body),
            (status, &body),
            "case {case:?}: a second read must report the same job"
        );
        let after_read = emulator.job(&job_id);
        assert_eq!(
            (after_read.status.as_str(), after_read.last_status_change),
            (JOB_STATUS_QUEUED, seeded_at()),
            "case {case:?}: reading a job must leave the record where it was"
        );

        // The nonce this read published is the one that admits a worker to the job,
        // which is the read checked through another operation.
        let reported_nonce = details
            .get("nonce")
            .and_then(Value::as_str)
            .expect("the details report a nonce")
            .to_string();
        let (status, body) = emulator
            .acknowledge(&job_id, &reported_nonce, &client_token)
            .await;
        assert_eq!(
            (
                status,
                body.get("status").and_then(Value::as_str).unwrap_or_default()
            ),
            (200, JOB_STATUS_IN_PROGRESS),
            "case {case:?}: the nonce the details reported must admit the worker, \
             got {status} with {body}"
        );
    }
}

/// Every modelled length bound of this operation's two members, at the bound and one
/// past it: a value inside the bound reaches the lookup (and is answered from the
/// store), one outside it never does — the boundary validator answers
/// `ValidationException`.
#[tokio::test]
async fn a_value_outside_a_published_bound_never_reaches_the_lookup() {
    let emulator = Emulator::new();

    // (case, jobId, clientToken, expected error code)
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "jobId at its minimum",
            sized(JOB_ID_MIN, 'j'),
            "t".to_string(),
            "JobNotFoundException",
        ),
        (
            "jobId at its maximum",
            sized(JOB_ID_MAX, 'j'),
            "t".to_string(),
            "JobNotFoundException",
        ),
        (
            "jobId one below its minimum",
            sized(JOB_ID_MIN - 1, 'j'),
            "t".to_string(),
            "ValidationException",
        ),
        (
            "jobId one past its maximum",
            sized(JOB_ID_MAX + 1, 'j'),
            "t".to_string(),
            "ValidationException",
        ),
        (
            "clientToken at its minimum",
            "absent-job".to_string(),
            sized(CLIENT_TOKEN_MIN, 't'),
            "JobNotFoundException",
        ),
        (
            "clientToken at its maximum",
            "absent-job".to_string(),
            sized(CLIENT_TOKEN_MAX, 't'),
            "JobNotFoundException",
        ),
        (
            "clientToken one below its minimum",
            "absent-job".to_string(),
            sized(CLIENT_TOKEN_MIN - 1, 't'),
            "ValidationException",
        ),
        (
            "clientToken one past its maximum",
            "absent-job".to_string(),
            sized(CLIENT_TOKEN_MAX + 1, 't'),
            "ValidationException",
        ),
    ];

    for (case, job_id, client_token, expected) in cases {
        let (status, body) = emulator.details(&job_id, &client_token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// The read reports a job in any modelled `JobStatus` (`JOB_STATUS_VALUES`, the
/// catalog the model publishes) and moves none of them: a worker may read the job it
/// holds whether it is still waiting, being worked on, or finished, and the record is
/// the same after the read as before it.
#[tokio::test]
async fn a_job_is_reported_in_every_modelled_status_and_moved_in_none() {
    for status_value in JOB_STATUS_VALUES {
        let emulator = Emulator::new();
        let action_type = partner_action_type("Build", "PartnerBuild");
        let job_id = format!("job-{status_value}");
        emulator.seed_job(
            &action_type,
            &job_id,
            "the-token",
            "the-nonce",
            status_value,
            None,
        );

        let (status, body) = emulator.details(&job_id, "the-token").await;
        assert_eq!(
            status, 200,
            "case {status_value:?}: expected HTTP 200, got {status} with {body}"
        );
        let details = job_details(&body);
        assert_eq!(
            (
                details.get("id").and_then(Value::as_str),
                details.get("nonce").and_then(Value::as_str)
            ),
            (Some(job_id.as_str()), Some("the-nonce")),
            "case {status_value:?}: expected the stored job, got {details}"
        );

        let after_read = emulator.job(&job_id);
        assert_eq!(
            (after_read.status.as_str(), after_read.last_status_change),
            (*status_value, seeded_at()),
            "case {status_value:?}: reading a job must not move it"
        );
    }
}
