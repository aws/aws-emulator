// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `AssumeQueueRoleForWorker`.
//!
//! The rules under test:
//!
//! * a worker is vended a queue's role only WHILE the scheduler holds it on work of that
//!   queue — before it has been given a session of the queue, on a queue it holds no session
//!   of, and once the session it held has ended, the call is the modelled 409
//!   `ConflictException`;
//! * what the call vends is a SESSION of the queue's own `roleArn`: the access key id it
//!   answers with names a session of that role in the account's session registry — the very
//!   record every service resolves a temporary credential's principal through — whose ARN is
//!   the assumed-role ARN of that role, so a call made with the credential speaks for the
//!   queue role and not for the account;
//! * a queue created with no role has none to vend, and the response's optional
//!   `credentials` member is then absent rather than the call refused;
//! * the worker is reached only through its own farm and fleet and the queue through the
//!   farm, so each level the request names and this emulator does not hold is a 404 naming
//!   that level.
//!
//! Every call goes through the same front door a client uses — the shared [`Emulator`]
//! harness of this test target — so routing, deserialization, constraint validation, the
//! handler and serialization all run. The minted session is read back out of the account's
//! session registry in the emulator's own engine, which is the store a later signed request
//! is resolved against.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::fleet::create_fleet;
use crate::worker::create_worker;

/// How long a test waits for a condition the clock owes before it gives up.
///
/// Generous on purpose: every wait here polls for the CONDITION and returns as soon as it
/// holds, so this bounds only a run in which the emulator never gets there.
const CONDITION_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll loop waits between looks.
const POLL_PAUSE: Duration = Duration::from_millis(20);

/// How many schedule polls one worker's run of a job is given before the test gives up.
const MAX_POLLS: usize = 32;

fn queue_roles_path(farm_id: &str, fleet_id: &str, worker_id: &str, queue_id: &str) -> String {
    format!(
        "/2023-10-12/farms/{farm_id}/fleets/{fleet_id}/workers/{worker_id}/queue-roles\
         ?queueId={queue_id}"
    )
}

/// An Open Job Description job template declaring one step with one task, which is the
/// smallest job a worker can be handed a session for.
fn one_task_template(name: &str) -> String {
    json!({
        "specificationVersion": "jobtemplate-2023-09",
        "name": name,
        "steps": [{
            "name": "Render",
            "script": {"actions": {"onRun": {"command": "echo", "args": ["{{Task.File.Run}}"]}}},
        }],
    })
    .to_string()
}

/// One farm, one fleet and one worker of it: the caller every row below drives.
struct Worker {
    emulator: Emulator,
    farm_id: String,
    fleet_id: String,
    worker_id: String,
}

impl Worker {
    /// Build the farm, fleet and worker every row needs.
    async fn new(name: &str) -> Self {
        let emulator = Emulator::new();
        let farm_id = emulator.farm(name).await;
        let fleet_id = create_fleet(&emulator, &farm_id, name).await;
        let (status, created) = create_worker(&emulator, &farm_id, &fleet_id, json!({}), None).await;
        assert_eq!(
            status, 200,
            "CreateWorker {name}: expected 200, got {status} with {created}"
        );
        let worker_id = string_at(&created, "workerId");
        Self {
            emulator,
            farm_id,
            fleet_id,
            worker_id,
        }
    }

    /// Create a queue carrying `role_arn` and put its work on this worker's fleet.
    async fn queue(&self, display_name: &str, role_arn: Option<&str>) -> String {
        let mut body = Map::new();
        body.insert("displayName".to_string(), json!(display_name));
        if let Some(role_arn) = role_arn {
            body.insert("roleArn".to_string(), json!(role_arn));
        }
        let (status, created) = self
            .emulator
            .post(
                &format!("/2023-10-12/farms/{}/queues", self.farm_id),
                Value::Object(body),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateQueue {display_name}: expected 200, got {status} with {created}"
        );
        let queue_id = string_at(&created, "queueId");
        let (status, association) = self
            .emulator
            .call(
                "PUT",
                &format!("/2023-10-12/farms/{}/queue-fleet-associations", self.farm_id),
                Some(json!({"queueId": queue_id, "fleetId": self.fleet_id})),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateQueueFleetAssociation {queue_id}: expected 200, got {status} with \
             {association}"
        );
        queue_id
    }

    /// `AssumeQueueRoleForWorker` for `queue_id`.
    async fn assume(&self, queue_id: &str) -> (u16, Value) {
        self.emulator
            .get(&queue_roles_path(
                &self.farm_id,
                &self.fleet_id,
                &self.worker_id,
                queue_id,
            ))
            .await
    }

    /// One `UpdateWorkerSchedule` poll carrying `reported`, which must succeed.
    async fn schedule(&self, case: &str, reported: Map<String, Value>) -> Value {
        let body = if reported.is_empty() {
            json!({})
        } else {
            json!({"updatedSessionActions": Value::Object(reported)})
        };
        let (status, response) = self
            .emulator
            .call(
                "PATCH",
                &format!(
                    "/2023-10-12/farms/{}/fleets/{}/workers/{}/schedule",
                    self.farm_id, self.fleet_id, self.worker_id
                ),
                Some(body),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateWorkerSchedule to answer 200, got {status} with \
             {response}"
        );
        response
    }

    /// Create a job under `queue_id` and poll the schedule until the worker holds a session
    /// of it.
    ///
    /// A created job reaches the status that lets it be scheduled on the clock, so this waits
    /// for the CONDITION — a session in the schedule — rather than for a span.
    async fn start_work(&self, case: &str, queue_id: &str) {
        let (status, created) = self
            .emulator
            .post(
                &format!(
                    "/2023-10-12/farms/{}/queues/{queue_id}/jobs",
                    self.farm_id
                ),
                json!({
                    "template": one_task_template(case),
                    "templateType": "JSON",
                    "priority": 50,
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateJob expected 201, got {status} with {created}"
        );
        let deadline = Instant::now() + CONDITION_DEADLINE;
        loop {
            let response = self.schedule(case, Map::new()).await;
            if !assigned_sessions(&response).is_empty() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the worker to be handed a session within the \
                 deadline, last schedule {response}"
            );
            tokio::time::sleep(POLL_PAUSE).await;
        }
    }

    /// Report every action the worker is handed as succeeded until the schedule hands it
    /// nothing, which is what ends the session it held.
    async fn finish_work(&self, case: &str) {
        for _ in 0..MAX_POLLS {
            let response = self.schedule(case, Map::new()).await;
            let mut reported = Map::new();
            for (_, session) in assigned_sessions(&response) {
                for action in session
                    .get("sessionActions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    reported.insert(
                        string_at(&action, "sessionActionId"),
                        json!({"completedStatus": "SUCCEEDED", "processExitCode": 0}),
                    );
                }
            }
            if reported.is_empty() {
                return;
            }
            self.schedule(case, reported).await;
        }
        panic!("case {case:?}: the worker was still holding work after {MAX_POLLS} polls");
    }
}

/// The sessions one schedule response assigned, as `(sessionId, session)` pairs.
fn assigned_sessions(response: &Value) -> Vec<(String, Value)> {
    response
        .get("assignedSessions")
        .and_then(Value::as_object)
        .map(|sessions| {
            sessions
                .iter()
                .map(|(id, session)| (id.clone(), session.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The `credentials` member of a successful answer, held to carrying every member the
/// modelled `AwsCredentials` declares.
fn credentials(case: &str, body: &Value) -> Value {
    let credentials = body
        .get("credentials")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected credentials in {body}"));
    for member in ["accessKeyId", "secretAccessKey", "sessionToken", "expiration"] {
        assert!(
            credentials.get(member).is_some(),
            "case {case:?}: expected {member} in the vended credentials {credentials}"
        );
    }
    credentials
}

/// The sentence this service refuses a worker that is working on nothing of the queue with,
/// with the worker and the queue it asked about filled in.
fn not_working_message(worker_id: &str, queue_id: &str) -> String {
    format!("Worker {worker_id} is not working on queue {queue_id}.")
}

/// Hold one answer to being the modelled 409 for a worker that is not working on
/// `queue_id`: the status, the error code and the sentence, which is a requirement of the
/// error class and not of one call.
fn assert_not_working(case: &str, worker: &Worker, queue_id: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (409, "ConflictException"),
        "case {case:?}: expected a 409 ConflictException, got {status} with {body}"
    );
    let expected = not_working_message(&worker.worker_id, queue_id);
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {case:?}: expected the refusal to read {expected:?}, got {body}"
    );
}

/// The identity the account's session registry resolves `access_key_id` to — the record a
/// later request signed with the credential is resolved against.
fn session_identity(case: &str, emulator: &Emulator, access_key_id: &str) -> (String, Option<String>) {
    let registry = sts_state::service_state(emulator.engine())
        .expect("the account's session registry lives in this emulator's engine");
    let held = registry.lock();
    let session = held
        .session(access_key_id)
        .unwrap_or_else(|| panic!("case {case:?}: no session is held for {access_key_id}"));
    (session.identity.arn.clone(), session.role_arn.clone())
}

#[tokio::test]
async fn the_queue_role_is_vended_only_while_the_worker_works_on_that_queue() {
    // The rule: a worker holds a queue's role only while the scheduler has it working on
    // that queue. The rows differ along WHEN the call is made relative to the session the
    // worker holds, and on WHICH queue it asks about.
    let worker = Worker::new("queue-roles").await;
    let role_arn = "arn:aws:iam::111122223333:role/DeadlineQueue";
    let queue_id = worker.queue("worked", Some(role_arn)).await;
    let other_queue_id = worker.queue("untouched", Some(role_arn)).await;

    let (status, early) = worker.assume(&queue_id).await;
    assert_not_working("before any session", &worker, &queue_id, status, &early);

    worker.start_work("working", &queue_id).await;

    let (status, vended) = worker.assume(&queue_id).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the queue role to be vended, got {status} with {vended}",
        "while working on the queue"
    );
    let vended = credentials("while working on the queue", &vended);

    // The worker is working on ONE queue, so the queue it holds no session of is refused
    // even though the same role would be vended for it.
    let (status, other) = worker.assume(&other_queue_id).await;
    assert_not_working("another queue", &worker, &other_queue_id, status, &other);

    worker.finish_work("finished").await;

    let (status, late) = worker.assume(&queue_id).await;
    assert_not_working("after the session ended", &worker, &queue_id, status, &late);

    // The one success above vended real credential material, not an echo of the request.
    let access_key_id = string_at(&vended, "accessKeyId");
    assert_ne!(
        access_key_id,
        string_at(&vended, "secretAccessKey"),
        "expected the vended secret to differ from the access key id, got {vended}"
    );
}

#[tokio::test]
async fn a_vended_credential_is_a_session_of_the_queues_own_role() {
    // The rule: the credentials are a session of the queue's `roleArn`, so the access key id
    // they carry names a session of THAT role whose ARN is the assumed-role ARN of it. The
    // rows differ along the form of the role ARN the queue carries — the account it names,
    // whether the role sits under an IAM path, and the symbols its name uses — each of which
    // the modelled `IamRoleArn` pattern admits (`smithy_query(query="shape",
    // shape="IamRoleArn")`: `^arn:(aws[a-zA-Z-]*):iam::\d{12}:role(/[!-.0-~]+)*/[\w+=,.@-]+$`).
    let cases = [
        (
            "a role at the account root",
            "arn:aws:iam::111122223333:role/DeadlineQueue",
            "DeadlineQueue",
        ),
        (
            "a role under a path",
            "arn:aws:iam::444455556666:role/service-role/deadline/QueueRole",
            "QueueRole",
        ),
        (
            "a role name using the symbols the pattern admits",
            "arn:aws-us-gov:iam::777788889999:role/queue+role=1.2@render-eu",
            "queue+role=1.2@render-eu",
        ),
    ];

    for (case, role_arn, role_name) in cases {
        let worker = Worker::new(case).await;
        let queue_id = worker.queue(case, Some(role_arn)).await;
        worker.start_work(case, &queue_id).await;

        let (status, body) = worker.assume(&queue_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the queue role to be vended, got {status} with {body}"
        );
        let vended = credentials(case, &body);
        let access_key_id = string_at(&vended, "accessKeyId");
        let (session_arn, session_role) = session_identity(case, &worker.emulator, &access_key_id);
        let expected_arn = format!(
            "arn:aws:sts::{}:assumed-role/{role_name}/{}",
            role_arn
                .split(':')
                .nth(4)
                .expect("an IAM role ARN carries an account field"),
            worker.worker_id
        );
        assert_eq!(
            session_arn, expected_arn,
            "case {case:?}: expected the credential to speak for {expected_arn}, got \
             {session_arn}"
        );
        assert_eq!(
            session_role.as_deref(),
            Some(role_arn),
            "case {case:?}: expected the session to be recorded for {role_arn}, got \
             {session_role:?}"
        );

        // Two calls are two sessions: a vend mints its own credential rather than handing
        // back one it minted before.
        let (status, again) = worker.assume(&queue_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a second vend to succeed, got {status} with {again}"
        );
        let second = string_at(&credentials(case, &again), "accessKeyId");
        assert_ne!(
            second, access_key_id,
            "case {case:?}: expected a credential of its own per vend, got {access_key_id} \
             twice"
        );
    }
}

#[tokio::test]
async fn a_queue_with_no_role_has_no_credentials_to_vend() {
    // The rule: the credentials are a session of the queue's role, so a queue created with
    // no role has nothing to vend — and `credentials` is the one member of this response
    // that the model leaves optional, so the answer carries none rather than refusing.
    let worker = Worker::new("no-role").await;
    let queue_id = worker.queue("roleless", None).await;
    worker.start_work("no role", &queue_id).await;

    let (status, body) = worker.assume(&queue_id).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200 for a queue with no role, got {status} with {body}",
        "a queue with no role"
    );
    assert_eq!(
        body.get("credentials"),
        None,
        "case {:?}: expected no credentials for a queue with no role, got {body}",
        "a queue with no role"
    );
}

#[tokio::test]
async fn a_level_this_emulator_does_not_hold_is_a_404_naming_that_level() {
    // The rule: the worker is reached through its own farm and fleet and the queue through
    // the farm, so each level the request names and this emulator does not hold is a 404
    // naming that level. The rows differ along WHICH level is absent.
    let worker = Worker::new("absent").await;
    let queue_id = worker.queue("present", None).await;
    let absent_farm = absent_id("farm", &"a".repeat(32));
    let absent_fleet = absent_id("fleet", &"b".repeat(32));
    let absent_worker = absent_id("worker", &"c".repeat(32));
    let absent_queue = absent_id("queue", &"d".repeat(32));
    // Each row carries the id it expects the refusal to name, because the ids of the
    // resources this suite CREATED are minted per run: which level a row made absent is the
    // row's own fact, not something to recover from the text of a generated id.
    let cases = [
        (
            "an absent farm",
            absent_farm.clone(),
            worker.fleet_id.clone(),
            worker.worker_id.clone(),
            queue_id.clone(),
            absent_farm.clone(),
        ),
        (
            "an absent fleet",
            worker.farm_id.clone(),
            absent_fleet.clone(),
            worker.worker_id.clone(),
            queue_id.clone(),
            absent_fleet.clone(),
        ),
        (
            "an absent worker",
            worker.farm_id.clone(),
            worker.fleet_id.clone(),
            absent_worker.clone(),
            queue_id.clone(),
            absent_worker.clone(),
        ),
        (
            "an absent queue",
            worker.farm_id.clone(),
            worker.fleet_id.clone(),
            worker.worker_id.clone(),
            absent_queue.clone(),
            absent_queue.clone(),
        ),
    ];

    for (case, farm_id, fleet_id, worker_id, queue, absent) in cases {
        let (status, body) = worker
            .emulator
            .get(&queue_roles_path(&farm_id, &fleet_id, &worker_id, &queue))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 naming the absent level, got {status} with {body}"
        );
        assert_eq!(
            body.get("resourceId").and_then(Value::as_str),
            Some(absent.as_str()),
            "case {case:?}: expected the refusal to name {absent}, got {body}"
        );
    }
}
