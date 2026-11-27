// SPDX-License-Identifier: CC0-1.0

//! The TASK METADATA ENDPOINT read from inside an EXECUTE-COMMAND session, driven end to end
//! over the session's own data channel.
//!
//! # The rule under test
//!
//! > A session an `ExecuteCommand` opens runs INSIDE the container it names, so it carries the
//! > same environment that container's own program does: the address of the task metadata
//! > endpoint under `ECS_CONTAINER_METADATA_URI_V4`, naming that container and no sibling of
//! > it. A program typed into the session reads its OWN container's metadata at that address
//! > and its TASK's metadata — the task and every container in it — under `task` beneath it,
//! > every member computed from the task's own records, so what the program reads is what a
//! > client reads back through `DescribeTasks`; a path the endpoint holds no document for is
//! > answered `404`.
//!
//! # What the rows span
//!
//! WHICH container of a two-container task the session is opened on, WHICH client the typed
//! program fetches each document with (the downloader that writes what it fetched to its own
//! output, and the transfer client — swapped between the rows, so neither document is tied to
//! one of them), and WHICH document is asked for: the container's, the task's, and the paths
//! under the endpoint it holds none of.
//!
//! The reading program and what each document has to answer are the shared ones
//! ([`crate::task_metadata_readings`]), which the suite driving the container's OWN program
//! reads the same endpoint through ([`crate::task_metadata_endpoint`]): what this suite adds is
//! the TRANSPORT — a session's data channel rather than a log stream.
//!
//! Nothing is asserted off the store: every session is opened through `ExecuteCommand`'s own
//! wire call, every byte is read off a real connection to the announced data channel, and every
//! value compared against is read off Amazon ECS's own `DescribeTasks`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, poll_until, service, string, ACCOUNT};
use crate::exec_session_data_channel::{
    agent_status, dial, long_running_container, session_of, Channel,
};
use crate::task_metadata_readings::{
    assert_readings, last_reading, reading_lines, Client, ExpectedReadings, RUNNING,
};

/// The cluster the task of this suite runs in.
const CLUSTER: &str = "execmetadata";

/// The task-definition family it runs, with TWO containers so a session's reading is the
/// reading of the container it was opened on.
const FAMILY: &str = "execmetadatafamily";

/// The two containers that family declares.
const FIRST_CONTAINER: &str = "app";
const SECOND_CONTAINER: &str = "sidecar";

/// The status a connected execute-command agent reports, which a session is opened on.
const AGENT_RUNNING: &str = "RUNNING";

/// The shell `ExecuteCommand` is asked for, which the session runs.
const SHELL: &str = "/bin/sh";

/// One row: the container a session is opened on, and the client each of its two documents is
/// read with.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    container: &'static str,
    /// The client the container's OWN document is read with.
    own: Client,
    /// The client its TASK's document is read with.
    task: Client,
}

/// The rows, spanning the container the session runs in and which client reads which document.
const ROWS: [Row; 2] = [
    Row {
        case: "the session of the first container, reading its own document with the downloader",
        container: FIRST_CONTAINER,
        own: Client::Downloader,
        task: Client::Transfer,
    },
    Row {
        case: "the session of the sibling container, reading its own document with the transfer \
               client",
        container: SECOND_CONTAINER,
        own: Client::Transfer,
        task: Client::Downloader,
    },
];

/// A router holding one cluster with capacity and one revision of [`FAMILY`] declaring a task
/// role — an exec-enabled launch needs one. Answers the cluster's ARN and the revision
/// registered, both of which the documents report.
async fn fixture(case: &str) -> (MultiServiceRouter, String, String) {
    let router = service();
    let cluster_arn = cluster_with_capacity(&router, CLUSTER, case).await;
    let registered = ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "taskRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/execMetadataTaskRole"),
            "containerDefinitions": [
                long_running_container(FIRST_CONTAINER),
                long_running_container(SECOND_CONTAINER),
            ],
        }),
        case,
    )
    .await;
    let definition_arn = string(&registered["taskDefinition"], "taskDefinitionArn", case);
    let revision = definition_arn
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string();
    (router, cluster_arn, revision)
}

/// One exec-enabled task of [`FAMILY`], read back through `DescribeTasks` once it is RUNNING
/// with a runtime id and a connected agent on each of its containers — the condition a session
/// is opened on, waited for rather than slept through.
async fn running_task(router: &MultiServiceRouter, case: &str) -> (String, Value) {
    let launched = ok(
        router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": FAMILY,
            "count": 1,
            "enableExecuteCommand": true,
        }),
        case,
    )
    .await;
    let task_arn = string(&list(&launched, "tasks", case)[0], "taskArn", case);
    let settled = poll_until(
        case,
        "the task to report RUNNING with a runtime id and a connected agent on each container",
        || {
            ok(
                router,
                "DescribeTasks",
                json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
                case,
            )
        },
        |body| {
            let task = &body["tasks"][0];
            task["lastStatus"].as_str() == Some(RUNNING)
                && [FIRST_CONTAINER, SECOND_CONTAINER].iter().all(|name| {
                    agent_status(task, name, case).as_deref() == Some(AGENT_RUNNING)
                        && task["containers"].as_array().is_some_and(|containers| {
                            containers.iter().any(|container| {
                                container["name"].as_str() == Some(*name)
                                    && container["runtimeId"].is_string()
                            })
                        })
                })
        },
    )
    .await;
    (task_arn, settled["tasks"][0].clone())
}

/// The container `DescribeTasks` reports under `name`.
fn described_container(task: &Value, name: &str, case: &str) -> Value {
    list(task, "containers", case)
        .into_iter()
        .find(|container| container["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case {case:?}: expected the container {name:?} in {task}"))
}

/// Open a session on `container` of `task_arn` and type every line of the reading program into
/// it, answering the lines the channel carried back — read until the LAST reading has been
/// written, so a case waits for the condition rather than for a span.
async fn session_readings(
    router: &MultiServiceRouter,
    task_arn: &str,
    row: Row,
    case: &str,
) -> Vec<String> {
    let answer = ok(
        router,
        "ExecuteCommand",
        json!({
            "cluster": CLUSTER,
            "task": task_arn,
            "container": row.container,
            "command": SHELL,
            "interactive": true,
        }),
        case,
    )
    .await;
    assert_eq!(
        answer["containerName"].as_str(),
        Some(row.container),
        "case {case:?}: expected the session to be opened on the container the request named, \
         got {answer}"
    );
    let (_, stream_url, token) = session_of(&answer, case);
    let mut channel: Channel = dial(case, &stream_url, &token)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the session's channel must carry it"));
    for line in reading_lines(row.own, row.task) {
        channel.type_line(case, &line).await;
    }
    let read = channel.read_until(case, &last_reading()).await;
    read.lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

#[tokio::test]
async fn an_exec_session_reads_its_containers_task_metadata_endpoint() {
    let case = "an exec-enabled task whose sessions read the metadata endpoint";
    let (router, cluster_arn, revision) = fixture(case).await;
    let (task_arn, described) = running_task(&router, case).await;

    // What the one TASK document has to report, whichever container's session read it.
    let mut task_documents: Vec<Value> = Vec::new();
    for row in ROWS {
        let case = format!("{case}: {}", row.case);
        let case = case.as_str();
        let lines = session_readings(&router, &task_arn, row, case).await;
        let container = described_container(&described, row.container, case);
        let runtime_id = string(&container, "runtimeId", case);
        task_documents.push(assert_readings(
            &lines,
            &ExpectedReadings {
                case,
                runtime_id: &runtime_id,
                container_name: row.container,
                cluster_arn: &cluster_arn,
                task_arn: &task_arn,
                family: FAMILY,
                revision: &revision,
                // What this reading's own container is, beside the members every reading
                // reports: both read off `DescribeTasks` for the same container.
                container_members: vec![
                    ("Image", container["image"].clone()),
                    ("ContainerARN", container["containerArn"].clone()),
                ],
                // Where the task was placed and how, as the task it belongs to reports them: a
                // program reads its own zone and launch type off this document rather than
                // through the API.
                task_members: vec![
                    ("AvailabilityZone", described["availabilityZone"].clone()),
                    ("LaunchType", described["launchType"].clone()),
                ],
                container_names: vec![FIRST_CONTAINER, SECOND_CONTAINER],
            },
        ));
    }

    // The two sessions read ONE task document — the task is the same task from inside either
    // container — while each read a container document of its own.
    assert_eq!(
        task_documents[0], task_documents[1],
        "expected both sessions of the task to read the same task document"
    );
}
