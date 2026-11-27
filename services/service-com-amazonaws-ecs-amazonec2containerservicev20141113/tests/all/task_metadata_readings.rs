// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of the TASK METADATA ENDPOINT these suites take, and the ONE account of
//! what it has to answer.
//!
//! Two suites drive that endpoint from the two places a session runs inside a container: the
//! container's own program and the probe the agent runs in it
//! ([`crate::task_metadata_endpoint`]), and the session an `ExecuteCommand` opens
//! ([`crate::exec_session_task_metadata`]). What differs between them is the TRANSPORT — a log
//! stream against a data channel — so the address, the program that reads it, and the documents
//! it has to answer are stated here once rather than once per suite.
//!
//! Nothing in this file is a test of its own: it is the harness those suites share, beside the
//! wire harness every suite of this crate shares ([`crate::ecs_wire`]).

use serde_json::{json, Value};

/// The address the endpoint is served at inside every task, as the four octets of the
/// link-local address the ECS developer guide's task metadata endpoint version 4 names, and the
/// version segment of the paths it answers. Written here rather than imported from the service,
/// so a wrong address in the implementation cannot make a case pass — and held as octets, the
/// way this emulator holds every link-local address it serves rather than dials.
const METADATA_HOST_OCTETS: [u8; 4] = [169, 254, 170, 2];
const METADATA_VERSION_SEGMENT: &str = "v4";

/// Those two as the start of every address this endpoint answers at.
pub(crate) fn metadata_uri_prefix() -> String {
    format!(
        "http://{}/{METADATA_VERSION_SEGMENT}/",
        std::net::Ipv4Addr::from(METADATA_HOST_OCTETS)
    )
}

/// The variable Amazon ECS injects that address into.
pub(crate) const METADATA_URI_VARIABLE: &str = "ECS_CONTAINER_METADATA_URI_V4";

/// The prefixes the reading program writes each reading under. Every status is written as the
/// value of a variable the program expands, so the line a terminal ECHOES back for a command
/// can never be read as that command's own output.
pub(crate) const URI_LINE: &str = "uri=";
const OWN_DOCUMENT_LINE: &str = "own=";
const OWN_STATUS_LINE: &str = "own-status=";
const TASK_DOCUMENT_LINE: &str = "task=";
const TASK_STATUS_LINE: &str = "task-status=";
const MISSING_STATUS_LINE: &str = "missing-status=";
const MISSING_DOWNLOADER_LINE: &str = "missing-downloader-status=";
const UNSERVED_STATUS_LINE: &str = "unserved-status=";
const UNSERVED_DOWNLOADER_LINE: &str = "unserved-downloader-status=";

/// The two paths under one container's endpoint that hold no document: the statistics of the
/// container, which the task metadata endpoint version 4 documents and this emulator holds no
/// runtime to compute, and a path no version of the endpoint serves at all.
const STATISTICS_PATH: &str = "stats";
const UNSERVED_PATH: &str = "not-a-document";

/// The status both clients exit with for a transfer that was answered.
const ANSWERED: &str = "0";

/// The status a path the endpoint holds no document for is answered with, as the transfer
/// client's write-out format reports it.
const NOT_FOUND: &str = "404";

/// The status the downloader exits with for a response it reports as an error: "8   Server
/// issued an error response" (the GNU Wget manual's exit-status list), which is what that client
/// makes of that same `404`.
pub(crate) const WGET_SERVER_ERROR: &str = "8";

/// The `lastStatus` a started task and its containers report, which both documents carry as
/// what the task and the container are WANTED at and KNOWN to be at.
pub(crate) const RUNNING: &str = "RUNNING";

/// `Container$type` the endpoint reports for a container of the task definition itself, as
/// against one Amazon ECS adds to the task for a feature of its own.
const CONTAINER_TYPE: &str = "NORMAL";

/// The last reading the program takes, as the line it is written on: a case that has read this
/// has read every reading before it.
pub(crate) fn last_reading() -> String {
    format!("{UNSERVED_DOWNLOADER_LINE}{WGET_SERVER_ERROR}")
}

/// The two clients a program inside a container fetches a document with, as the HTTP client a
/// container's session provides names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Client {
    /// The one that writes what it fetched to its own output.
    Downloader,
    /// The one that transfers a URL.
    Transfer,
}

impl Client {
    /// The command this client fetches `url` to its own output with.
    fn fetch(self, url: &str) -> String {
        match self {
            Client::Downloader => format!("wget -qO- {url}"),
            Client::Transfer => format!("curl -s {url}"),
        }
    }
}

/// The lines of the program that takes every reading: the address the environment carries, then
/// the container's own document and its task's document at that address — one with each client —
/// and then the paths under the endpoint that hold no document at all, asked for with each
/// client in turn. Each document is followed by the status the client that fetched it exited
/// with, so what a case asserts is the client's own account of the request.
pub(crate) fn reading_lines(own: Client, task: Client) -> Vec<String> {
    let uri = format!("${METADATA_URI_VARIABLE}");
    vec![
        format!("echo {URI_LINE}{uri}"),
        format!(
            "printf '{OWN_DOCUMENT_LINE}'; {}; S=$?; echo; echo {OWN_STATUS_LINE}$S",
            own.fetch(&uri)
        ),
        format!(
            "printf '{TASK_DOCUMENT_LINE}'; {}; S=$?; echo; echo {TASK_STATUS_LINE}$S",
            task.fetch(&format!("{uri}/task"))
        ),
        format!(
            "curl -s -o /dev/null -w '{MISSING_STATUS_LINE}%{{http_code}}\\n' \
             {uri}/{STATISTICS_PATH}"
        ),
        format!("wget -qO- {uri}/{STATISTICS_PATH}; echo {MISSING_DOWNLOADER_LINE}$?"),
        format!(
            "curl -s -o /dev/null -w '{UNSERVED_STATUS_LINE}%{{http_code}}\\n' \
             {uri}/{UNSERVED_PATH}"
        ),
        format!("wget -qO- {uri}/{UNSERVED_PATH}; echo {UNSERVED_DOWNLOADER_LINE}$?"),
    ]
}

/// The one line of `lines` that STARTS with `prefix`, past the prefix — and a failure naming
/// what was carried instead when none does. A line that starts with the prefix is the program's
/// own output: a command a terminal echoes starts with the command's own name.
pub(crate) fn line_after(lines: &[String], prefix: &str, case: &str) -> String {
    lines
        .iter()
        .find_map(|line| line.strip_prefix(prefix))
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a line starting {prefix:?}, got {lines:?}")
        })
}

/// The document a line carries, parsed — and a failure naming the text when it is not one.
pub(crate) fn document(text: &str, case: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a JSON document, got {text:?} ({error})")
    })
}

/// What one reading of the endpoint has to have answered: the task and container the reading was
/// taken in, as the API reports them, and the members of each document beyond the ones every
/// reading carries.
#[derive(Debug)]
pub(crate) struct ExpectedReadings<'a> {
    /// The case the failure of any assertion is named with.
    pub(crate) case: &'a str,
    /// The id the agent gave the container the reading was taken in, which names its endpoint.
    pub(crate) runtime_id: &'a str,
    /// That container's name.
    pub(crate) container_name: &'a str,
    /// The ARN of the cluster its task runs in.
    pub(crate) cluster_arn: &'a str,
    /// Its task's ARN.
    pub(crate) task_arn: &'a str,
    /// The family and revision of the task definition that task runs.
    pub(crate) family: &'a str,
    pub(crate) revision: &'a str,
    /// Members of the CONTAINER document beyond the five every reading reports.
    pub(crate) container_members: Vec<(&'static str, Value)>,
    /// Members of the TASK document beyond the six every reading reports.
    pub(crate) task_members: Vec<(&'static str, Value)>,
    /// The `Name` of every container the task document has to carry, in order.
    pub(crate) container_names: Vec<&'a str>,
}

/// The LABELS the agent applies to a container it created, as the endpoint reports them: the
/// cluster the task runs in, the container's own name, its task, and the family and revision of
/// the task definition that task runs.
fn expected_labels(expected: &ExpectedReadings<'_>) -> Vec<(&'static str, Value)> {
    vec![
        ("com.amazonaws.ecs.cluster", json!(expected.cluster_arn)),
        (
            "com.amazonaws.ecs.container-name",
            json!(expected.container_name),
        ),
        ("com.amazonaws.ecs.task-arn", json!(expected.task_arn)),
        (
            "com.amazonaws.ecs.task-definition-family",
            json!(expected.family),
        ),
        (
            "com.amazonaws.ecs.task-definition-version",
            json!(expected.revision),
        ),
    ]
}

/// Assert what one reading of the endpoint answered, whatever carried its lines: the address the
/// environment carried, the container's own document, its task's document, and the two clients'
/// accounts of a path the endpoint holds no document for. Answers the TASK document, so a case
/// can go on to compare the readings of two containers of one task.
pub(crate) fn assert_readings(lines: &[String], expected: &ExpectedReadings<'_>) -> Value {
    let case = expected.case;

    // The address the environment carries: the endpoint of the container the reading was taken
    // in, named by the id the agent gave that container.
    let uri = line_after(lines, URI_LINE, case);
    let expected_uri = format!("{}{}", metadata_uri_prefix(), expected.runtime_id);
    assert_eq!(
        uri, expected_uri,
        "case {case:?}: expected {expected_uri:?}, got {uri:?}"
    );

    // The container's OWN document, and the status the client that fetched it exited with.
    assert_eq!(
        line_after(lines, OWN_STATUS_LINE, case),
        ANSWERED,
        "case {case:?}: expected the client to report the transfer answered, got {lines:?}"
    );
    let own = document(&line_after(lines, OWN_DOCUMENT_LINE, case), case);
    let members = [
        ("DockerId", json!(expected.runtime_id)),
        ("Name", json!(expected.container_name)),
        ("DesiredStatus", json!(RUNNING)),
        ("KnownStatus", json!(RUNNING)),
        ("Type", json!(CONTAINER_TYPE)),
    ];
    for (member, wanted) in members.iter().chain(expected.container_members.iter()) {
        assert_eq!(
            own[member], *wanted,
            "case {case:?}: expected the container's {member} to be {wanted}, got {own}"
        );
    }
    for (label, wanted) in expected_labels(expected) {
        assert_eq!(
            own["Labels"][label], wanted,
            "case {case:?}: expected the label {label:?} to be {wanted}, got {own}"
        );
    }

    // Its TASK's document, read with the other client: the task, and every container in it.
    assert_eq!(
        line_after(lines, TASK_STATUS_LINE, case),
        ANSWERED,
        "case {case:?}: expected the client to report the transfer answered, got {lines:?}"
    );
    let task = document(&line_after(lines, TASK_DOCUMENT_LINE, case), case);
    let members = [
        ("Cluster", json!(expected.cluster_arn)),
        ("TaskARN", json!(expected.task_arn)),
        ("Family", json!(expected.family)),
        // The endpoint carries the revision as TEXT, which is the form the document publishes
        // it in.
        ("Revision", json!(expected.revision)),
        ("DesiredStatus", json!(RUNNING)),
        ("KnownStatus", json!(RUNNING)),
    ];
    for (member, wanted) in members.iter().chain(expected.task_members.iter()) {
        assert_eq!(
            task[member], *wanted,
            "case {case:?}: expected the task's {member} to be {wanted}, got {task}"
        );
    }
    let names: Vec<Value> = task["Containers"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the task document to carry its containers, got {task}")
        })
        .iter()
        .map(|container| container["Name"].clone())
        .collect();
    let wanted: Vec<Value> = expected
        .container_names
        .iter()
        .map(|name| json!(name))
        .collect();
    assert_eq!(
        names, wanted,
        "case {case:?}: expected the task document to carry every container of the task, got \
         {task}"
    );

    // The paths under the endpoint it holds no document for: answered as a document that is not
    // there, which each client reports in its own way — the documented statistics path this
    // emulator holds no runtime to compute, and a path no version of the endpoint serves.
    for (status_line, downloader_line, path) in [
        (MISSING_STATUS_LINE, MISSING_DOWNLOADER_LINE, STATISTICS_PATH),
        (UNSERVED_STATUS_LINE, UNSERVED_DOWNLOADER_LINE, UNSERVED_PATH),
    ] {
        assert_eq!(
            line_after(lines, status_line, case),
            NOT_FOUND,
            "case {case:?}: expected the status of the path {path:?}, which the endpoint holds \
             no document for, got {lines:?}"
        );
        assert_eq!(
            line_after(lines, downloader_line, case),
            WGET_SERVER_ERROR,
            "case {case:?}: expected the downloader to report the error response for {path:?}, \
             got {lines:?}"
        );
    }
    task
}
