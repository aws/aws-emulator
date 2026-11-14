// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request that names no user pool to work on, over the wire,
//! driven exactly as an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule: every operation that works ON a stored user pool takes that pool's id
//! (`UserPoolId`, `@required` wherever it appears —
//! `smithy_query(query="operation",operation="DescribeUserPoolClient")`), and the pool a
//! request addresses is resolved before anything else about the request is read. A request
//! carrying no pool id at all is therefore refused by the null check that resolution opens
//! with: HTTP 400 `InvalidParameterException` whose message names the member as the MODEL
//! names it and says it must not be null — not the constraint report that quotes a value and
//! the constraint it failed. A pool id the request DID supply keeps that report, and so does
//! every other member, present or absent.
//!
//! The rows span what the rule generalises over: the operation (a read of the pool, a read
//! of one of its app clients, an administrative read of a user, a listing, a write of pool
//! configuration, a write that mints a resource, a start of a stored job) and what the
//! request carries BESIDE the pool id (nothing, a well-formed member, a member that breaks
//! its own rule, another absent required member). The recorded case —
//! `DescribeUserPoolClient` reaching this endpoint without its `UserPoolId` — is one row of
//! it.
//!
//! The second table is the rule's boundary: a pool id that was supplied (outside the form
//! the model declares, or the empty string) and an absent member that is NOT the pool id
//! each keep the framework's constraint report.
//!
//! The harness is the one `log_delivery_configuration.rs` builds for this service, reached
//! through the test crate root rather than copied.

use serde_json::{json, Value};

use super::log_delivery_configuration::{call, error_code, error_message, ok, pool, router};

/// The error code every refusal of a bad parameter of this service carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The sentence a request that named no user pool earns: the member as the model names it,
/// then that it must not be null.
const MUST_NOT_BE_NULL: &str = "UserPoolId must not be null";

/// A `ClientId` within `@length` 1..128 and inside the `^[\w+]+$` the model declares for it
/// (`smithy_query(query="shape",shape="ClientIdType")`).
const WELL_FORMED_CLIENT_ID: &str = "1h57kf5cpq17m0eml12EXAMPLE";

/// A `JobId` inside the `^import-[0-9a-zA-Z-]+$` the model declares for one
/// (`smithy_query(query="shape",shape="UserImportJobIdType")`).
const WELL_FORMED_JOB_ID: &str = "import-00000000-0000-4000-8000-000000000000";

/// The role a user import job writes its log stream with.
const IMPORT_ROLE: &str = "arn:aws:iam::000000000000:role/cognito-import";

/// A CloudWatch Logs log group a user pool exports its logs to.
const LOG_GROUP: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/pool";

/// The `@pattern` a pool id has to satisfy, as this service's clause quotes one: the modelled
/// expression (`UserPoolIdType`, `^[\w-]+_[0-9a-zA-Z]+$`) without the whole-value anchors.
const POOL_ID_PATTERN: &str = r"[\w-]+_[0-9a-zA-Z]+";

/// One row of the null-check table: an operation, and the members its request carries
/// BESIDE the pool id.
struct Row {
    case: &'static str,
    operation: &'static str,
    members: Value,
}

/// The envelope the framework states one broken constraint in, over its clause.
fn one_validation_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a member carrying a value earns.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause an ABSENT member earns: there was no value to quote.
fn absent_clause(path: &str) -> String {
    format!("Value null at '{path}' failed to satisfy constraint: Member must not be null")
}

#[tokio::test]
async fn a_request_naming_no_user_pool_is_refused_by_the_pool_id_null_check() {
    let router = router();
    // A real pool, so the listing read at the end of this test reports what the refused
    // writes left behind: nothing.
    let user_pool_id = pool(&router, "a pool the refused writes must not reach", None).await;

    // Nine operations spanning the classes that address a pool: its own read, a read of one
    // of its app clients (the recorded case), an administrative read of a user, a listing, a
    // read of its import header, a write of pool configuration, two writes that mint a
    // resource, and a start of a stored job — then the same read with its other member absent
    // too, and with that member breaking its own rule. Each row carries every OTHER member
    // its request declares, so the pool id is the only thing missing.
    let rows = [
        Row {
            case: "a read of the pool itself",
            operation: "DescribeUserPool",
            members: json!({}),
        },
        Row {
            case: "a read of one of its app clients",
            operation: "DescribeUserPoolClient",
            members: json!({ "ClientId": WELL_FORMED_CLIENT_ID }),
        },
        Row {
            case: "an administrative read of a user",
            operation: "AdminGetUser",
            members: json!({ "Username": "ada" }),
        },
        Row {
            case: "a listing of the pool's users",
            operation: "ListUsers",
            members: json!({ "Limit": 10 }),
        },
        Row {
            case: "a read of the pool's import header",
            operation: "GetCSVHeader",
            members: json!({}),
        },
        Row {
            case: "a write of the pool's log delivery configuration",
            operation: "SetLogDeliveryConfiguration",
            members: json!({
                "LogConfigurations": [{
                    "LogLevel": "ERROR",
                    "EventSource": "userNotification",
                    "CloudWatchLogsConfiguration": { "LogGroupArn": LOG_GROUP },
                }],
            }),
        },
        Row {
            case: "a write that mints an app client",
            operation: "CreateUserPoolClient",
            members: json!({ "ClientName": "orphan-app" }),
        },
        Row {
            case: "a write that mints a user import job",
            operation: "CreateUserImportJob",
            members: json!({ "JobName": "orphan-import", "CloudWatchLogsRoleArn": IMPORT_ROLE }),
        },
        Row {
            case: "a start of a stored user import job",
            operation: "StartUserImportJob",
            members: json!({ "JobId": WELL_FORMED_JOB_ID }),
        },
        Row {
            case: "a read whose other required member is absent too",
            operation: "DescribeUserPoolClient",
            members: json!({}),
        },
        Row {
            case: "a read whose other member breaks its own pattern",
            operation: "DescribeUserPoolClient",
            members: json!({ "ClientId": "not a client id !!" }),
        },
    ];

    for row in rows {
        let case = row.case;
        assert!(
            row.members.get("UserPoolId").is_none(),
            "case {case:?}: the row's request must carry no pool id, got {}",
            row.members
        );
        let (status, payload) = call(&router, row.operation, &row.members).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, INVALID_PARAMETER, MUST_NOT_BE_NULL),
            "case {case:?}: {} must answer the null check's own sentence, got {status} \
             with {payload}",
            row.operation
        );
    }

    // Refused before anything was written: the pool the refused writes never named holds no
    // app client and no import job.
    let clients = ok(
        &router,
        "ListUserPoolClients",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        clients["UserPoolClients"],
        json!([]),
        "a refused write must mint no app client, got {clients}"
    );
    let jobs = ok(
        &router,
        "ListUserImportJobs",
        &json!({ "UserPoolId": user_pool_id, "MaxResults": 10 }),
    )
    .await;
    assert_eq!(
        jobs["UserImportJobs"],
        json!([]),
        "a refused write must mint no user import job, got {jobs}"
    );
}

/// One row of the boundary table: a request whose refusal keeps the framework's constraint
/// report, and the message it states.
struct ReportRow {
    case: &'static str,
    operation: &'static str,
    body: Value,
    expected: String,
}

#[tokio::test]
async fn a_pool_id_the_request_supplied_keeps_the_constraint_report() {
    let router = router();
    let user_pool_id = pool(&router, "a pool the boundary rows address", None).await;

    // The requirement half, as the framework words a `@pattern` class.
    let must_match = format!("Member must satisfy regular expression pattern: {POOL_ID_PATTERN}");

    let rows = vec![
        ReportRow {
            case: "a pool id outside the form the model declares",
            operation: "DescribeUserPool",
            body: json!({ "UserPoolId": "no-underscore-here" }),
            expected: one_validation_error(&clause(
                "no-underscore-here",
                "userPoolId",
                &must_match,
            )),
        },
        ReportRow {
            case: "an absent member that is not the pool id",
            operation: "DescribeUserPoolClient",
            body: json!({ "UserPoolId": user_pool_id.clone() }),
            expected: one_validation_error(&absent_clause("clientId")),
        },
        ReportRow {
            case: "an absent member that is not the pool id, on a write",
            operation: "CreateUserPoolClient",
            body: json!({ "UserPoolId": user_pool_id.clone() }),
            expected: one_validation_error(&absent_clause("clientName")),
        },
        ReportRow {
            case: "a supplied pool id beside a member outside its own pattern",
            operation: "DescribeUserPoolClient",
            body: json!({
                "UserPoolId": user_pool_id.clone(),
                "ClientId": "not a client id !!",
            }),
            expected: one_validation_error(&clause(
                "not a client id !!",
                "clientId",
                r"Member must satisfy regular expression pattern: [\w+]+",
            )),
        },
    ];

    for row in rows {
        let (status, payload) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, INVALID_PARAMETER, row.expected.as_str()),
            "case {:?}: expected the constraint report, got {status} with {payload}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_empty_pool_id_is_a_value_the_report_quotes_rather_than_an_absence() {
    let router = router();

    // The empty string is a value a request DID carry, so it is reported as one: it breaks
    // both of the member's constraints (`UserPoolIdType` is `@length` 1..55 and
    // `^[\w-]+_[0-9a-zA-Z]+$`), and the report quotes the value in a clause for each. WHICH
    // ORDER those two clauses are stated in is not asserted here: that is the subject of its
    // own recorded refusal, and this test is about the empty string not being read as an
    // absence.
    let (status, payload) = call(&router, "GetCSVHeader", &json!({ "UserPoolId": "" })).await;
    let message = error_message(&payload);
    assert_eq!(
        (status, error_code(&payload)),
        (400, INVALID_PARAMETER),
        "expected a 400 {INVALID_PARAMETER}, got {status} with {payload}"
    );
    assert!(
        message.starts_with("2 validation errors detected: "),
        "an empty pool id breaks both of the member's constraints, got {message:?}"
    );
    for requirement in [
        "Member must have length greater than or equal to 1",
        &format!("Member must satisfy regular expression pattern: {POOL_ID_PATTERN}"),
    ] {
        let stated = clause("", "userPoolId", requirement);
        assert!(
            message.contains(&stated),
            "case {requirement:?}: the report must state {stated:?}, got {message:?}"
        );
    }
    assert_ne!(
        message, MUST_NOT_BE_NULL,
        "an empty pool id is a value, not the absence the null check reports"
    );
}
