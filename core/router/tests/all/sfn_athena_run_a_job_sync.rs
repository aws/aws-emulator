// SPDX-License-Identifier: CC0-1.0

//! The EXECUTION ROLE the calls of a Step Functions run-a-job (`.sync`) task are made
//! as, over the Athena query integration — driven across the three services the
//! question spans, mounted on ONE engine exactly as `core/router/src/lib.rs` mounts
//! them.
//!
//! RULE: Step Functions makes EVERY call of a `Task` state's service integration
//! acting as the state machine's `roleArn` — the call that STARTS the work and, for the
//! run-a-job pattern, the call that REPORTS the job it started — so the identity-based
//! policies in force on that role decide whether each is made. A reporting call those
//! policies do not allow is not made, and the job it would have reported was already
//! submitted: the task therefore fails with ASL's own `States.TaskFailed` carrying the
//! invoked service's access-denied answer, the history keeps its `TaskSubmitted` and
//! gains no `TaskSucceeded`, and the job itself goes on running in its own service. The
//! access request that call is judged by is over the resource the job BELONGS to — a
//! query's workgroup — because the reporting call's own request names the query by its
//! id and IAM takes the action on the workgroup it runs in.
//!
//! Three crates have to agree for that to hold (Step Functions starts and polls the
//! query, IAM holds the documents the polls are judged against, and Athena serves both
//! calls), so the suite lives here rather than in any of them.
//!
//! The rows differ along what the rule generalises over: whether the policies allow the
//! reporting call (no statement about it, an explicit `Deny` over a grant, a grant for
//! the workgroup and a wildcard grant), and how the starting request names the workgroup
//! the access request is over (by name, and omitted, which runs in the account's default
//! workgroup).
//!
//! Nothing here asserts a value this emulator invented: the query id comes off the
//! submission event, the task's result is compared with what Athena's own
//! `GetQueryExecution` reports, and the refusal's own sentence is read out of the
//! `TaskFailed` event.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The harness the other execution-role suite builds: the role with its trust document,
// the state machine, the execution reader and the awsJson caller for Step Functions
// itself. `tests/all.rs` is one crate root, so this suite borrows them rather than
// carrying a second copy.
use crate::sfn_task_target_authorization::{
    allow, logs_only, machine, ran, role, sfn, string, task_failure, ACCOUNT, REGION,
};

/// How long a row waits for Athena to report a terminal query.
const PATIENCE: Duration = Duration::from_secs(20);

/// The fragment the cause of an access-denied refusal carries: the code the answer came
/// back under.
const REFUSAL_CODE: &str = "Error Code: AccessDeniedException";

/// The ASL error name a task whose own work failed reports.
const TASK_FAILED_ERROR: &str = "States.TaskFailed";

/// The history events of a run-a-job task
/// (`smithy_query(query="shape",shape="HistoryEventType")`).
const TASK_SUBMITTED: &str = "TaskSubmitted";
const TASK_SUCCEEDED: &str = "TaskSucceeded";

/// The `Resource` every row's `Task` names: the Athena query integration under the
/// run-a-job pattern.
const START_QUERY_SYNC: &str = "arn:aws:states:::athena:startQueryExecution.sync";

/// The IAM actions the pattern's two calls are taken under: the operation names of
/// Athena's own `StartQueryExecution` and `GetQueryExecution` under that service's
/// action namespace.
const START_ACTION: &str = "athena:StartQueryExecution";
const REPORT_ACTION: &str = "athena:GetQueryExecution";

/// The workgroup a query runs in when the request names none: "if a workgroup is not
/// specified, a list of available query execution IDs for the queries in the primary
/// workgroup is returned" (`ListQueryExecutionsInput$WorkGroup` of
/// `services/service-com-amazonaws-athena-amazonathena/smithy-model/
/// com-amazonaws-athena-amazonathena.json`).
const DEFAULT_WORK_GROUP: &str = "primary";

/// Where a workgroup's results are written. Nothing here reads the objects: the
/// location is what lets a query run at all ("if none of them is set, Athena issues an
/// error that no output location is provided").
const RESULTS_LOCATION: &str = "s3://sfn-athena-sync-results/results/";

/// The statement a query this suite runs is made of, which needs no table.
const QUERY: &str = "SELECT 'marker' AS marker, 42 AS answer";

/// The three services this question spans, over ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_athena_amazonathena::EmulatedService::new(engine),
    );
    router
}

/// Call one Athena operation, requiring a 200.
///
/// `awsJson1_1` under the target prefix `AmazonAthena`, from that service's model's
/// `service_info` (`sigv4Name` `athena`).
async fn athena(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("athena.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonAthena.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/athena/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {operation}: expected 200 from Athena, got {status} with {parsed:?}"
    );
    parsed
}

/// The ARN a policy names the workgroup `name` by, which is the resource Athena takes a
/// query's actions on.
fn work_group_arn(name: &str) -> String {
    format!("arn:aws:athena:{REGION}:{ACCOUNT}:workgroup/{name}")
}

/// Register a workgroup whose results go to [`RESULTS_LOCATION`].
async fn work_group(router: &MultiServiceRouter, name: &str) {
    athena(
        router,
        "CreateWorkGroup",
        &json!({
            "Name": name,
            "Configuration": {
                "ResultConfiguration": { "OutputLocation": RESULTS_LOCATION },
            },
        }),
    )
    .await;
}

/// The events of one execution's history.
async fn history(router: &MultiServiceRouter, execution_arn: &str) -> Vec<Value> {
    let answered = sfn(
        router,
        "GetExecutionHistory",
        &json!({ "executionArn": execution_arn, "maxResults": 100 }),
    )
    .await;
    answered
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// How many events of `events` are of the type `event_type`.
fn count_of(events: &[Value], event_type: &str) -> usize {
    events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
        .count()
}

/// The `QueryExecutionId` the one `TaskSubmitted` event of this history reports, which
/// is what the starting call answered.
fn submitted_query_id(events: &[Value], case: &str) -> String {
    assert_eq!(
        count_of(events, TASK_SUBMITTED),
        1,
        "case {case:?}: expected exactly one {TASK_SUBMITTED}, got {events:?}"
    );
    let submitted = events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some(TASK_SUBMITTED))
        .unwrap_or_else(|| panic!("case {case:?}: expected a {TASK_SUBMITTED} event"));
    let output = submitted
        .get("taskSubmittedEventDetails")
        .and_then(|details| details.get("output"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the submission's output, got {submitted:?}"));
    let answered: Value = serde_json::from_str(output)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON submission, got {output:?}"));
    answered
        .get("QueryExecutionId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the submission to name the QueryExecutionId, got {answered:?}")
        })
        .to_string()
}

/// Athena's own report of the query `query_id`, once that query is terminal.
async fn query_settled(router: &MultiServiceRouter, query_id: &str, case: &str) -> Value {
    let began = Instant::now();
    loop {
        let reported = athena(
            router,
            "GetQueryExecution",
            &json!({ "QueryExecutionId": query_id }),
        )
        .await;
        let state = reported["QueryExecution"]["Status"]["State"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if !matches!(state.as_str(), "QUEUED" | "RUNNING") {
            return reported;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected Athena to settle the query within {PATIENCE:?}, got {reported:?}"
        );
        ::tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// One row: the case, the workgroup the starting request names (`None` omits the
/// member, which runs in the default workgroup), the statements the role carries beside
/// the log writes, and whether the pattern's reporting call is allowed.
struct Row {
    case: &'static str,
    work_group: Option<String>,
    statements: Vec<Value>,
    polls: bool,
}

/// A run-a-job task's REPORTING call is made only when the execution role allows it, on
/// the workgroup the query runs in; the job it started runs either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_execution_role_decides_the_patterns_reporting_call() {
    let router = emulator();
    let named = "sfn-athena-sync";
    work_group(&router, named).await;
    let named_arn = work_group_arn(named);
    let default_arn = work_group_arn(DEFAULT_WORK_GROUP);

    let rows = vec![
        Row {
            case: "the reporting call granted on the workgroup the query names",
            work_group: Some(named.to_string()),
            statements: vec![
                allow(START_ACTION, &named_arn),
                allow(REPORT_ACTION, &named_arn),
            ],
            polls: true,
        },
        Row {
            case: "no statement about the reporting call",
            work_group: Some(named.to_string()),
            statements: vec![allow(START_ACTION, &named_arn)],
            polls: false,
        },
        Row {
            case: "an explicit deny of the reporting call over a grant",
            work_group: Some(named.to_string()),
            statements: vec![
                allow(START_ACTION, &named_arn),
                allow(REPORT_ACTION, &named_arn),
                json!({ "Effect": "Deny", "Action": REPORT_ACTION, "Resource": &named_arn }),
            ],
            polls: false,
        },
        Row {
            case: "a wildcard grant over every Athena action",
            work_group: Some(named.to_string()),
            statements: vec![allow("athena:*", "*")],
            polls: true,
        },
        Row {
            // A request that names no workgroup runs in the account's default one, so
            // that is the workgroup the pattern's calls are taken on.
            case: "the default workgroup, both calls granted on it",
            work_group: None,
            statements: vec![
                allow(START_ACTION, &default_arn),
                allow(REPORT_ACTION, &default_arn),
            ],
            polls: true,
        },
        Row {
            case: "the default workgroup, the reporting call granted on another",
            work_group: None,
            statements: vec![
                allow(START_ACTION, &default_arn),
                allow(REPORT_ACTION, &named_arn),
            ],
            polls: false,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let mut statements = vec![logs_only()];
        statements.extend(row.statements);
        let role_arn = role(
            &router,
            &format!("AthenaSyncRole{index}"),
            &json!({ "Version": "2012-10-17", "Statement": statements }),
        )
        .await;
        // The parameters a query is started with: the statement, the workgroup where
        // the row names one, and otherwise the results location that would come from
        // the workgroup's own settings.
        let parameters = match &row.work_group {
            Some(group) => json!({ "QueryString": QUERY, "WorkGroup": group }),
            None => json!({
                "QueryString": QUERY,
                "ResultConfiguration": { "OutputLocation": RESULTS_LOCATION },
            }),
        };
        let machine_name = format!("athena-sync-{index}");
        let machine_arn = machine(
            &router,
            &machine_name,
            START_QUERY_SYNC,
            &parameters,
            &role_arn,
        )
        .await;
        let (execution_arn, described) = ran(&router, &machine_arn, case).await;
        let events = history(&router, &execution_arn).await;
        // Whatever the policies say about reporting the query, the STARTING call was
        // granted, so the query was really submitted and the task really parked.
        let query_id = submitted_query_id(&events, case);
        let reported = query_settled(&router, &query_id, case).await;
        assert_eq!(
            reported["QueryExecution"]["Status"]["State"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: expected Athena to run the query it was handed, got {reported:?}"
        );

        let group_arn = match &row.work_group {
            Some(group) => work_group_arn(group),
            None => default_arn.clone(),
        };
        if !row.polls {
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some("FAILED"),
                "case {case:?}: expected FAILED, got {described:?}"
            );
            assert_eq!(
                described.get("error").and_then(Value::as_str),
                Some(TASK_FAILED_ERROR),
                "case {case:?}: expected {TASK_FAILED_ERROR}, got {described:?}"
            );
            let details = task_failure(&router, &execution_arn, case).await;
            let cause = string(&details, "cause");
            assert!(
                cause.contains(REFUSAL_CODE),
                "case {case:?}: expected the cause to carry {REFUSAL_CODE:?}, got {cause:?}"
            );
            assert!(
                cause.contains(REPORT_ACTION),
                "case {case:?}: expected the cause to name {REPORT_ACTION}, got {cause:?}"
            );
            assert!(
                cause.contains(&format!("on resource: {group_arn} ")),
                "case {case:?}: expected the cause to name the workgroup {group_arn}, got {cause:?}"
            );
            // The principal is the SESSION of the role the poll was made as, under the
            // name the assume gave it.
            let principal =
                crate::sfn_task_target_authorization::session_principal(&role_arn, &cause);
            assert!(
                cause.contains(&principal),
                "case {case:?}: expected the cause to name the principal {principal}, got \
                 {cause:?}"
            );
            assert_eq!(
                count_of(&events, TASK_SUCCEEDED),
                0,
                "case {case:?}: nothing reported the job, so no {TASK_SUCCEEDED} may appear, got \
                 {events:?}"
            );
            continue;
        }

        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        assert_eq!(
            count_of(&events, TASK_SUCCEEDED),
            1,
            "case {case:?}: expected exactly one {TASK_SUCCEEDED}, got {events:?}"
        );
        // The task's result IS what the reporting call answered: Athena's own document
        // for that query, which is what the execution's output carries.
        let output: Value = serde_json::from_str(&string(&described, "output"))
            .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON output in {described:?}"));
        assert_eq!(
            output["QueryExecution"]["QueryExecutionId"].as_str(),
            Some(query_id.as_str()),
            "case {case:?}: expected the output to carry the query that was submitted, got \
             {output:?}"
        );
        assert_eq!(
            output["QueryExecution"]["Status"]["State"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: expected the output to carry the query's terminal state, got {output:?}"
        );
        assert_eq!(
            output["QueryExecution"]["WorkGroup"].as_str(),
            reported["QueryExecution"]["WorkGroup"].as_str(),
            "case {case:?}: expected the output to be Athena's own report, got {output:?}"
        );
    }
}
