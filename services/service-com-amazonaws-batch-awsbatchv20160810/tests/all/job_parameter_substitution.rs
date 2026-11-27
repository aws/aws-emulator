// SPDX-License-Identifier: CC0-1.0

//! The PARAMETER SUBSTITUTION a job's container is run with, end to end over the wire.
//!
//! RULE under test: `RegisterJobDefinition$parameters` is "default parameter substitution
//! placeholders to set in the job definition" and `SubmitJob$parameters` is "additional
//! parameters passed to the job that replace parameter substitution placeholders that are set
//! in the job definition … Parameters in a `SubmitJob` request override any corresponding
//! parameter defaults from the job definition"
//! (`smithy_query(query="doc", shape="SubmitJobRequest", member="parameters")`). So a
//! placeholder — `Ref::` followed by the parameter's name — standing in the words of the
//! container's `command` or in the values of its `environment` is REPLACED by that job's
//! effective parameter, the submit's value over the definition's default, in the container the
//! submit resolves for the job: so the container that runs carries the values, the program it
//! runs is the one those values decide — which its `ContainerDetail$exitCode` ("the exit code
//! returned upon completion") and so the job's own terminal `JobStatus` follow from — and
//! `DescribeJobs` reports that resolved container, with `JobDetail$parameters` echoing the
//! effective map the values came from.
//!
//! The rows differ along what the rule generalises over: WHERE the placeholder stands (a word
//! of the command, part of a longer word, an operand the program reads as a positional
//! parameter, the value of an environment variable, several placeholders in one program),
//! WHICH side decides the value (the definition's default, the submit's override, a definition
//! default with no override, a placeholder no parameter names at all), and whether the
//! container's command came from the definition or from `containerOverrides` — "the command to
//! send to the container that overrides the default command from the Docker image or the job
//! definition" (`smithy_query(query="shape", shape="ContainerOverrides")`).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::array_jobs_and_placement::managed_compute_environment;
use crate::job_lifecycle::{describe_job, job_queue, Batch};

/// `JobStatus` members this suite names. Source:
/// `smithy_query(query="shape", shape="JobStatus")`.
const SUCCEEDED: &str = "SUCCEEDED";
const FAILED: &str = "FAILED";

/// How long one job is given to settle, polled for the CONDITION rather than waited out.
const SETTLES_WITHIN: Duration = Duration::from_secs(60);

/// How long one look waits before the next.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One row: what the definition and the submit declare, and the resolved container the job must
/// then run and report.
struct Row {
    case: &'static str,
    /// `RegisterJobDefinition$parameters`.
    defaults: &'static [(&'static str, &'static str)],
    /// The definition's `containerProperties.command`.
    command: &'static [&'static str],
    /// The definition's container `environment`.
    environment: &'static [(&'static str, &'static str)],
    /// `SubmitJob$parameters`, empty for a submit that states none.
    overrides: &'static [(&'static str, &'static str)],
    /// `SubmitJob$containerOverrides.command`, empty for a submit that states none.
    override_command: &'static [&'static str],
    /// The command `DescribeJobs` must report, which is the one the container was given.
    resolved_command: &'static [&'static str],
    /// The environment it must report with it.
    resolved_environment: &'static [(&'static str, &'static str)],
    /// The status the resolved program exits with.
    exit: i64,
}

const ROWS: &[Row] = &[
    Row {
        case: "a definition default fills the placeholder the command carries",
        defaults: &[("code", "0")],
        command: &["/bin/sh", "-c", "exit Ref::code"],
        environment: &[],
        overrides: &[],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "exit 0"],
        resolved_environment: &[],
        exit: 0,
    },
    Row {
        case: "the submit's parameter overrides that default",
        defaults: &[("code", "0")],
        command: &["/bin/sh", "-c", "exit Ref::code"],
        environment: &[],
        overrides: &[("code", "6")],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "exit 6"],
        resolved_environment: &[],
        exit: 6,
    },
    Row {
        case: "a placeholder inside a longer word, and one the program writes",
        defaults: &[("code", "0"), ("message", "greeting")],
        command: &["/bin/sh", "-c", "echo msg=Ref::message; exit Ref::code"],
        environment: &[],
        overrides: &[],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "echo msg=greeting; exit 0"],
        resolved_environment: &[],
        exit: 0,
    },
    Row {
        case: "a placeholder in an OPERAND the program reads as a positional parameter",
        defaults: &[("code", "0")],
        command: &["/bin/sh", "-c", "exit \"$0\"", "Ref::code"],
        environment: &[],
        overrides: &[],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "exit \"$0\"", "0"],
        resolved_environment: &[],
        exit: 0,
    },
    Row {
        case: "the submit's parameter decides that operand too",
        defaults: &[("code", "0")],
        command: &["/bin/sh", "-c", "exit \"$0\"", "Ref::code"],
        environment: &[],
        overrides: &[("code", "7")],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "exit \"$0\"", "7"],
        resolved_environment: &[],
        exit: 7,
    },
    Row {
        case: "a placeholder in the value of an environment variable",
        defaults: &[("code", "4")],
        command: &["/bin/sh", "-c", "exit $DECLARED_CODE"],
        environment: &[("DECLARED_CODE", "Ref::code")],
        overrides: &[],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "exit $DECLARED_CODE"],
        resolved_environment: &[("DECLARED_CODE", "4")],
        exit: 4,
    },
    Row {
        case: "the submit's parameter reaches an environment placeholder too",
        defaults: &[("code", "4")],
        command: &["/bin/sh", "-c", "exit $DECLARED_CODE"],
        environment: &[("DECLARED_CODE", "Ref::code")],
        overrides: &[("code", "9")],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "exit $DECLARED_CODE"],
        resolved_environment: &[("DECLARED_CODE", "9")],
        exit: 9,
    },
    Row {
        case: "several placeholders decide one program",
        defaults: &[("first", "1"), ("second", "2"), ("code", "3")],
        command: &["/bin/sh", "-c", "echo Ref::first Ref::second; exit Ref::code"],
        environment: &[],
        overrides: &[("code", "5")],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "echo 1 2; exit 5"],
        resolved_environment: &[],
        exit: 5,
    },
    Row {
        case: "the command containerOverrides sent is the one substituted into",
        defaults: &[("code", "7")],
        command: &["/bin/sh", "-c", "exit 1"],
        environment: &[],
        overrides: &[],
        override_command: &["/bin/sh", "-c", "exit Ref::code"],
        resolved_command: &["/bin/sh", "-c", "exit 7"],
        resolved_environment: &[],
        exit: 7,
    },
    Row {
        case: "a placeholder no parameter names is left as the definition wrote it",
        defaults: &[("code", "0")],
        command: &["/bin/sh", "-c", "echo Ref::absent; exit Ref::code"],
        environment: &[],
        overrides: &[],
        override_command: &[],
        resolved_command: &["/bin/sh", "-c", "echo Ref::absent; exit 0"],
        resolved_environment: &[],
        exit: 0,
    },
];

/// One `parameters`-shaped map as a request carries it.
fn map_of(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// One `environment` list as a request carries it.
fn environment_of(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect(),
    )
}

/// Look again until `job_id` reports a terminal `JobStatus`, and answer its detail.
async fn until_terminal(batch: &Batch, case: &str, job_id: &str) -> Value {
    let deadline = Instant::now() + SETTLES_WITHIN;
    loop {
        let detail = describe_job(batch, job_id).await;
        let status = detail["status"].as_str().unwrap_or_default();
        if status == SUCCEEDED || status == FAILED {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected a terminal status inside {SETTLES_WITHIN:?}, last {detail}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: the container a job RUNS is its declared container with the job's effective parameters
/// substituted into the words of its command and the values of its environment — so the exit
/// status the job reports is the one that substituted program ends with, and the container the
/// job reports is the resolved one, beside the effective parameters it was resolved from.
#[tokio::test]
async fn a_job_runs_the_program_its_effective_parameters_fill() {
    let batch = Batch::new();
    let environment =
        managed_compute_environment(&batch, "ce-parameters", state::STATE_ENABLED).await;
    let queue_arn = job_queue(&batch, "q-parameters", &environment).await;

    let mut submitted: Vec<(&Row, String)> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("jd-parameters-{index}");
        batch
            .ok(
                "/v1/registerjobdefinition",
                json!({
                    "jobDefinitionName": &name,
                    "type": "container",
                    "parameters": map_of(row.defaults),
                    "containerProperties": {
                        "image": "busybox",
                        "command": row.command,
                        "environment": environment_of(row.environment),
                        "resourceRequirements": [
                            {"type": "VCPU", "value": "1"},
                            {"type": "MEMORY", "value": "512"},
                        ],
                    },
                }),
            )
            .await;
        let mut request = json!({
            "jobName": format!("parameters-{index}"),
            "jobQueue": &queue_arn,
            "jobDefinition": &name,
        });
        if !row.overrides.is_empty() {
            request["parameters"] = map_of(row.overrides);
        }
        if !row.override_command.is_empty() {
            request["containerOverrides"] = json!({ "command": row.override_command });
        }
        let answered = batch.ok("/v1/submitjob", request).await;
        let job_id = answered["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected a jobId in {answered}", row.case))
            .to_string();
        submitted.push((row, job_id));
    }

    for (row, job_id) in &submitted {
        let case = row.case;
        let detail = until_terminal(&batch, case, job_id).await;
        let wanted = if row.exit == 0 { SUCCEEDED } else { FAILED };
        assert_eq!(
            detail["container"]["exitCode"].as_i64(),
            Some(row.exit),
            "case {case}: expected the substituted program to exit {}, got {detail}",
            row.exit
        );
        assert_eq!(
            detail["status"].as_str(),
            Some(wanted),
            "case {case}: expected the job to end {wanted} on that exit status, got {detail}"
        );
        // The container the job REPORTS is the resolved one: the values stand where the
        // placeholders were, so a client reads what the container was given.
        assert_eq!(
            detail["container"]["command"],
            json!(row.resolved_command),
            "case {case}: expected the resolved command to be reported, got {detail}"
        );
        assert_eq!(
            detail["container"]["environment"],
            environment_of(row.resolved_environment),
            "case {case}: expected the resolved environment to be reported, got {detail}"
        );
        // And the effective parameters those values came from.
        let mut expected_parameters = map_of(row.defaults);
        for (key, value) in row.overrides {
            expected_parameters[*key] = json!(value);
        }
        assert_eq!(
            detail["parameters"], expected_parameters,
            "case {case}: expected the effective parameters to be reported, got {detail}"
        );
    }
}
