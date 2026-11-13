// SPDX-License-Identifier: CC0-1.0

//! `SubmitTaskStateChange` — the ECS AGENT's report that a TASK changed state — driven end
//! to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test:
//!
//! * The report resolves what it names before it records anything: the cluster that hosts
//!   the task, then the task in that cluster, then every nested reference it carries. Each
//!   condition is refused with the MODELLED error its contract declares and the fixed
//!   sentence of the condition — the agent plane answers a missing cluster with
//!   `ClientException`, a reference that resolves to nothing with
//!   `InvalidParameterException` — and a report carrying one unresolved reference records
//!   none of its entries.
//! * The agent RETRIES, so what is stored is the furthest stage the task has been reported
//!   at: a report of the stage a live task is already at is acknowledged and changes nothing,
//!   and one naming an EARLIER stage is refused — the task lifecycle runs in one direction
//!   and the server enforces it. Once the task has reached the terminal `STOPPED` no report
//!   is taken at all. The container entries keep the retry handling: a stale container entry
//!   is acknowledged and changes nothing.
//! * Everything a resolved report observed — the image-pull and execution instants, each
//!   container's outcome, each attachment's state, each managed agent's status — is
//!   recorded, and the caller-sized members of a container entry are bounded at the size
//!   their own docs state.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeTasks`, so a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, count, list, ok, refusal, service,
    string, ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster every row below acts in.
const CLUSTER: &str = "taskreportplane";

/// A second cluster, so a task placed ELSEWHERE is one that resolves to nothing here.
const OTHER_CLUSTER: &str = "elsewherereport";

/// The `awsvpc` family the suite runs, with TWO containers so a report that names one
/// names one of several, and so the container nobody reported on can be asserted
/// untouched. `awsvpc` is what allocates the task the interface an attachment entry names.
const FAMILY: &str = "taskreportfamily";

/// The two containers that family declares.
const FIRST_CONTAINER: &str = "app";
const SECOND_CONTAINER: &str = "sidecar";

/// The image the containers start from; irrelevant to these rules, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// What the agent plane answers a report it has recorded with.
const ACK: &str = "ACK";

/// The `lastStatus` a run of [`FAMILY`] leaves its task at: an `awsvpc` task is allocated
/// its own elastic network interface, and Amazon ECS has to attach that interface before
/// the task can be launched, which is what `PROVISIONING` reports.
const STARTING_STAGE: &str = "PROVISIONING";

/// The sentence a report naming a cluster that resolves to nothing is refused with.
const CLUSTER_NOT_FOUND: &str = "Cluster not found.";

/// The sentence a report naming a task that resolves to nothing is refused with.
const TASK_NOT_FOUND: &str = "The referenced task was not found";

/// The sentence a report naming a container the task does not hold is refused with.
const CONTAINER_NOT_FOUND: &str = "The referenced container was not found";

/// The one managed agent `ManagedAgentName` names (`smithy_query(query="shape",
/// shape="ManagedAgentName")` -> ExecuteCommandAgent).
const MANAGED_AGENT: &str = "ExecuteCommandAgent";

/// A router holding two clusters with capacity and one `awsvpc` revision of [`FAMILY`].
async fn fixture() -> MultiServiceRouter {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "fixture").await;
    cluster_with_capacity(&router, OTHER_CLUSTER, "fixture").await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "containerDefinitions": [
                container(FIRST_CONTAINER, IMAGE),
                container(SECOND_CONTAINER, IMAGE),
            ],
        }),
        "fixture",
    )
    .await;
    router
}

/// Run ONE standalone task of [`FAMILY`] in `cluster` and return the task it reports.
async fn run_task(router: &MultiServiceRouter, cluster: &str, case: &str) -> Value {
    let body = ok(
        router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": FAMILY,
            "count": 1,
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(tasks.len(), 1, "case {case:?}: expected one task, got {body}");
    tasks[0].clone()
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the
/// call that changed it.
async fn describe_task(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one task back, got {body}"))
}

/// One container of a described task, by name.
fn container_of(task: &Value, name: &str, case: &str) -> Value {
    task["containers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task's containers, got {task}"))
        .iter()
        .find(|container| container["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case {case:?}: expected a container {name:?}, got {task}"))
        .clone()
}

/// The one interface an `awsvpc` task is allocated, off a described task.
fn only_attachment(task: &Value, case: &str) -> Value {
    let attachments = list(task, "attachments", case);
    assert_eq!(
        attachments.len(),
        1,
        "case {case:?}: expected the one interface an awsvpc task is allocated, got {task}"
    );
    attachments[0].clone()
}

/// A fresh fixture holding one running-ish task: the router, the task's ARN, and the id of
/// the interface it was allocated.
async fn fixture_with_a_task(case: &str) -> (MultiServiceRouter, String, String) {
    let router = fixture().await;
    let task = run_task(&router, CLUSTER, case).await;
    let task_arn = string(&task, "taskArn", case);
    let attachment_id = string(&only_attachment(&task, case), "id", case);
    (router, task_arn, attachment_id)
}

/// An attachment ARN of `id`: `arn:aws:ecs:<region>:<account>:attachment/<id>`.
fn attachment_arn(id: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:attachment/{id}")
}

#[tokio::test]
async fn a_task_report_resolves_the_cluster_then_the_task_before_it_records_anything() {
    let case = "references";
    let (router, task_arn, _) = fixture_with_a_task(case).await;
    let task_id = task_arn
        .rsplit('/')
        .next()
        .expect("a task ARN ends in its id")
        .to_string();
    let elsewhere = string(&run_task(&router, OTHER_CLUSTER, case).await, "taskArn", case);

    // The cluster is resolved FIRST, so these rows carry a task reference that WOULD
    // resolve in the live cluster. They differ along the form the `cluster` member takes
    // and why it resolves to nothing: a name no cluster carries, the longest such name,
    // the ARN form of one, and the omitted member — which names the default cluster this
    // fixture never created.
    let missing_clusters: [(&str, Option<String>); 4] = [
        ("short name", Some("ghost-cluster".to_string())),
        ("longest name", Some("g".repeat(255))),
        (
            "full ARN",
            Some(format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/ghost-by-arn"
            )),
        ),
        ("no cluster named at all", None),
    ];
    for (form, cluster) in &missing_clusters {
        let case = format!("SubmitTaskStateChange with a {form} cluster");
        let mut request = json!({ "task": task_id.clone(), "status": "RUNNING" });
        if let Some(cluster) = cluster {
            request["cluster"] = json!(cluster);
        }
        let (code, message) = refusal(&router, "SubmitTaskStateChange", request, &case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClientException", CLUSTER_NOT_FOUND),
            "case {case:?}: expected the cluster-not-found refusal, got {code} with \
             {message:?}"
        );
    }

    // With the cluster resolved, the `task` member is resolved against THAT cluster. The
    // rows differ along the form of the reference and why it resolves to nothing.
    let missing_tasks: [(&str, Option<String>); 5] = [
        ("bare id of nothing", Some("0".repeat(32))),
        (
            "uuid of nothing",
            Some("11111111-2222-3333-4444-555555555555".to_string()),
        ),
        (
            "full ARN of nothing",
            Some(format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{CLUSTER}/{}",
                "e".repeat(32)
            )),
        ),
        ("a task of another cluster, by ARN", Some(elsewhere)),
        ("no task named at all", None),
    ];
    for (form, task) in &missing_tasks {
        let case = format!("SubmitTaskStateChange with a {form}");
        let mut request = json!({ "cluster": CLUSTER, "status": "RUNNING" });
        if let Some(task) = task {
            request["task"] = json!(task);
        }
        let (code, message) = refusal(&router, "SubmitTaskStateChange", request, &case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", TASK_NOT_FOUND),
            "case {case:?}: expected the task-not-found refusal, got {code} with {message:?}"
        );
    }

    // None of those refusals touched the task that does exist.
    let read = describe_task(&router, CLUSTER, &task_arn, case).await;
    assert_eq!(
        read["lastStatus"].as_str(),
        Some(STARTING_STAGE),
        "case {case:?}: a refused report must change nothing, got {read}"
    );
}

#[tokio::test]
async fn a_report_resolves_every_nested_reference_before_it_records_any_of_them() {
    let case = "nested references";
    let (router, task_arn, attachment_id) = fixture_with_a_task(case).await;

    // Each row carries ONE unresolvable nested reference BEHIND an entry that does
    // resolve, so the refusal is asserted to be about the report as a whole: the resolved
    // entry is not recorded either. The rows span the three nested lists the report
    // carries and, for the attachment, the two forms an `attachmentArn` takes.
    let rows: [(&str, Value, &str, String); 4] = [
        (
            "a container entry naming no container of the task",
            json!({
                "containers": [
                    { "containerName": FIRST_CONTAINER, "status": "RUNNING" },
                    { "containerName": "ghost", "status": "RUNNING" },
                ],
            }),
            "InvalidParameterException",
            CONTAINER_NOT_FOUND.to_string(),
        ),
        (
            "a managed-agent entry naming no container of the task",
            json!({
                "containers": [{ "containerName": FIRST_CONTAINER, "status": "RUNNING" }],
                "managedAgents": [{
                    "containerName": "ghost",
                    "managedAgentName": MANAGED_AGENT,
                    "status": "RUNNING",
                }],
            }),
            "InvalidParameterException",
            CONTAINER_NOT_FOUND.to_string(),
        ),
        (
            "an attachment entry whose ARN names no interface of the task",
            json!({
                "containers": [{ "containerName": FIRST_CONTAINER, "status": "RUNNING" }],
                "attachments": [
                    { "attachmentArn": attachment_arn(&attachment_id), "status": "ATTACHED" },
                    {
                        "attachmentArn": attachment_arn("deadbeef-0000-0000-0000-000000000000"),
                        "status": "ATTACHED",
                    },
                ],
            }),
            "InvalidParameterException",
            "Attachment with id deadbeef-0000-0000-0000-000000000000 does not exist"
                .to_string(),
        ),
        (
            "an attachment entry whose bare id names no interface of the task",
            json!({
                "containers": [{ "containerName": FIRST_CONTAINER, "status": "RUNNING" }],
                "attachments": [{
                    "attachmentArn": "99999999-8888-7777-6666-555555555555",
                    "status": "ATTACHED",
                }],
            }),
            "InvalidParameterException",
            "Attachment with id 99999999-8888-7777-6666-555555555555 does not exist"
                .to_string(),
        ),
    ];

    for (row, members, expected_code, expected_message) in rows {
        let mut request = json!({
            "cluster": CLUSTER,
            "task": task_arn.clone(),
            "status": "RUNNING",
        });
        for (member, value) in members.as_object().expect("the row carries an object") {
            request[member] = value.clone();
        }
        let (code, message) = refusal(&router, "SubmitTaskStateChange", request, row).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (expected_code, expected_message.as_str()),
            "case {row:?}: expected the unresolved-reference refusal, got {code} with \
             {message:?}"
        );

        // Neither the task-level status nor the entry that DID resolve was recorded.
        let read = describe_task(&router, CLUSTER, &task_arn, row).await;
        assert_eq!(
            read["lastStatus"].as_str(),
            Some(STARTING_STAGE),
            "case {row:?}: a refused report must leave the task where it was, got {read}"
        );
        assert_eq!(
            container_of(&read, FIRST_CONTAINER, row)["lastStatus"].as_str(),
            Some(STARTING_STAGE),
            "case {row:?}: a refused report must record none of its entries, got {read}"
        );
        assert_eq!(
            only_attachment(&read, row)["status"].as_str(),
            Some("PRECREATED"),
            "case {row:?}: a refused report must record none of its entries, got {read}"
        );
    }
}

#[tokio::test]
async fn the_reported_stage_is_recorded_with_the_instants_that_stage_carries() {
    // The rows span the task lifecycle this service stores (`Task$lastStatus`, "Task
    // Lifecycle" in the ECS developer guide: PROVISIONING, PENDING, RUNNING,
    // DEACTIVATING, STOPPED) plus a stage that lifecycle does not name, which cannot be
    // ranked and so is stored as the agent reported it. Every row reports against a FRESH
    // task that a run has just left at [`STARTING_STAGE`], so the expected stage is a
    // function of the report alone: a stage past that one is recorded, the stage it is
    // already at is not. Each row carries the stage it reports, the stage the task must
    // then hold, whether the report MOVED it, and the instant that stage carries.
    let rows: [(&str, &str, &str, bool, Option<&str>); 6] = [
        (
            "an onward RUNNING",
            "RUNNING",
            "RUNNING",
            true,
            Some("startedAt"),
        ),
        (
            "an onward DEACTIVATING",
            "DEACTIVATING",
            "DEACTIVATING",
            true,
            Some("stoppingAt"),
        ),
        (
            "an onward STOPPED",
            "STOPPED",
            "STOPPED",
            true,
            Some("stoppedAt"),
        ),
        (
            "the stage it is already at",
            STARTING_STAGE,
            STARTING_STAGE,
            false,
            None,
        ),
        ("an onward PENDING", "PENDING", "PENDING", true, None),
        (
            "a stage the lifecycle does not name",
            "ACTIVATING",
            "ACTIVATING",
            true,
            None,
        ),
    ];

    for (case, reported, expected, moved, instant) in rows {
        let (router, task_arn, _) = fixture_with_a_task(case).await;
        let before = count(
            &describe_task(&router, CLUSTER, &task_arn, case).await,
            "version",
            case,
        );

        let acknowledged = ok(
            &router,
            "SubmitTaskStateChange",
            json!({
                "cluster": CLUSTER,
                "task": task_arn.clone(),
                "status": reported,
                "reason": "the agent said so",
            }),
            case,
        )
        .await;
        assert_eq!(
            acknowledged["acknowledgment"].as_str(),
            Some(ACK),
            "case {case:?}: the agent is told the report was taken, got {acknowledged}"
        );

        let read = describe_task(&router, CLUSTER, &task_arn, case).await;
        assert_eq!(
            read["lastStatus"].as_str(),
            Some(expected),
            "case {case:?}: expected the task to hold {expected:?}, got {read}"
        );
        let after = count(&read, "version", case);
        if moved {
            assert!(
                after > before,
                "case {case:?}: a report that moved the task bumps its version, got \
                 {before} then {after}"
            );
        } else {
            assert_eq!(
                after, before,
                "case {case:?}: a report that moved nothing leaves the version alone, got \
                 {before} then {after}"
            );
        }
        if let Some(member) = instant {
            assert!(
                read[member].is_number(),
                "case {case:?}: the stage carries its own instant {member}, got {read}"
            );
        }
        if reported == "STOPPED" {
            assert_eq!(
                read["stoppedReason"].as_str(),
                Some("the agent said so"),
                "case {case:?}: the reason the report carried is why the task stopped, got \
                 {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_task_report_never_runs_backwards_however_often_it_is_retried() {
    let case = "retries";
    let (router, task_arn, _) = fixture_with_a_task(case).await;

    ok(
        &router,
        "SubmitTaskStateChange",
        json!({ "cluster": CLUSTER, "task": task_arn.clone(), "status": "STOPPED" }),
        case,
    )
    .await;
    let stopped = describe_task(&router, CLUSTER, &task_arn, case).await;
    let version = count(&stopped, "version", case);

    // A replay of the stage a task is AT is the agent's retry: acknowledged, changing
    // nothing — but a task that has reached the TERMINAL stage makes no further transition,
    // so even a replay of that stage is refused.
    let row = "a replayed STOPPED";
    let (code, message) = refusal(
        &router,
        "SubmitTaskStateChange",
        json!({ "cluster": CLUSTER, "task": task_arn.clone(), "status": "STOPPED" }),
        row,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            "The status STOPPED is not a valid status for a task in the STOPPED status."
        ),
        "case {row:?}: expected the terminal-state refusal, got {code} with {message:?}"
    );

    // The rows walk the lifecycle BACKWARDS from where the task now is: each names a
    // transition the task cannot have made, so each is REFUSED and changes nothing.
    for (row, status) in [
        ("a stale DEACTIVATING", "DEACTIVATING"),
        ("a stale RUNNING", "RUNNING"),
        ("a stale PENDING", "PENDING"),
        ("a stale PROVISIONING", "PROVISIONING"),
    ] {
        let (code, message) = refusal(
            &router,
            "SubmitTaskStateChange",
            json!({ "cluster": CLUSTER, "task": task_arn.clone(), "status": status }),
            row,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                format!(
                    "The status {status} is not a valid status for a task in the STOPPED \
                     status."
                )
                .as_str()
            ),
            "case {row:?}: expected the out-of-order refusal, got {code} with {message:?}"
        );

        let read = describe_task(&router, CLUSTER, &task_arn, row).await;
        assert_eq!(
            (read["lastStatus"].as_str(), count(&read, "version", row)),
            (Some("STOPPED"), version),
            "case {row:?}: a refused report must change nothing, got {read}"
        );
    }
}

#[tokio::test]
async fn the_instants_only_the_agent_observes_are_stored_as_reported() {
    // The three timestamp members the report carries, each read back off the task member
    // it feeds. The rows differ along WHICH members one report carries — one at a time and
    // all three together — so each is asserted to be stored independently of the others.
    let rows: [(&str, &str, i64); 3] = [
        ("the image-pull start", "pullStartedAt", 1_700_000_001),
        ("the image-pull end", "pullStoppedAt", 1_700_000_002),
        ("the execution end", "executionStoppedAt", 1_700_000_003),
    ];

    for (case, member, instant) in rows {
        let (router, task_arn, _) = fixture_with_a_task(case).await;
        let mut request = json!({ "cluster": CLUSTER, "task": task_arn.clone() });
        request[member] = json!(instant);
        let acknowledged = ok(&router, "SubmitTaskStateChange", request, case).await;
        assert_eq!(
            acknowledged["acknowledgment"].as_str(),
            Some(ACK),
            "case {case:?}: the report was taken, got {acknowledged}"
        );

        let read = describe_task(&router, CLUSTER, &task_arn, case).await;
        assert_eq!(
            read[member].as_i64(),
            Some(instant),
            "case {case:?}: expected the task to hold the instant the report carried, got \
             {read}"
        );
    }

    // All three in ONE report, and then the same report again: the second changes nothing,
    // because an instant restating the one already stored is not a change.
    let case = "every instant at once";
    let (router, task_arn, _) = fixture_with_a_task(case).await;
    let request = json!({
        "cluster": CLUSTER,
        "task": task_arn.clone(),
        "pullStartedAt": 1_700_000_010_i64,
        "pullStoppedAt": 1_700_000_020_i64,
        "executionStoppedAt": 1_700_000_030_i64,
    });
    ok(&router, "SubmitTaskStateChange", request.clone(), case).await;
    let read = describe_task(&router, CLUSTER, &task_arn, case).await;
    assert_eq!(
        (
            read["pullStartedAt"].as_i64(),
            read["pullStoppedAt"].as_i64(),
            read["executionStoppedAt"].as_i64(),
        ),
        (
            Some(1_700_000_010),
            Some(1_700_000_020),
            Some(1_700_000_030)
        ),
        "case {case:?}: expected all three instants the report carried, got {read}"
    );
    let version = count(&read, "version", case);
    ok(&router, "SubmitTaskStateChange", request, case).await;
    let again = describe_task(&router, CLUSTER, &task_arn, case).await;
    assert_eq!(
        count(&again, "version", case),
        version,
        "case {case:?}: a report restating what is stored changes nothing, got {again}"
    );
}

#[tokio::test]
async fn a_resolved_report_records_each_container_attachment_and_managed_agent_it_names() {
    // The rows span the stage a container entry reports and the status a managed-agent entry
    // reports (`ManagedAgentStateChange$status` is an unconstrained string, so the agent's
    // own word is stored as given). The attachment entry reports the one state the agent
    // plane accepts (`ATTACHED`); the states it refuses are the subject of
    // [`crate::attachment_state_reports`]. Each row reports against a fresh task.
    let rows: [(&str, &str, &str, &str); 3] = [
        ("a task coming up", "RUNNING", "ATTACHED", "PENDING"),
        ("a task that is up", "RUNNING", "ATTACHED", "RUNNING"),
        ("a task going away", "STOPPED", "ATTACHED", "STOPPED"),
    ];

    for (case, container_status, attachment_status, agent_status) in rows {
        let (router, task_arn, attachment_id) = fixture_with_a_task(case).await;

        let acknowledged = ok(
            &router,
            "SubmitTaskStateChange",
            json!({
                "cluster": CLUSTER,
                "task": task_arn.clone(),
                "status": container_status,
                "containers": [{
                    "containerName": FIRST_CONTAINER,
                    "status": container_status,
                    "runtimeId": "0123456789abcdef0123456789abcdef",
                    "imageDigest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "exitCode": 0,
                    "reason": "what the agent saw",
                    "networkBindings": [{
                        "bindIP": "0.0.0.0",
                        "containerPort": 80,
                        "hostPort": 32768,
                        "protocol": "tcp",
                    }],
                }],
                "attachments": [{
                    "attachmentArn": attachment_arn(&attachment_id),
                    "status": attachment_status,
                }],
                "managedAgents": [{
                    "containerName": FIRST_CONTAINER,
                    "managedAgentName": MANAGED_AGENT,
                    "status": agent_status,
                    "reason": "what the agent saw of its agent",
                }],
            }),
            case,
        )
        .await;
        assert_eq!(
            acknowledged["acknowledgment"].as_str(),
            Some(ACK),
            "case {case:?}: the report was taken, got {acknowledged}"
        );

        let read = describe_task(&router, CLUSTER, &task_arn, case).await;
        let reported = container_of(&read, FIRST_CONTAINER, case);
        assert_eq!(
            (
                reported["lastStatus"].as_str(),
                reported["runtimeId"].as_str(),
                reported["imageDigest"].as_str(),
                reported["exitCode"].as_i64(),
                reported["reason"].as_str(),
            ),
            (
                Some(container_status),
                Some("0123456789abcdef0123456789abcdef"),
                Some(
                    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                ),
                Some(0),
                Some("what the agent saw"),
            ),
            "case {case:?}: expected the outcome the entry carried, got {reported}"
        );
        let bindings = list(&reported, "networkBindings", case);
        assert_eq!(
            bindings.len(),
            1,
            "case {case:?}: expected the binding the entry carried, got {reported}"
        );
        assert_eq!(
            (
                bindings[0]["containerPort"].as_i64(),
                bindings[0]["hostPort"].as_i64(),
            ),
            (Some(80), Some(32768)),
            "case {case:?}: expected the ports the entry carried, got {reported}"
        );

        let agents = list(&reported, "managedAgents", case);
        assert_eq!(
            agents.len(),
            1,
            "case {case:?}: expected the one managed agent the entry named, got {reported}"
        );
        assert_eq!(
            (
                agents[0]["name"].as_str(),
                agents[0]["lastStatus"].as_str(),
                agents[0]["reason"].as_str(),
            ),
            (
                Some(MANAGED_AGENT),
                Some(agent_status),
                Some("what the agent saw of its agent"),
            ),
            "case {case:?}: expected the managed agent the entry reported, got {reported}"
        );
        // `ManagedAgent$lastStartedAt` is "when the managed agent was last started", so an
        // agent reported running carries one and one that never ran does not.
        assert_eq!(
            agents[0]["lastStartedAt"].is_number(),
            agent_status == "RUNNING",
            "case {case:?}: expected a start instant only for an agent reported running, \
             got {reported}"
        );

        assert_eq!(
            only_attachment(&read, case)["status"].as_str(),
            Some(attachment_status),
            "case {case:?}: expected the state the attachment entry named, got {read}"
        );

        // The container the report never named holds nothing the report carried.
        let sibling = container_of(&read, SECOND_CONTAINER, case);
        assert_eq!(
            (
                sibling["runtimeId"].as_str(),
                sibling["exitCode"].as_i64(),
                sibling["reason"].as_str(),
                list(&sibling, "managedAgents", case).len(),
            ),
            (None, None, None, 0),
            "case {case:?}: the container nobody reported on holds no outcome, got \
             {sibling}"
        );
    }
}

#[tokio::test]
async fn a_container_entry_is_bounded_at_what_its_members_document() {
    let case = "bounds";
    let (router, task_arn, _) = fixture_with_a_task(case).await;

    // `NetworkBinding$containerPortRange`: "You can specify a maximum of 100 port ranges
    // per container"; `Container$reason`: "A short (255 max characters) human-readable
    // string". Each published bound is accepted on itself and refused one past it, so the
    // rows carry the bound and the value one beyond it. `None` is a row that must be
    // taken.
    let binding = |port: i64| json!({ "containerPort": port, "hostPort": port, "protocol": "tcp" });
    let bindings = |count: i64| -> Value {
        (0..count).map(|index| binding(1024 + index)).collect()
    };
    let rows: [(&str, Value, Option<String>); 4] = [
        (
            "the network-binding bound itself",
            json!([{ "containerName": FIRST_CONTAINER, "networkBindings": bindings(100) }]),
            None,
        ),
        (
            "one network binding past the bound",
            json!([{ "containerName": FIRST_CONTAINER, "networkBindings": bindings(101) }]),
            Some("network bindings can have at most 100 items.".to_string()),
        ),
        (
            "the reason bound itself",
            json!([{ "containerName": FIRST_CONTAINER, "reason": "r".repeat(255) }]),
            None,
        ),
        (
            "one reason character past the bound",
            json!([{ "containerName": FIRST_CONTAINER, "reason": "r".repeat(256) }]),
            Some("reason can have at most 255 characters.".to_string()),
        ),
    ];

    for (row, containers, expected) in rows {
        let request = json!({
            "cluster": CLUSTER,
            "task": task_arn.clone(),
            "containers": containers,
        });
        match expected {
            None => {
                let acknowledged = ok(&router, "SubmitTaskStateChange", request, row).await;
                assert_eq!(
                    acknowledged["acknowledgment"].as_str(),
                    Some(ACK),
                    "case {row:?}: the bound itself must be accepted, got {acknowledged}"
                );
            }
            Some(sentence) => {
                let (code, message) =
                    refusal(&router, "SubmitTaskStateChange", request, row).await;
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    ("InvalidParameterException", sentence.as_str()),
                    "case {row:?}: expected the bound refusal, got {code} with {message:?}"
                );
            }
        }
    }

    // What the accepted rows left is what the task holds: the 100 bindings and the
    // 255-character reason, neither truncated nor replaced by a refused row.
    let stored = container_of(
        &describe_task(&router, CLUSTER, &task_arn, case).await,
        FIRST_CONTAINER,
        case,
    );
    assert_eq!(
        (
            list(&stored, "networkBindings", case).len(),
            stored["reason"].as_str().map(str::len),
        ),
        (100, Some(255)),
        "case {case:?}: expected what the accepted reports left, got {stored}"
    );
}
