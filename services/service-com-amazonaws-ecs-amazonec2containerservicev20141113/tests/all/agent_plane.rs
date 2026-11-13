// SPDX-License-Identifier: CC0-1.0

//! The operations the ECS AGENT and an exec/protection client call, driven end to end over
//! the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test:
//!
//! * Each of these operations resolves the resources its request names — the cluster, then
//!   the task or container instance in it, then the container of that task — and refuses
//!   with the MODELLED error ITS own contract declares for that condition, carrying the
//!   fixed sentence of the condition rather than any value the request held. The tables
//!   below vary the reference along its form (bare id, uuid, full ARN, a resource of
//!   another cluster) and the operation it is sent to, so the sentence is asserted to be a
//!   constant of the condition and not of the request.
//! * `UpdateContainerAgent` addresses the agent by the ID of the instance it runs on, so
//!   the FORM of that id is checked before anything is resolved, and the update it accepts
//!   is owed by the CLOCK: the instance reports the first stage now and the terminal one
//!   once the span has passed.
//! * `DiscoverPollEndpoint` answers the agent with the endpoints of the REGION the request
//!   was signed for, once the cluster and (when the request names one) the container
//!   instance it is called for resolve to records this store holds.
//! * `SubmitContainerStateChange` is a report the agent RETRIES, so what is stored is the
//!   furthest stage the container has been reported at: a report of a stage it has already
//!   passed is acknowledged and changes nothing.
//! * Scale-in protection is a property of a task the service scheduler owns, lasts the
//!   period the request asks for (or the documented default), and lapses on the clock.
//!
//! Nothing here reaches into the state struct, so a response that is not computed from
//! persisted state cannot pass; every write is read back through an implemented describe.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, identity_document, identity_document_signature, list, ok, refusal,
    service, string, ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The cluster every row below acts in.
const CLUSTER: &str = "agentplane";

/// A second cluster, so a reference to a resource registered ELSEWHERE is a reference that
/// resolves to nothing in `CLUSTER`.
const OTHER_CLUSTER: &str = "elsewhereplane";

/// The task-definition family the suite runs, with TWO containers so a request that names
/// a container names one of several.
const FAMILY: &str = "agentfamily";

/// The two containers that family declares.
const FIRST_CONTAINER: &str = "app";
const SECOND_CONTAINER: &str = "sidecar";

/// The agent version this emulator publishes as the update target
/// (`state::LATEST_CONTAINER_AGENT_VERSION`), and an older one an instance may report.
const LATEST_AGENT_VERSION: &str = "1.89.3";
const OLDER_AGENT_VERSION: &str = "1.70.0";

/// The sentence a `containerInstance` that is not a container-instance id is refused with.
const INSTANCE_ID_PATTERN_MESSAGE: &str = "ContainerInstanceId must match ^[a-f0-9-]{1,36}$";

/// A router holding one cluster with capacity, a second cluster, and one task-definition
/// revision of [`FAMILY`].
async fn fixture() -> MultiServiceRouter {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "fixture").await;
    cluster_with_capacity(&router, OTHER_CLUSTER, "fixture").await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "containerDefinitions": [
                container(FIRST_CONTAINER, "public.ecr.aws/nginx/nginx:latest"),
                container(SECOND_CONTAINER, "public.ecr.aws/nginx/nginx:latest"),
            ],
        }),
        "fixture",
    )
    .await;
    router
}

/// Run ONE standalone task of [`FAMILY`] in `cluster` and return its ARN.
async fn run_task(router: &MultiServiceRouter, cluster: &str, case: &str) -> String {
    let body = ok(
        router,
        "RunTask",
        json!({ "cluster": cluster, "taskDefinition": FAMILY, "count": 1 }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(tasks.len(), 1, "case {case:?}: expected one task, got {body}");
    string(&tasks[0], "taskArn", case)
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

/// Read ONE container instance back through `DescribeContainerInstances`.
async fn describe_instance(
    router: &MultiServiceRouter,
    cluster: &str,
    instance_arn: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeContainerInstances",
        json!({ "cluster": cluster, "containerInstances": [instance_arn] }),
        case,
    )
    .await;
    list(&body, "containerInstances", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one instance back, got {body}"))
}

/// Register a container instance into `cluster` reporting `agent_version` (absent when
/// `None`) and return its ARN.
async fn register_agent(
    router: &MultiServiceRouter,
    cluster: &str,
    ec2_instance_id: &str,
    agent_version: Option<&str>,
    case: &str,
) -> String {
    let mut request = json!({
        "cluster": cluster,
        "instanceIdentityDocument": identity_document(ec2_instance_id),
        "instanceIdentityDocumentSignature": identity_document_signature(),
    });
    if let Some(version) = agent_version {
        request["versionInfo"] = json!({ "agentVersion": version, "agentHash": "0a1b2c3" });
    }
    let body = ok(router, "RegisterContainerInstance", request, case).await;
    string(&body["containerInstance"], "containerInstanceArn", case)
}

/// The id segment of a container-instance ARN.
fn instance_id_of(instance_arn: &str) -> String {
    instance_arn
        .rsplit('/')
        .next()
        .expect("a container-instance ARN ends in its id")
        .to_string()
}

#[tokio::test]
async fn discover_poll_endpoint_answers_the_region_endpoints_for_a_registered_instance() {
    let router = fixture().await;
    let instance_arn = register_agent(&router, CLUSTER, "i-0dpe0dpe0dpe0dpe0", None, "fixture")
        .await;
    let instance_id = instance_id_of(&instance_arn);
    let elsewhere =
        register_agent(&router, OTHER_CLUSTER, "i-0e1e2e3e4e5e6e7e8", None, "fixture").await;

    // The endpoints are hosts of this service in the Region the request was signed for.
    let expected = [
        ("endpoint", format!("https://ecs-a-1.{SIGNED_REGION}.amazonaws.com/")),
        (
            "telemetryEndpoint",
            format!("https://ecs-t-1.{SIGNED_REGION}.amazonaws.com/"),
        ),
        (
            "serviceConnectEndpoint",
            format!("https://ecs-sc.{SIGNED_REGION}.amazonaws.com/"),
        ),
    ];

    // The rows span the ways an agent may ask: naming its instance by id, by full ARN, and
    // naming no instance at all; with the cluster by name, by ARN, and left to the default.
    let accepted = [
        (
            "the instance by id",
            json!({ "cluster": CLUSTER, "containerInstance": instance_id.clone() }),
        ),
        (
            "the instance by ARN",
            json!({ "cluster": CLUSTER, "containerInstance": instance_arn.clone() }),
        ),
        (
            "the cluster by ARN",
            json!({
                "cluster": format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{CLUSTER}"),
                "containerInstance": instance_id.clone(),
            }),
        ),
        ("no instance named", json!({ "cluster": CLUSTER })),
    ];
    for (case, request) in accepted {
        let body = ok(&router, "DiscoverPollEndpoint", request, case).await;
        for (member, value) in &expected {
            assert_eq!(
                string(&body, member, case),
                *value,
                "case {case:?}: expected {member} to be the Region's own endpoint, got {body}"
            );
        }
    }

    // A `containerInstance` that is not a container-instance id at all names no instance in
    // any cluster, so the FORM is what the refusal states — whatever else the request
    // names.
    let malformed = [
        ("a value with spaces", "not a valid identifier".to_string()),
        ("a value outside the alphabet", "ZZZZ".to_string()),
        ("punctuation", "@@@".to_string()),
        ("one character past the length", "a".repeat(37)),
        (
            "an ARN whose id segment is no id",
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/{CLUSTER}/not an id"
            ),
        ),
    ];
    for (case, reference) in &malformed {
        let (code, message) = refusal(
            &router,
            "DiscoverPollEndpoint",
            json!({ "cluster": CLUSTER, "containerInstance": reference }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClientException", INSTANCE_ID_PATTERN_MESSAGE),
            "case {case:?}: expected the id-form refusal for {reference:?}, got {code} with \
             {message:?}"
        );
    }

    // A well-formed id that names no instance OF THAT CLUSTER — never registered, or
    // registered elsewhere — is the not-found refusal, naming the instance referred to.
    let unknown = [
        (
            "an id nothing registered",
            "0".repeat(32),
            "0".repeat(32),
        ),
        (
            "an instance of another cluster",
            instance_id_of(&elsewhere),
            instance_id_of(&elsewhere),
        ),
    ];
    for (case, reference, named) in &unknown {
        let (code, message) = refusal(
            &router,
            "DiscoverPollEndpoint",
            json!({ "cluster": CLUSTER, "containerInstance": reference }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "ClientException",
                format!("Referenced container instance {named} not found.").as_str()
            ),
            "case {case:?}: expected the not-found refusal, got {code} with {message:?}"
        );
    }

    // And a cluster this account does not hold: this operation models no cluster-specific
    // error, so it is the client error, with the sentence every cluster-resolving call
    // answers.
    let case = "a cluster that was never created";
    let (code, message) = refusal(
        &router,
        "DiscoverPollEndpoint",
        json!({ "cluster": "ghost-cluster", "containerInstance": instance_id }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("ClientException", "Cluster not found."),
        "case {case:?}: expected the cluster-not-found refusal, got {code} with {message:?}"
    );
}

#[tokio::test]
async fn execute_command_resolves_the_cluster_then_the_task_then_the_container() {
    let router = fixture().await;
    let task_arn = run_task(&router, CLUSTER, "fixture").await;
    let task_id = task_arn
        .rsplit('/')
        .next()
        .expect("a task ARN ends in its id")
        .to_string();

    // A cluster that does not exist is reported as that, whatever form the reference
    // takes and whatever else the request names.
    let missing_clusters = [
        ("short name", "ghost-cluster".to_string()),
        ("name with digits", "ghost42".to_string()),
        ("longest name", "g".repeat(255)),
        (
            "full ARN",
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/ghost-by-arn"),
        ),
    ];
    for (form, cluster) in &missing_clusters {
        let case = format!("ExecuteCommand with a {form} cluster");
        let (code, message) = refusal(
            &router,
            "ExecuteCommand",
            json!({
                "cluster": cluster,
                "task": task_id.clone(),
                "command": "ls",
                "interactive": true,
            }),
            &case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClusterNotFoundException", "Cluster not found."),
            "case {case:?}: expected the cluster-not-found refusal, got {code} with \
             {message:?}"
        );
    }

    // A task that resolves to nothing IN the named cluster: the forms differ in alphabet,
    // length and whether the reference is an id or an ARN, and one names a task that
    // really exists — in the other cluster.
    let elsewhere = run_task(&router, OTHER_CLUSTER, "fixture").await;
    let missing_tasks = [
        ("32-character id", "b".repeat(32)),
        (
            "uuid-shaped id",
            "00000000-0000-0000-0000-00000000beef".to_string(),
        ),
        (
            "full ARN of nothing",
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{CLUSTER}/{}",
                "c".repeat(32)
            ),
        ),
        ("a task of another cluster, by ARN", elsewhere.clone()),
    ];
    for (form, task) in &missing_tasks {
        let case = format!("ExecuteCommand with a {form}");
        let (code, message) = refusal(
            &router,
            "ExecuteCommand",
            json!({
                "cluster": CLUSTER,
                "task": task,
                "command": "ls",
                "interactive": true,
            }),
            &case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                "The specified task was not found. Specify a valid task ID and try again."
            ),
            "case {case:?}: expected the task-not-found refusal, got {code} with {message:?}"
        );
    }

    // A container the task does not declare is reported on its own, before the session
    // the call would open is attempted.
    for (form, requested) in [
        ("a container of no revision", "ghost".to_string()),
        (
            "a container name differing only in case",
            FIRST_CONTAINER.to_uppercase(),
        ),
    ] {
        let case = format!("ExecuteCommand naming {form}");
        let (code, message) = refusal(
            &router,
            "ExecuteCommand",
            json!({
                "cluster": CLUSTER,
                "task": task_arn.clone(),
                "container": requested,
                "command": "ls",
                "interactive": true,
            }),
            &case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                "The specified container was not found. Specify a valid container name and \
                 try again."
            ),
            "case {case:?}: expected the container-not-found refusal, got {code} with \
             {message:?}"
        );
    }

    // A task and container that DO resolve reach the session itself, which needs the
    // execute-command agent inside the container: no container runs here, so that agent
    // is not running — the condition this modelled refusal states. The rows differ along
    // the task reference's form and along whether the request names a container.
    let resolvable: [(&str, &str, Option<&str>); 4] = [
        ("the task by id, no container named", &task_id, None),
        ("the task by ARN, no container named", &task_arn, None),
        ("the task by id, first container", &task_id, Some(FIRST_CONTAINER)),
        (
            "the task by ARN, second container",
            &task_arn,
            Some(SECOND_CONTAINER),
        ),
    ];
    for (form, task, requested) in resolvable {
        let case = format!("ExecuteCommand on {form}");
        let mut request = json!({
            "cluster": CLUSTER,
            "task": task,
            "command": "ls",
            "interactive": true,
        });
        if let Some(name) = requested {
            request["container"] = json!(name);
        }
        let (code, message) = refusal(&router, "ExecuteCommand", request, &case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                "The execute command failed because execute command was not enabled when \
                 the task was run or the execute command agent isn't running. Wait and try \
                 again or run a new task with execute command enabled and try again."
            ),
            "case {case:?}: expected the agent-not-running refusal, got {code} with \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn update_container_agent_checks_the_id_form_before_it_resolves_anything() {
    let router = fixture().await;

    // The id Amazon ECS mints is 1 to 36 characters of lowercase hex and hyphens, so the
    // rows differ along alphabet, case and length — the bound itself, and one past it —
    // and the refusal is the same whether the cluster exists or not, because a value that
    // is not an id names no instance in any cluster.
    let bad_ids = [
        ("a letter outside the alphabet", "x".to_string()),
        ("an uppercase hex id", "D".repeat(32)),
        ("a long name", "g".repeat(255)),
        ("an empty reference", String::new()),
        ("37 hex characters", "a".repeat(37)),
    ];
    for (form, reference) in &bad_ids {
        for (cluster_case, cluster) in [("a live cluster", CLUSTER), ("a missing cluster", "ghost")]
        {
            let case = format!("UpdateContainerAgent with {form} in {cluster_case}");
            let (code, message) = refusal(
                &router,
                "UpdateContainerAgent",
                json!({ "cluster": cluster, "containerInstance": reference }),
                &case,
            )
            .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                ("InvalidParameterException", INSTANCE_ID_PATTERN_MESSAGE),
                "case {case:?}: expected the id-pattern refusal, got {code} with {message:?}"
            );
        }
    }

    // An id of the right FORM naming nothing: the cluster is resolved first, then the
    // instance. The bound of the pattern (36 characters) is an id, so it reaches both.
    let well_formed = [
        ("32 hex characters", "a".repeat(32)),
        ("a uuid", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()),
        ("36 hex characters", "f".repeat(36)),
        ("one character", "a".to_string()),
    ];
    for (form, reference) in &well_formed {
        let case = format!("UpdateContainerAgent with {form} in a missing cluster");
        let (code, message) = refusal(
            &router,
            "UpdateContainerAgent",
            json!({ "cluster": "ghost-cluster", "containerInstance": reference }),
            &case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClusterNotFoundException", "Cluster not found."),
            "case {case:?}: expected the cluster-not-found refusal, got {code} with \
             {message:?}"
        );

        let case = format!("UpdateContainerAgent with {form} in a live cluster");
        let (code, message) = refusal(
            &router,
            "UpdateContainerAgent",
            json!({ "cluster": CLUSTER, "containerInstance": reference }),
            &case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "ClientException",
                format!("Referenced container instance {reference} not found.").as_str()
            ),
            "case {case:?}: expected the instance-not-found refusal, got {code} with \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn an_agent_update_is_accepted_once_and_lands_on_the_clock() {
    let router = fixture().await;

    // An instance that reported no agent version at all: Amazon ECS cannot tell what to
    // update from, which is what `MissingVersionException` states.
    let silent = register_agent(&router, CLUSTER, "i-0a0a0a0a0a0a0a0a0", None, "silent").await;
    let (code, message) = refusal(
        &router,
        "UpdateContainerAgent",
        json!({ "cluster": CLUSTER, "containerInstance": silent.clone() }),
        "an instance that reported no agent version",
    )
    .await;
    assert_eq!(
        code, "MissingVersionException",
        "case \"no reported version\": expected MissingVersionException, got {code} with \
         {message:?}"
    );

    // An instance already running the version this emulator publishes has nothing to
    // update to.
    let current = register_agent(
        &router,
        CLUSTER,
        "i-0b0b0b0b0b0b0b0b0",
        Some(LATEST_AGENT_VERSION),
        "current",
    )
    .await;
    let (code, message) = refusal(
        &router,
        "UpdateContainerAgent",
        json!({ "cluster": CLUSTER, "containerInstance": current.clone() }),
        "an instance already at the latest version",
    )
    .await;
    assert_eq!(
        code, "NoUpdateAvailableException",
        "case \"already latest\": expected NoUpdateAvailableException, got {code} with \
         {message:?}"
    );

    // An instance behind that version accepts the update, by either reference form, and
    // reports the FIRST stage of it. The stored instance reads the same back.
    for (case, by_arn) in [("by ARN", true), ("by id", false)] {
        let arn = register_agent(
            &router,
            CLUSTER,
            &format!("i-0c0c0c0c0c0c0c0{}", u8::from(by_arn)),
            Some(OLDER_AGENT_VERSION),
            case,
        )
        .await;
        let reference = if by_arn {
            arn.clone()
        } else {
            instance_id_of(&arn)
        };
        let accepted = ok(
            &router,
            "UpdateContainerAgent",
            json!({ "cluster": CLUSTER, "containerInstance": reference }),
            case,
        )
        .await;
        let reported = &accepted["containerInstance"];
        assert_eq!(
            string(reported, "containerInstanceArn", case),
            arn,
            "case {case:?}: expected the instance the request named, got {accepted}"
        );
        assert_eq!(
            reported["agentUpdateStatus"].as_str(),
            Some("PENDING"),
            "case {case:?}: an accepted update reports its first stage, got {accepted}"
        );
        assert_eq!(
            reported["versionInfo"]["agentVersion"].as_str(),
            Some(OLDER_AGENT_VERSION),
            "case {case:?}: the agent still reports the version it is on, got {accepted}"
        );

        // A second call while the first has not landed is refused, and the stored
        // instance is unchanged by that refusal.
        let (code, message) = refusal(
            &router,
            "UpdateContainerAgent",
            json!({ "cluster": CLUSTER, "containerInstance": arn.clone() }),
            case,
        )
        .await;
        assert_eq!(
            code, "UpdateInProgressException",
            "case {case:?}: expected UpdateInProgressException, got {code} with {message:?}"
        );
        let read = describe_instance(&router, CLUSTER, &arn, case).await;
        assert_eq!(
            read["agentUpdateStatus"].as_str(),
            Some("PENDING"),
            "case {case:?}: a refused repeat leaves the stage where it was, got {read}"
        );

        // The clock owes the rest: once the span has passed the instance reports the
        // terminal stage and the version it was brought to, with nobody asking again.
        tokio::time::sleep(ONE_SPAN).await;
        let settled = describe_instance(&router, CLUSTER, &arn, case).await;
        assert_eq!(
            settled["agentUpdateStatus"].as_str(),
            Some("UPDATED"),
            "case {case:?}: the clock must settle the update, got {settled}"
        );
        assert_eq!(
            settled["versionInfo"]["agentVersion"].as_str(),
            Some(LATEST_AGENT_VERSION),
            "case {case:?}: the updated agent reports the new version, got {settled}"
        );

        // And once it is there, there is no update left to make.
        let (code, message) = refusal(
            &router,
            "UpdateContainerAgent",
            json!({ "cluster": CLUSTER, "containerInstance": arn.clone() }),
            case,
        )
        .await;
        assert_eq!(
            code, "NoUpdateAvailableException",
            "case {case:?}: expected NoUpdateAvailableException after the update, got \
             {code} with {message:?}"
        );
    }

    // An instance of ANOTHER cluster is not one this request names.
    let elsewhere = register_agent(
        &router,
        OTHER_CLUSTER,
        "i-0d0d0d0d0d0d0d0d0",
        Some(OLDER_AGENT_VERSION),
        "elsewhere",
    )
    .await;
    let id = instance_id_of(&elsewhere);
    let (code, message) = refusal(
        &router,
        "UpdateContainerAgent",
        json!({ "cluster": CLUSTER, "containerInstance": id.clone() }),
        "an instance of another cluster",
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "ClientException",
            format!("Referenced container instance {id} not found.").as_str()
        ),
        "case \"another cluster\": expected the instance-not-found refusal, got {code} \
         with {message:?}"
    );
}

#[tokio::test]
async fn a_container_state_change_is_recorded_once_and_never_runs_backwards() {
    let router = fixture().await;
    let task_arn = run_task(&router, CLUSTER, "fixture").await;
    let task_id = task_arn
        .rsplit('/')
        .next()
        .expect("a task ARN ends in its id")
        .to_string();

    // The cluster, then the task, then the container: each condition with the code THIS
    // operation declares for it, over several forms of the reference.
    for (form, cluster) in [
        ("short name", "ghost-cluster".to_string()),
        ("longest name", "g".repeat(255)),
        (
            "full ARN",
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/ghost-by-arn"),
        ),
    ] {
        let case = format!("SubmitContainerStateChange with a {form} cluster");
        let (code, message) = refusal(
            &router,
            "SubmitContainerStateChange",
            json!({
                "cluster": cluster,
                "task": "0".repeat(32),
                "containerName": FIRST_CONTAINER,
                "status": "RUNNING",
            }),
            &case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClientException", "Cluster not found."),
            "case {case:?}: expected the cluster-not-found refusal, got {code} with \
             {message:?}"
        );
    }

    for (form, task) in [
        ("32-character id", "0".repeat(32)),
        (
            "uuid-shaped id",
            "00000000-0000-0000-0000-0000000000ff".to_string(),
        ),
        (
            "full ARN of nothing",
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{CLUSTER}/{}",
                "e".repeat(32)
            ),
        ),
    ] {
        let case = format!("SubmitContainerStateChange with a {form} task");
        let (code, message) = refusal(
            &router,
            "SubmitContainerStateChange",
            json!({
                "cluster": CLUSTER,
                "task": task,
                "containerName": FIRST_CONTAINER,
                "status": "RUNNING",
            }),
            &case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", "The referenced task was not found"),
            "case {case:?}: expected the task-not-found refusal, got {code} with {message:?}"
        );
    }

    let case = "SubmitContainerStateChange naming a container of no revision";
    let (code, message) = refusal(
        &router,
        "SubmitContainerStateChange",
        json!({
            "cluster": CLUSTER,
            "task": task_arn.clone(),
            "containerName": "ghost",
            "status": "RUNNING",
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            "The referenced container was not found"
        ),
        "case {case:?}: expected the container-not-found refusal, got {code} with {message:?}"
    );

    // The report the agent makes about a container it RAN: the outcome it carries is
    // stored with the stage, and reads back through `DescribeTasks`.
    let acknowledged = ok(
        &router,
        "SubmitContainerStateChange",
        json!({
            "cluster": CLUSTER,
            "task": task_id.clone(),
            "containerName": FIRST_CONTAINER,
            "status": "STOPPED",
            "runtimeId": "0123456789abcdef0123456789abcdef",
            "exitCode": 137,
            "reason": "OutOfMemoryError: Container killed due to memory usage",
            "networkBindings": [
                { "bindIP": "0.0.0.0", "containerPort": 80, "hostPort": 32768, "protocol": "tcp" }
            ],
        }),
        "a stop report",
    )
    .await;
    assert_eq!(
        acknowledged["acknowledgment"].as_str(),
        Some("ACK"),
        "case \"a stop report\": the agent is told the report was taken, got {acknowledged}"
    );
    let stored = container_of(
        &describe_task(&router, CLUSTER, &task_arn, "a stop report").await,
        FIRST_CONTAINER,
        "a stop report",
    );
    assert_eq!(
        (
            stored["lastStatus"].as_str(),
            stored["exitCode"].as_i64(),
            stored["runtimeId"].as_str(),
        ),
        (
            Some("STOPPED"),
            Some(137),
            Some("0123456789abcdef0123456789abcdef")
        ),
        "case \"a stop report\": the report must be stored, got {stored}"
    );
    assert_eq!(
        stored["networkBindings"][0]["hostPort"].as_i64(),
        Some(32768),
        "case \"a stop report\": the reported network binding must be stored, got {stored}"
    );

    // The agent retries, and reports can overtake each other. A report of a stage the
    // container has already passed — or of the stage it is in — is acknowledged and
    // changes nothing: the rows walk the lifecycle backwards from where the container is.
    for (case, status, exit_code) in [
        ("a replayed STOPPED", "STOPPED", 0),
        ("a stale RUNNING", "RUNNING", 1),
        ("a stale PENDING", "PENDING", 2),
    ] {
        let acknowledged = ok(
            &router,
            "SubmitContainerStateChange",
            json!({
                "cluster": CLUSTER,
                "task": task_id.clone(),
                "containerName": FIRST_CONTAINER,
                "status": status,
                "exitCode": exit_code,
            }),
            case,
        )
        .await;
        assert_eq!(
            acknowledged["acknowledgment"].as_str(),
            Some("ACK"),
            "case {case:?}: a stale report is still acknowledged, got {acknowledged}"
        );
        let read = container_of(
            &describe_task(&router, CLUSTER, &task_arn, case).await,
            FIRST_CONTAINER,
            case,
        );
        assert_eq!(
            (read["lastStatus"].as_str(), read["exitCode"].as_i64()),
            (Some("STOPPED"), Some(137)),
            "case {case:?}: a stale report must change nothing, got {read}"
        );
    }

    // The task's OTHER container is untouched by every report above: a report names one
    // container of one task.
    let sibling = container_of(
        &describe_task(&router, CLUSTER, &task_arn, "the sibling").await,
        SECOND_CONTAINER,
        "the sibling",
    );
    assert!(
        sibling["exitCode"].is_null(),
        "case \"the sibling\": the container nobody reported on holds no outcome, got \
         {sibling}"
    );

    // A report that names no container names the task's first one, which is the one the
    // reports above moved, so it is stale too.
    let acknowledged = ok(
        &router,
        "SubmitContainerStateChange",
        json!({
            "cluster": CLUSTER,
            "task": task_id.clone(),
            "status": "RUNNING",
        }),
        "no container named",
    )
    .await;
    assert_eq!(
        acknowledged["acknowledgment"].as_str(),
        Some("ACK"),
        "case \"no container named\": expected an acknowledgment, got {acknowledged}"
    );
    let read = container_of(
        &describe_task(&router, CLUSTER, &task_arn, "no container named").await,
        FIRST_CONTAINER,
        "no container named",
    );
    assert_eq!(
        read["lastStatus"].as_str(),
        Some("STOPPED"),
        "case \"no container named\": the first container is still where it was, got {read}"
    );
}

#[tokio::test]
async fn a_container_state_change_is_bounded_at_what_its_members_document() {
    let router = fixture().await;
    let task_arn = run_task(&router, CLUSTER, "fixture").await;

    // `NetworkBinding$containerPortRange`: "You can specify a maximum of 100 port ranges
    // per container"; `Container$reason`: "A short (255 max characters) human-readable
    // string". Each published bound is accepted on itself and refused one past it. Rows
    // carry the members the report adds to the request, and the refusal expected (`None`
    // where the report must be taken).
    let bindings = |count: i64| -> Value {
        (0..count)
            .map(|index| {
                json!({
                    "bindIP": "0.0.0.0",
                    "containerPort": index + 1,
                    "hostPort": index + 1,
                    "protocol": "tcp",
                })
            })
            .collect()
    };
    let rows: [(&str, Value, Option<&str>); 3] = [
        (
            "both bounds on themselves",
            json!({ "networkBindings": bindings(100), "reason": "r".repeat(255) }),
            None,
        ),
        (
            "one network binding too many",
            json!({ "networkBindings": bindings(101) }),
            Some("network bindings can have at most 100 items."),
        ),
        (
            "one reason character too many",
            json!({ "reason": "r".repeat(256) }),
            Some("reason can have at most 255 characters."),
        ),
    ];

    for (case, members, expected) in rows {
        let mut request = json!({
            "cluster": CLUSTER,
            "task": task_arn.clone(),
            "containerName": FIRST_CONTAINER,
            "status": "STOPPED",
        });
        for (member, value) in members.as_object().expect("the row carries an object") {
            request[member] = value.clone();
        }
        match expected {
            None => {
                let acknowledged =
                    ok(&router, "SubmitContainerStateChange", request, case).await;
                assert_eq!(
                    acknowledged["acknowledgment"].as_str(),
                    Some("ACK"),
                    "case {case:?}: the bound itself must be accepted, got {acknowledged}"
                );
            }
            Some(sentence) => {
                let (code, message) =
                    refusal(&router, "SubmitContainerStateChange", request, case).await;
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    ("InvalidParameterException", sentence),
                    "case {case:?}: expected the bound refusal, got {code} with {message:?}"
                );
            }
        }
    }

    // The refusals above stored nothing: the container keeps what the ACCEPTED report
    // left it.
    let stored = container_of(
        &describe_task(&router, CLUSTER, &task_arn, "after the bounds").await,
        FIRST_CONTAINER,
        "after the bounds",
    );
    assert_eq!(
        (
            stored["networkBindings"]
                .as_array()
                .map(|bindings| bindings.len()),
            stored["reason"].as_str().map(str::len),
        ),
        (Some(100), Some(255)),
        "case \"after the bounds\": expected what the accepted report left, got {stored}"
    );
}

#[tokio::test]
async fn scale_in_protection_is_set_and_read_back_from_what_was_stored() {
    let router = fixture().await;
    // A task the SERVICE scheduler owns, and a standalone one beside it.
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "protected-service",
            "taskDefinition": FAMILY,
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        "fixture",
    )
    .await;
    let listed = ok(
        &router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": "protected-service" }),
        "fixture",
    )
    .await;
    let task_arns = list(&listed, "taskArns", "fixture");
    let service_task = task_arns
        .first()
        .and_then(|arn| arn.as_str())
        .unwrap_or_else(|| panic!("case \"fixture\": expected the service's task, got {listed}"))
        .to_string();
    let standalone = run_task(&router, CLUSTER, "fixture").await;

    // The cluster is resolved first, for both operations — for an entry that names the task
    // by its own ID, which carries no cluster of its own. An entry naming a task by an ARN
    // scoped to ANOTHER cluster is refused for that disagreement instead, before either
    // identifier is resolved (`crate::task_protection_scale_in`).
    let service_task_id = service_task
        .rsplit('/')
        .next()
        .expect("a task ARN ends with the task id")
        .to_string();
    for operation in ["GetTaskProtection", "UpdateTaskProtection"] {
        let case = format!("{operation} in a missing cluster");
        let mut request = json!({
            "cluster": "ghost-cluster",
            "tasks": [service_task_id.clone()],
        });
        if operation == "UpdateTaskProtection" {
            request["protectionEnabled"] = json!(true);
        }
        let (code, message) = refusal(&router, operation, request, &case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClusterNotFoundException", "Cluster not found."),
            "case {case:?}: expected the cluster-not-found refusal, got {code} with \
             {message:?}"
        );
    }

    // "You can specify a value from 1 minute to up to 2,880 minutes (48 hours)": each
    // published bound is accepted and one past it is refused naming the bound it crossed,
    // and a refusal protects nothing.
    for (case, minutes, refusal_message) in [
        ("one minute", 1, None),
        ("the longest period", 2_880, None),
        (
            "zero minutes",
            0,
            Some("expiresInMinutes must be at least 1"),
        ),
        (
            "one past the longest period",
            2_881,
            Some("expiresInMinutes must be at most 2880"),
        ),
        (
            "a negative period",
            -1,
            Some("expiresInMinutes must be at least 1"),
        ),
    ] {
        let request = json!({
            "cluster": CLUSTER,
            "tasks": [service_task.clone()],
            "protectionEnabled": true,
            "expiresInMinutes": minutes,
        });
        match refusal_message {
            None => {
                let body = ok(&router, "UpdateTaskProtection", request, case).await;
                let protected = list(&body, "protectedTasks", case);
                assert_eq!(
                    protected.len(),
                    1,
                    "case {case:?}: expected the task back protected, got {body}"
                );
                assert_eq!(
                    protected[0]["protectionEnabled"].as_bool(),
                    Some(true),
                    "case {case:?}: an accepted period protects the task, got {body}"
                );
                assert!(
                    protected[0]["expirationDate"].is_number(),
                    "case {case:?}: a protected task reports when protection expires, got \
                     {body}"
                );
            }
            Some(expected) => {
                let (code, message) =
                    refusal(&router, "UpdateTaskProtection", request, case).await;
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    ("InvalidParameterException", expected),
                    "case {case:?}: expected the period refusal, got {code} with {message:?}"
                );
            }
        }
    }

    // What the last accepted write left is what a READ reports: protection is read from
    // the store, not from the call that set it.
    let read = ok(
        &router,
        "GetTaskProtection",
        json!({ "cluster": CLUSTER, "tasks": [service_task.clone()] }),
        "read back",
    )
    .await;
    let protected = list(&read, "protectedTasks", "read back");
    assert_eq!(
        (
            protected.len(),
            protected
                .first()
                .and_then(|task| task["protectionEnabled"].as_bool())
        ),
        (1, Some(true)),
        "case \"read back\": expected the stored protection, got {read}"
    );

    // A standalone task is not one these operations act on, and an id naming nothing is
    // MISSING — both reported as failures rather than refusing the whole call.
    let ghost = format!(
        "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{CLUSTER}/{}",
        "a".repeat(32)
    );
    for operation in ["GetTaskProtection", "UpdateTaskProtection"] {
        let case = format!("{operation} over a standalone task and a ghost");
        let mut request = json!({
            "cluster": CLUSTER,
            "tasks": [standalone.clone(), ghost.clone(), service_task.clone()],
        });
        if operation == "UpdateTaskProtection" {
            request["protectionEnabled"] = json!(true);
        }
        let body = ok(&router, operation, request, &case).await;
        let mut reasons: Vec<String> = list(&body, "failures", &case)
            .iter()
            .map(|failure| string(failure, "reason", &case))
            .collect();
        reasons.sort();
        assert_eq!(
            reasons,
            vec!["MISSING".to_string(), "TASK_NOT_VALID".to_string()],
            "case {case:?}: expected one failure per task this call cannot act on, got \
             {body}"
        );
        assert_eq!(
            list(&body, "protectedTasks", &case).len(),
            1,
            "case {case:?}: only the service's task is answered for, got {body}"
        );
    }

    // Unsetting protection takes it off, and the read agrees.
    let unset = ok(
        &router,
        "UpdateTaskProtection",
        json!({
            "cluster": CLUSTER,
            "tasks": [service_task.clone()],
            "protectionEnabled": false,
        }),
        "unset",
    )
    .await;
    assert_eq!(
        list(&unset, "protectedTasks", "unset")[0]["protectionEnabled"].as_bool(),
        Some(false),
        "case \"unset\": protection must come off, got {unset}"
    );

    // And a READ never moves what a later caller sees: the expiry a read reports is the
    // one the write recorded, not one the read extended.
    let protected = ok(
        &router,
        "UpdateTaskProtection",
        json!({
            "cluster": CLUSTER,
            "tasks": [service_task.clone()],
            "protectionEnabled": true,
            "expiresInMinutes": 1,
        }),
        "expiry",
    )
    .await;
    let expires_at = list(&protected, "protectedTasks", "expiry")[0]["expirationDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case \"expiry\": expected an expiry, got {protected}"));
    let read = ok(
        &router,
        "GetTaskProtection",
        json!({ "cluster": CLUSTER, "tasks": [service_task.clone()] }),
        "expiry",
    )
    .await;
    let still = list(&read, "protectedTasks", "expiry");
    assert_eq!(
        (
            still[0]["protectionEnabled"].as_bool(),
            still[0]["expirationDate"].as_f64()
        ),
        (Some(true), Some(expires_at)),
        "case \"expiry\": a read inside the period must not move the expiry, got {read}"
    );
}
