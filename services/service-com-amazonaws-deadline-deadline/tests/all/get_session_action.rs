// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetSessionAction`: reading back one action of one session.
//!
//! The rules under test:
//!
//! * a read of a session action reports the action the scheduler stored — the same
//!   definition, in whichever of the four `SessionActionDefinition` members the work the
//!   scheduler handed the worker belongs to, under the id of the session that holds it — and
//!   nothing the worker has not reported yet;
//! * what the worker reports through `UpdateWorkerSchedule` is what the read then reports:
//!   the status it gave the action (every `CompletedStatus` value, and `RUNNING` for a report
//!   that carries none), its exit code, its progress and the manifests of its task run;
//! * the read is a read: two reads of one action agree, and the action is reached only
//!   through its own farm, queue and job, so each level the request names and this emulator
//!   does not hold is a 404 naming that level.
//!
//! The harness (`Emulator`) and the farm, fleet and worker helpers are the ones the other
//! suites drive the service with, so these tests go through routing, deserialization,
//! constraint validation, the handler and serialization, exactly as the router does in
//! production.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

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

/// How many times a test polls the schedule before it declares the worker stuck.
const MAX_POLLS: usize = 16;

/// The `specificationVersion` an Open Job Description job template carries.
const JOB_TEMPLATE_VERSION: &str = "jobtemplate-2023-09";

/// The `specificationVersion` an Open Job Description environment template carries.
const ENVIRONMENT_TEMPLATE_VERSION: &str = "environment-2023-09";

fn jobs_path(farm_id: &str, queue_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs")
}

fn schedule_path(farm_id: &str, fleet_id: &str, worker_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_id}/workers/{worker_id}/schedule")
}

/// `GET /2023-10-12/farms/{farmId}/queues/{queueId}/jobs/{jobId}/session-actions/{sessionActionId}`,
/// from `smithy_query(query="operation", operation="GetSessionAction")`.
fn session_action_path(
    farm_id: &str,
    queue_id: &str,
    job_id: &str,
    session_action_id: &str,
) -> String {
    format!(
        "{}/{job_id}/session-actions/{session_action_id}",
        jobs_path(farm_id, queue_id)
    )
}

/// A job template declaring one step whose parameter space holds `tasks` points.
fn template(name: &str, tasks: i64) -> String {
    json!({
        "specificationVersion": JOB_TEMPLATE_VERSION,
        "name": name,
        "steps": [{
            "name": "Render",
            "parameterSpace": {"taskParameterDefinitions": [{
                "name": "Frame",
                "type": "INT",
                "range": (1..=tasks).collect::<Vec<i64>>(),
            }]},
            "script": {"actions": {"onRun": {"command": "echo", "args": ["{{Task.Param.Frame}}"]}}},
        }],
    })
    .to_string()
}

/// An environment template of its own, as a queue environment carries it.
fn environment_template(name: &str) -> String {
    json!({
        "specificationVersion": ENVIRONMENT_TEMPLATE_VERSION,
        "environment": {"name": name, "variables": {"SEAT": "1"}},
    })
    .to_string()
}

/// The attachments a job is created with: one manifest naming where its inputs live.
///
/// A job that carries attachments has them synchronised onto the host before anything runs,
/// which is the `syncInputJobAttachments` action of its session.
fn attachments() -> Value {
    json!({
        "manifests": [{
            "rootPath": "/mnt/shared/scene",
            "rootPathFormat": "posix",
            "inputManifestPath": "manifests/scene.json",
            "inputManifestHash": "0123456789abcdef",
            "outputRelativeDirectories": ["output"],
        }],
        "fileSystem": "COPIED",
    })
}

/// One worker of one fleet, on a queue whose work that fleet is scheduling.
struct Fixture {
    emulator: Emulator,
    farm_id: String,
    queue_id: String,
    fleet_id: String,
    worker_id: String,
}

impl Fixture {
    /// Build the farm, queue, fleet, worker and association every test below needs.
    async fn new(name: &str) -> Self {
        let emulator = Emulator::new();
        let farm_id = emulator.farm(name).await;
        let queue_id = emulator.queue(&farm_id, name).await;
        let fleet_id = create_fleet(&emulator, &farm_id, name).await;
        let (status, created) = create_worker(&emulator, &farm_id, &fleet_id, json!({}), None).await;
        assert_eq!(
            status, 200,
            "CreateWorker {name}: expected 200, got {status} with {created}"
        );
        let worker_id = string_at(&created, "workerId");
        let fixture = Self {
            emulator,
            farm_id,
            queue_id,
            fleet_id,
            worker_id,
        };
        let (status, body) = fixture
            .emulator
            .call(
                "PUT",
                &format!(
                    "/2023-10-12/farms/{}/queue-fleet-associations",
                    fixture.farm_id
                ),
                Some(json!({"queueId": fixture.queue_id, "fleetId": fixture.fleet_id})),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateQueueFleetAssociation: expected 200, got {status} with {body}"
        );
        fixture
    }

    /// Create one queue environment under this fixture's queue, so a session of its jobs
    /// enters an environment before it runs a task.
    async fn queue_environment(&self, name: &str) {
        let (status, created) = self
            .emulator
            .post(
                &format!(
                    "/2023-10-12/farms/{}/queues/{}/environments",
                    self.farm_id, self.queue_id
                ),
                json!({
                    "priority": 1,
                    "templateType": "JSON",
                    "template": environment_template(name),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateQueueEnvironment {name}: expected 200, got {status} with {created}"
        );
    }

    /// Create a job under this fixture's queue from `body` and return its id.
    async fn job(&self, body: Value) -> String {
        let (status, created) = self
            .emulator
            .post(&jobs_path(&self.farm_id, &self.queue_id), body)
            .await;
        assert_eq!(
            status, 201,
            "CreateJob: expected 201, got {status} with {created}"
        );
        string_at(&created, "jobId")
    }

    /// One `UpdateWorkerSchedule` call carrying `reported`, which must succeed.
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
                &schedule_path(&self.farm_id, &self.fleet_id, &self.worker_id),
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

    /// `GetSessionAction` under this fixture's farm, queue and `job_id`.
    async fn read_action(&self, job_id: &str, session_action_id: &str) -> (u16, Value) {
        self.emulator
            .get(&session_action_path(
                &self.farm_id,
                &self.queue_id,
                job_id,
                session_action_id,
            ))
            .await
    }

    /// `GetSessionAction`, which must succeed.
    async fn read_action_ok(&self, case: &str, job_id: &str, session_action_id: &str) -> Value {
        let (status, action) = self.read_action(job_id, session_action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSessionAction {session_action_id} to answer 200, got \
             {status} with {action}"
        );
        action
    }

    /// Poll the schedule until it hands the worker at least one action, and answer them.
    ///
    /// A created job reaches the status its tasks can be scheduled in by the clock, so this
    /// waits for the CONDITION rather than for a span.
    async fn handed_until(&self, case: &str) -> Vec<Handed> {
        let deadline = Instant::now() + CONDITION_DEADLINE;
        loop {
            let response = self.schedule(case, Map::new()).await;
            let handed = handed_actions(case, &response);
            if !handed.is_empty() {
                return handed;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the schedule to hand the worker an action within the \
                 deadline, last read {response}"
            );
            tokio::time::sleep(POLL_PAUSE).await;
        }
    }
}

/// One action the schedule handed the worker: the session it belongs to, its id, and the
/// definition the schedule carried for it.
#[derive(Debug, Clone)]
struct Handed {
    session_id: String,
    session_action_id: String,
    definition: Value,
}

impl Handed {
    /// The `SessionActionDefinition` member this action's work belongs to.
    fn kind(&self) -> String {
        self.definition
            .as_object()
            .and_then(|members| members.keys().next().cloned())
            .unwrap_or_else(|| panic!("expected a definition member in {}", self.definition))
    }
}

/// Every action of every session an `UpdateWorkerSchedule` response assigned, by session id
/// then in the order the session holds them.
fn handed_actions(case: &str, response: &Value) -> Vec<Handed> {
    let sessions = response
        .get("assignedSessions")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: expected an assignedSessions map in {response}"));
    let mut ids: Vec<&String> = sessions.keys().collect();
    ids.sort();
    let mut handed = Vec::new();
    for session_id in ids {
        let session = &sessions[session_id];
        let actions = session
            .get("sessionActions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected sessionActions in {session}"));
        for action in actions {
            handed.push(Handed {
                session_id: session_id.clone(),
                session_action_id: string_at(action, "sessionActionId"),
                definition: action
                    .get("definition")
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a definition in {action}")),
            });
        }
    }
    handed
}

/// Which members of a read action are absent, as a sorted list, out of those a worker fills
/// in only by reporting.
fn absent_reported_members(action: &Value) -> Vec<&'static str> {
    [
        "startedAt",
        "endedAt",
        "workerUpdatedAt",
        "processExitCode",
        "progressMessage",
        "progressPercent",
        "manifests",
        "acquiredLimits",
    ]
    .into_iter()
    .filter(|member| action.get(member).is_none())
    .collect()
}

#[tokio::test]
async fn a_session_action_reports_the_definition_the_scheduler_handed_its_worker() {
    // The rule: a read of an action reports the work the scheduler stored for it, in the
    // `SessionActionDefinition` member that work belongs to. The cases are the four members
    // one session's actions span — a job with attachments synchronises them, enters the
    // environments it runs under, runs the tasks of its step, and exits those environments —
    // and the rows differ along which member each action carries.
    let case = "every definition member";
    let fixture = Fixture::new("definitions").await;
    fixture.queue_environment("Seat").await;
    let job_id = fixture
        .job(json!({
            "template": template("Definitions", 2),
            "templateType": "JSON",
            "priority": 50,
            "attachments": attachments(),
        }))
        .await;

    let mut kinds: BTreeSet<String> = BTreeSet::new();
    let mut task_runs = 0;
    let mut handed = fixture.handed_until(case).await;
    for poll in 0..MAX_POLLS {
        let mut reported = Map::new();
        for action in &handed {
            let kind = action.kind();
            kinds.insert(kind.clone());
            if kind == "taskRun" {
                task_runs += 1;
            }

            // Before the worker has said anything, the action carries what the scheduler
            // stored and nothing else.
            let read = fixture
                .read_action_ok(case, &job_id, &action.session_action_id)
                .await;
            assert_eq!(
                (
                    string_at(&read, "sessionActionId"),
                    string_at(&read, "sessionId"),
                    string_at(&read, "status"),
                    read.get("definition").cloned().unwrap_or(Value::Null),
                ),
                (
                    action.session_action_id.clone(),
                    action.session_id.clone(),
                    "ASSIGNED".to_string(),
                    action.definition.clone(),
                ),
                "case {case:?}: expected the {kind} action handed in poll {poll} to read back as \
                 the scheduler handed it, got {read}"
            );
            assert_eq!(
                absent_reported_members(&read).len(),
                8,
                "case {case:?}: expected the unreported {kind} action to carry nothing the worker \
                 has not reported, got {read}"
            );

            // A read changes nothing a later caller sees.
            let again = fixture
                .read_action_ok(case, &job_id, &action.session_action_id)
                .await;
            assert_eq!(
                read, again,
                "case {case:?}: two reads of the {kind} action disagreed: {read} then {again}"
            );

            reported.insert(
                action.session_action_id.clone(),
                json!({"completedStatus": "SUCCEEDED", "processExitCode": 0}),
            );
        }

        let response = fixture.schedule(case, reported).await;
        handed = handed_actions(case, &response);
        if handed.is_empty() {
            break;
        }
        assert!(
            poll + 1 < MAX_POLLS,
            "case {case:?}: the worker was still being handed work after {MAX_POLLS} polls"
        );
    }

    assert_eq!(
        kinds,
        [
            "envEnter".to_string(),
            "envExit".to_string(),
            "syncInputJobAttachments".to_string(),
            "taskRun".to_string(),
        ]
        .into_iter()
        .collect::<BTreeSet<String>>(),
        "case {case:?}: expected the session to span every definition member, got {kinds:?}"
    );
    assert_eq!(
        task_runs, 2,
        "case {case:?}: expected one task-run action per task of the step's parameter space, got \
         {task_runs}"
    );
}

#[tokio::test]
async fn a_session_action_reports_the_outcome_its_worker_gave_it() {
    // The rule: what the worker reports for an action is what a read of it reports — every
    // `CompletedStatus` value is the status the action then carries, and a report that
    // carries none is progress on an action still running. The rows differ along the status
    // reported (`smithy_query(query="shape", shape="CompletedStatus")`: SUCCEEDED, FAILED,
    // INTERRUPTED, CANCELED, NEVER_ATTEMPTED) and along whether the report completes the
    // action at all.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a successful run",
            json!({
                "completedStatus": "SUCCEEDED",
                "processExitCode": 0,
                "progressPercent": 100.0,
                "progressMessage": "rendered",
                "manifests": [{
                    "outputManifestPath": "output/frame.exr",
                    "outputManifestHash": "fedcba9876543210",
                }],
            }),
            Some("SUCCEEDED"),
        ),
        (
            "a run that failed",
            json!({"completedStatus": "FAILED", "processExitCode": 137}),
            Some("FAILED"),
        ),
        (
            "a run the worker was interrupted part way through",
            json!({"completedStatus": "INTERRUPTED", "progressPercent": 40.5}),
            Some("INTERRUPTED"),
        ),
        (
            "a cancelled run",
            json!({"completedStatus": "CANCELED"}),
            Some("CANCELED"),
        ),
        (
            "a run that never started",
            json!({"completedStatus": "NEVER_ATTEMPTED"}),
            Some("NEVER_ATTEMPTED"),
        ),
        (
            "progress on a run still going",
            json!({"progressPercent": 12.5, "progressMessage": "loading the scene"}),
            None,
        ),
    ];

    for (case, report, completed) in cases {
        let fixture = Fixture::new("outcomes").await;
        let job_id = fixture
            .job(json!({
                "template": template("Outcomes", 1),
                "templateType": "JSON",
                "priority": 50,
            }))
            .await;
        let handed = fixture.handed_until(case).await;
        let action = handed
            .iter()
            .find(|handed| handed.kind() == "taskRun")
            .unwrap_or_else(|| panic!("case {case:?}: expected a task-run action in {handed:?}"));

        let mut reported = Map::new();
        reported.insert(action.session_action_id.clone(), report.clone());
        fixture.schedule(case, reported).await;

        let read = fixture
            .read_action_ok(case, &job_id, &action.session_action_id)
            .await;
        // A report without a completed status is the worker working on the action, so the
        // status is `RUNNING` and the action has not ended.
        let expected_status = completed.unwrap_or("RUNNING");
        assert_eq!(
            string_at(&read, "status"),
            expected_status,
            "case {case:?}: expected the reported status, got {read}"
        );
        assert_eq!(
            read.get("endedAt").is_some(),
            completed.is_some(),
            "case {case:?}: expected endedAt only for an action the worker finished, got {read}"
        );
        assert!(
            read.get("startedAt").is_some() && read.get("workerUpdatedAt").is_some(),
            "case {case:?}: expected the action the worker reported on to carry when it started \
             and when that report arrived, got {read}"
        );
        for member in ["processExitCode", "progressPercent", "progressMessage"] {
            assert_eq!(
                read.get(member),
                report.get(member),
                "case {case:?}: expected {member} to read back as reported, got {read}"
            );
        }
        // The manifests of a task run are the ones the worker reported, and an action it
        // reported none for carries none at all.
        assert_eq!(
            read.get("manifests"),
            report.get("manifests"),
            "case {case:?}: expected the reported manifests, got {read}"
        );
    }
}

#[tokio::test]
async fn a_session_action_is_reached_only_through_its_own_farm_queue_and_job() {
    let case = "reached through its own path";
    let fixture = Fixture::new("not-found").await;
    let job_id = fixture
        .job(json!({
            "template": template("Reached", 1),
            "templateType": "JSON",
            "priority": 50,
        }))
        .await;
    // A second job of the same queue: its path reaches none of the first job's actions.
    let other_job_id = fixture
        .job(json!({
            "template": template("Other", 1),
            "templateType": "JSON",
            "priority": 1,
        }))
        .await;
    let handed = fixture.handed_until(case).await;
    let action_id = handed
        .iter()
        .find(|handed| handed.kind() == "taskRun")
        .map(|handed| handed.session_action_id.clone())
        .unwrap_or_else(|| panic!("case {case:?}: expected a task-run action in {handed:?}"));

    // A session action id this emulator minted for nothing, satisfying the modeled
    // `^sessionaction-[0-9a-f]{32}-(0|([1-9][0-9]{0,9}))$` pattern so the request passes the
    // boundary validator and the handler is what decides the outcome.
    let absent_action = format!("sessionaction-{}-7", "0".repeat(32));
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "an unheld farm",
            session_action_path(
                &absent_id("farm", "000000000000000000000000000000b1"),
                &fixture.queue_id,
                &job_id,
                &action_id,
            ),
            "farm",
            absent_id("farm", "000000000000000000000000000000b1"),
        ),
        (
            "an unheld queue",
            session_action_path(
                &fixture.farm_id,
                &absent_id("queue", "000000000000000000000000000000b2"),
                &job_id,
                &action_id,
            ),
            "queue",
            absent_id("queue", "000000000000000000000000000000b2"),
        ),
        (
            "an unheld job",
            session_action_path(
                &fixture.farm_id,
                &fixture.queue_id,
                &absent_id("job", "000000000000000000000000000000b3"),
                &action_id,
            ),
            "job",
            absent_id("job", "000000000000000000000000000000b3"),
        ),
        (
            "an action of another job of the same queue",
            session_action_path(
                &fixture.farm_id,
                &fixture.queue_id,
                &other_job_id,
                &action_id,
            ),
            "session-action",
            action_id.clone(),
        ),
        (
            "an action this emulator never handed out",
            session_action_path(
                &fixture.farm_id,
                &fixture.queue_id,
                &job_id,
                &absent_action,
            ),
            "session-action",
            absent_action.clone(),
        ),
    ];

    for (row, path, resource_type, resource_id) in cases {
        let (status, body) = fixture.emulator.get(&path).await;
        assert_eq!(
            (
                status,
                error_code(&body),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
            ),
            (
                404,
                "ResourceNotFoundException".to_string(),
                resource_type.to_string(),
                resource_id.clone(),
            ),
            "case {row:?}: expected GetSessionAction to answer a 404 naming what is missing, got \
             {status} with {body}"
        );
    }

    // The action its own job's path names is still there: a refused read of it changed
    // nothing.
    let read = fixture.read_action_ok(case, &job_id, &action_id).await;
    assert_eq!(
        string_at(&read, "sessionActionId"),
        action_id,
        "case {case:?}: expected the action to still read back under its own path, got {read}"
    );
}
