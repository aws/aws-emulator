// SPDX-License-Identifier: CC0-1.0

//! The access request the call that STARTS a run-a-job (`.sync`) integration's work is
//! subject to, over the integrations whose job belongs to a resource the request names
//! by that resource's NAME — driven over both services' own wire protocols through the
//! production `MultiServiceRouter` with Step Functions and IAM mounted on ONE engine.
//!
//! The rule under test: a `Task` state's integration call is made as the state
//! machine's `roleArn`, and the access request it is judged by is the integration's own
//! — the IAM action the invoked operation publishes over the ARN of the resource the
//! request addresses. For the work a run-a-job integration starts that resource is the
//! one the job BELONGS to: an Athena query's workgroup, a Glue run's job. A call the
//! policies in force on the role do not allow is NOT made: the task fails with the
//! invoked service's own access-denied refusal, reported under that service's base SDK
//! exception, and the execution ends `FAILED`.
//!
//! The rows differ along what the rule generalises over: which integration the
//! `Resource` names, which family names it (the optimized one and the AWS SDK one,
//! which make the same call on the same resource), whether it asks for the run-a-job
//! pattern (the same call either way), and how the request addresses the resource — by
//! name, and by omitting the member, which names the account's default workgroup.
//!
//! Only the DENIED side is here, and that is the point: the call is refused before it
//! is made, so neither target service has to be mounted for the rule to be observed.
//! What the role may do once the job IS started — the pattern's own reporting call —
//! needs those services to answer, so it lives in
//! `core/router/tests/all/sfn_athena_run_a_job_sync.rs`. The harness is
//! `tests/all/task_execution_role.rs`'s: `tests/all.rs` is one crate root, so the
//! wiring and the trust policy are stated there once.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_TASK_FAILED;

use crate::state_machine_lifecycle::{string, REGION};
use crate::task_execution_role::{Chain, REFUSAL_CODE};

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The workgroup an Athena query runs in when its request names none: "if a workgroup
/// is not specified, a list of available query execution IDs for the queries in the
/// primary workgroup is returned" (`ListQueryExecutionsInput$WorkGroup` of
/// `services/service-com-amazonaws-athena-amazonathena/smithy-model/
/// com-amazonaws-athena-amazonathena.json`).
const DEFAULT_WORK_GROUP: &str = "primary";

/// A definition whose one `Task` state makes `resource`'s call with `parameters`.
fn definition(resource: &str, parameters: &Value) -> String {
    json!({
        "StartAt": "Start",
        "States": {
            "Start": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            }
        }
    })
    .to_string()
}

/// One row: the case, the `Resource` the `Task` names, its parameters, the IAM action
/// the refusal must name, the ARN it must be refused ON, and the error name the task
/// reports for it.
struct Row {
    case: &'static str,
    resource: &'static str,
    parameters: Value,
    action: &'static str,
    resource_arn: String,
    refusal_error: &'static str,
}

/// The work a run-a-job integration starts is judged by the action the invoked
/// operation publishes over the resource the job belongs to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_started_jobs_call_is_judged_on_the_resource_the_job_belongs_to() {
    let chain = Chain::new();
    let work_group_arn = format!("arn:aws:athena:{REGION}:{ACCOUNT}:workgroup/analytics");
    let default_work_group_arn =
        format!("arn:aws:athena:{REGION}:{ACCOUNT}:workgroup/{DEFAULT_WORK_GROUP}");
    let job_arn = format!("arn:aws:glue:{REGION}:{ACCOUNT}:job/nightly");
    // Neither target has to exist: authorization is settled before the call, so these
    // name the resources a request addresses and nothing is created for them.
    let role_arn = chain
        .role(
            "StartedJobTargetsRole",
            &json!({
                "Version": "2012-10-17",
                // The log writes a state machine makes, which say nothing about any
                // integration call.
                "Statement": [{
                    "Effect": "Allow",
                    "Action": ["logs:CreateLogDelivery", "logs:PutLogEvents"],
                    "Resource": "*",
                }],
            }),
        )
        .await;

    let rows = [
        Row {
            case: "a query, its workgroup named",
            resource: "arn:aws:states:::athena:startQueryExecution",
            parameters: json!({ "QueryString": "SELECT 1", "WorkGroup": "analytics" }),
            action: "athena:StartQueryExecution",
            resource_arn: work_group_arn.clone(),
            refusal_error: "Athena.AmazonAthenaException",
        },
        Row {
            // The run-a-job pattern makes the same starting call on the same
            // workgroup, so the refusal must hold through it too.
            case: "the same query through the run-a-job pattern",
            resource: "arn:aws:states:::athena:startQueryExecution.sync",
            parameters: json!({ "QueryString": "SELECT 1", "WorkGroup": "analytics" }),
            action: "athena:StartQueryExecution",
            resource_arn: work_group_arn.clone(),
            refusal_error: "Athena.AmazonAthenaException",
        },
        Row {
            // And so does the other family, whose parameters are the invoked
            // operation's own members.
            case: "the same query through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:athena:startQueryExecution",
            parameters: json!({ "QueryString": "SELECT 1", "WorkGroup": "analytics" }),
            action: "athena:StartQueryExecution",
            resource_arn: work_group_arn,
            refusal_error: "Athena.AmazonAthenaException",
        },
        Row {
            // A request naming no workgroup runs in the account's default one, which
            // is the workgroup the access request is then over.
            case: "a query naming no workgroup at all",
            resource: "arn:aws:states:::athena:startQueryExecution.sync",
            parameters: json!({ "QueryString": "SELECT 1" }),
            action: "athena:StartQueryExecution",
            resource_arn: default_work_group_arn,
            refusal_error: "Athena.AmazonAthenaException",
        },
        Row {
            case: "a Glue job run, the job named",
            resource: "arn:aws:states:::glue:startJobRun.sync",
            parameters: json!({ "JobName": "nightly" }),
            action: "glue:StartJobRun",
            resource_arn: job_arn.clone(),
            refusal_error: "Glue.AWSGlueException",
        },
        Row {
            case: "the same job run through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:glue:startJobRun",
            parameters: json!({ "JobName": "nightly" }),
            action: "glue:StartJobRun",
            resource_arn: job_arn,
            refusal_error: "Glue.AWSGlueException",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine_name = format!("started-job-target-{index}");
        let machine = chain
            .machine(
                &machine_name,
                &definition(row.resource, &row.parameters),
                &role_arn,
            )
            .await;
        let (execution_arn, described) = chain.ran(&machine, "only", case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(row.refusal_error),
            "case {case:?}: expected {:?}, got {described:?}",
            row.refusal_error
        );
        let events = chain.history(&execution_arn).await;
        let failed: Vec<&Value> = events
            .iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_FAILED))
            .collect();
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one {EVENT_TASK_FAILED}, got {events:?}"
        );
        let details = failed[0]
            .get("taskFailedEventDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected taskFailedEventDetails"));
        let cause = string(details, "cause");
        assert!(
            cause.contains(REFUSAL_CODE),
            "case {case:?}: expected the cause to carry {REFUSAL_CODE:?}, got {cause:?}"
        );
        assert!(
            cause.contains(row.action),
            "case {case:?}: expected the cause to name the action {:?}, got {cause:?}",
            row.action
        );
        assert!(
            cause.contains(&format!("on resource: {} ", row.resource_arn)),
            "case {case:?}: expected the cause to name the resource {}, got {cause:?}",
            row.resource_arn
        );
        // The principal is the SESSION of the role the call was made as, under the name
        // the assume gave it.
        let principal =
            crate::task_refusal_session_principal::session_principal(&role_arn, &cause);
        assert!(
            cause.contains(&principal),
            "case {case:?}: expected the cause to name the principal {principal}, got {cause:?}"
        );
    }
}
