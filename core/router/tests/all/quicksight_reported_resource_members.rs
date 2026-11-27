// SPDX-License-Identifier: CC0-1.0

//! The members a QuickSight response carries for the resource it describes.
//!
//! RULE under test: **a member of a QuickSight resource is reported by every response
//! that carries that resource, with the value the request submitted or the service
//! derived — whether the on-disk model declares the member or the service reports it
//! beside the modelled ones.** Both halves of that rule are driven here over the wire,
//! so what is asserted is what a QuickSight SDK client observes:
//!
//! - the MODELLED half — `CreateDataSet` takes `UnmaterializedCalculatedColumns`
//!   (`smithy_query(query="shape",shape="CreateDataSetRequest")`) and `DataSet` declares
//!   the same member, so the list a create submits is persisted and reported by the
//!   describe, in the order it was submitted, and a list past the member's published
//!   maximum is refused before anything is stored;
//! - the half the model does NOT declare — an IAM policy assignment's own ARN, which the
//!   service reports in the create, the describe, the update and the delete bodies even
//!   though `CreateIAMPolicyAssignmentResponse`, `IAMPolicyAssignment`,
//!   `UpdateIAMPolicyAssignmentResponse` and `DeleteIAMPolicyAssignmentResponse` declare
//!   no `Arn` member.
//!
//! The rows vary what the rule generalises over: how many columns the list holds (none at
//! all, an empty list, one, several, and the published maximum), which optional members
//! each column carries, and — for the assignment half — the account, the namespace and
//! the response under test.
//!
//! It lives beside the router's suites because that is the target this crate adds a suite
//! to without touching a service crate's declaration list, and it drives the emulator as
//! it is ASSEMBLED, which is the wire path a client takes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every case uses unless it is about account keying.
const ACCOUNT: &str = "000000000000";

/// A second, unrelated 12-digit account id.
const OTHER_ACCOUNT: &str = "210987654321";

/// The namespace every QuickSight account has.
const DEFAULT_NAMESPACE: &str = "default";

/// A second namespace, for the row about namespace addressing.
const OTHER_NAMESPACE: &str = "analytics";

/// The Region the test client signs for.
const REGION: &str = "us-west-2";

/// The published maximum of `UnmaterializedCalculatedColumnList`
/// (`smithy_query(query="shape",shape="UnmaterializedCalculatedColumnList")`: `@length`
/// 0..512).
const MAX_COLUMNS: usize = 512;

/// An IAM policy ARN a request can name; the member is a free-form `Arn`.
const READ_ONLY_POLICY: &str = "arn:aws:iam::aws:policy/AWSQuicksightAthenaAccess";

/// The emulator as it is assembled, on one engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // These cases are about the handlers, not about signature verification; the
    // credential scope is still what routes the request.
    router.set_request_authentication(false);
    router
}

/// Dispatch one request signed for `quicksight` in [`REGION`], and answer its status and
/// parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://quicksight.{REGION}.amazonaws.com{path}"))
        .header("host", format!("quicksight.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSIGHT/20260101/{REGION}/quicksight/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// The error code a refusal body carries.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create the data source every fixture data set reads, and answer its ARN.
async fn data_source_fixture(router: &MultiServiceRouter, account: &str, id: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/accounts/{account}/data-sources"),
        Some(&json!({ "DataSourceId": id, "Name": "fixture source", "Type": "ATHENA" })),
    )
    .await;
    assert_eq!(
        status, 202,
        "fixture data source {id:?}: create expected 202, got {status} with {body}"
    );
    body["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture data source {id:?}: create returned no Arn: {body}"))
        .to_string()
}

/// A one-entry `PhysicalTableMap` reading `data_source_arn`.
fn relational_table_map(data_source_arn: &str) -> Value {
    json!({
        "orders-table": {
            "RelationalTable": {
                "DataSourceArn": data_source_arn,
                "Schema": "public",
                "Name": "orders",
                "InputColumns": [{ "Name": "order_id", "Type": "INTEGER" }],
            }
        }
    })
}

/// One unmaterialized calculated column. `Type` and `SubType` are the model's own
/// catalogs (`smithy_query(query="shape",shape="ColumnDataType")`: STRING, INTEGER,
/// DECIMAL, DATETIME; `ColumnDataSubType`: FLOAT, FIXED), so a row naming one of them
/// names a value the service admits.
fn column(id: &str, name: &str, expression: &str, kind: Option<&str>, sub: Option<&str>) -> Value {
    let mut column = json!({ "ColumnId": id, "ColumnName": name, "Expression": expression });
    if let Some(kind) = kind {
        column["Type"] = json!(kind);
    }
    if let Some(sub) = sub {
        column["SubType"] = json!(sub);
    }
    column
}

/// `count` distinct columns, for the rows about how many the list holds.
fn columns(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            column(
                &format!("calc-{index}"),
                &format!("Calculated {index}"),
                &format!("sum({{revenue}}) + {index}"),
                Some("DECIMAL"),
                Some("FLOAT"),
            )
        })
        .collect()
}

/// One row of the modelled half: the list a create submits, and what the describe owes.
struct ColumnRow {
    case: &'static str,
    account: &'static str,
    /// The `UnmaterializedCalculatedColumns` member the create carries, or `None` for a
    /// create that carries the member not at all.
    submitted: Option<Vec<Value>>,
}

/// The rows: the member absent, an empty list, one column with only the required members,
/// several columns differing in the optional ones, and the published maximum.
fn column_rows() -> Vec<ColumnRow> {
    vec![
        ColumnRow {
            case: "the member not submitted at all",
            account: ACCOUNT,
            submitted: None,
        },
        ColumnRow {
            case: "an empty list, the published minimum",
            account: ACCOUNT,
            submitted: Some(Vec::new()),
        },
        ColumnRow {
            case: "one column carrying only its required members",
            account: ACCOUNT,
            submitted: Some(vec![column(
                "margin",
                "Margin",
                "{revenue} - {cost}",
                None,
                None,
            )]),
        },
        ColumnRow {
            case: "three columns differing in their optional members",
            account: OTHER_ACCOUNT,
            submitted: Some(vec![
                column("label", "Label", "concat({region}, '-eu')", Some("STRING"), None),
                column("units", "Units", "sum({quantity})", Some("INTEGER"), None),
                column(
                    "rate",
                    "Rate",
                    "sum({revenue}) / sum({quantity})",
                    Some("DECIMAL"),
                    Some("FIXED"),
                ),
            ]),
        },
        ColumnRow {
            case: "the published maximum of columns",
            account: ACCOUNT,
            submitted: Some(columns(MAX_COLUMNS)),
        },
    ]
}

#[tokio::test]
async fn every_submitted_calculated_column_list_comes_back_from_the_describe() {
    let router = emulator();

    for (index, row) in column_rows().into_iter().enumerate() {
        let ColumnRow {
            case,
            account,
            submitted,
        } = row;
        let data_set_id = format!("calc-columns-{index}");
        let source_arn = data_source_fixture(&router, account, &format!("source-{index}")).await;

        let mut create = json!({
            "DataSetId": data_set_id,
            "Name": "calculated columns",
            "ImportMode": "DIRECT_QUERY",
            "PhysicalTableMap": relational_table_map(&source_arn),
        });
        if let Some(columns) = &submitted {
            create["UnmaterializedCalculatedColumns"] = json!(columns);
        }
        let (status, body) = call(
            &router,
            "POST",
            &format!("/accounts/{account}/data-sets"),
            Some(&create),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with {body}"
        );

        let (status, body) = call(
            &router,
            "GET",
            &format!("/accounts/{account}/data-sets/{data_set_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        let reported = &body["DataSet"]["UnmaterializedCalculatedColumns"];
        // A create that submitted nothing has nothing to report; one that submitted a
        // list — the empty list included — is reported with exactly that list, in the
        // order it was submitted.
        let expected = match &submitted {
            None => Value::Null,
            Some(columns) => json!(columns),
        };
        assert_eq!(
            *reported, expected,
            "case {case:?}: the describe must report the submitted calculated columns, got \
             {reported} in {body}"
        );
    }
}

#[tokio::test]
async fn a_calculated_column_list_past_its_published_maximum_is_refused() {
    let router = emulator();
    let source_arn = data_source_fixture(&router, ACCOUNT, "source-past-the-bound").await;
    let data_set_id = "calc-columns-past-the-bound";

    let (status, body) = call(
        &router,
        "POST",
        &format!("/accounts/{ACCOUNT}/data-sets"),
        Some(&json!({
            "DataSetId": data_set_id,
            "Name": "calculated columns",
            "ImportMode": "DIRECT_QUERY",
            "PhysicalTableMap": relational_table_map(&source_arn),
            "UnmaterializedCalculatedColumns": columns(MAX_COLUMNS + 1),
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the published maximum: expected 400, got {status} with {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationException",
        "case one past the published maximum: expected ValidationException, got {code} in {body}"
    );

    // The refusal stored nothing: the data set the refused create named does not exist.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/accounts/{ACCOUNT}/data-sets/{data_set_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case one past the published maximum: the refused data set must not exist, got {status} \
         with {body}"
    );
}

/// The ARN of an IAM policy assignment, as the service reports it: the account that owns
/// it and the name that identifies it there, with no Region segment.
fn assignment_arn(account: &str, assignment_name: &str) -> String {
    format!("arn:aws:quicksight::{account}:assignment/{assignment_name}")
}

#[tokio::test]
async fn every_iam_policy_assignment_response_reports_the_assignments_own_arn() {
    let router = emulator();

    // The rows vary the account, the namespace and the name's punctuation — what the rule
    // generalises over; the ARN under test is derived from the first and the last of them.
    let rows: [(&str, &str, &str); 3] = [
        ("the default namespace", ACCOUNT, DEFAULT_NAMESPACE),
        ("another namespace", ACCOUNT, OTHER_NAMESPACE),
        ("another account", OTHER_ACCOUNT, DEFAULT_NAMESPACE),
    ];

    for (case, account, namespace) in rows {
        let name = "Rule_9:tier=gold+eu@team";
        let arn = json!(assignment_arn(account, name));

        let (status, body) = call(
            &router,
            "POST",
            &format!("/accounts/{account}/namespaces/{namespace}/iam-policy-assignments"),
            Some(&json!({
                "AssignmentName": name,
                "AssignmentStatus": "ENABLED",
                "PolicyArn": READ_ONLY_POLICY,
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: create expected 201, got {status} with {body}"
        );
        assert_eq!(
            body["Arn"], arn,
            "case {case:?}: the create must report the assignment's own ARN, got {body}"
        );

        let (status, body) = call(
            &router,
            "GET",
            &format!("/accounts/{account}/namespaces/{namespace}/iam-policy-assignments/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["IAMPolicyAssignment"]["Arn"], arn,
            "case {case:?}: the describe must report the assignment's own ARN, got {body}"
        );

        let (status, body) = call(
            &router,
            "PUT",
            &format!("/accounts/{account}/namespaces/{namespace}/iam-policy-assignments/{name}"),
            Some(&json!({ "AssignmentStatus": "DISABLED" })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Arn"], arn,
            "case {case:?}: the update must report the assignment's own ARN, got {body}"
        );

        // `DeleteIAMPolicyAssignment` is bound to `/namespace/` rather than
        // `/namespaces/` (`smithy_query(query="operation",operation="DeleteIAMPolicyAssignment")`).
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/accounts/{account}/namespace/{namespace}/iam-policy-assignments/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Arn"], arn,
            "case {case:?}: the delete must report the ARN of the assignment it removed, got \
             {body}"
        );
    }
}
