// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests pinning how a MODELLED INPUT CONSTRAINT is rendered into the
//! message of the `ValidationException` the centralized request walk answers before any
//! handler runs.
//!
//! Three rules, none of them about one operation or one member, so every row below is an
//! instance of its rule and none of them is the rule:
//!
//! 1. a `@pattern` requirement is quoted as the expression the service PUBLISHES — the
//!    model's text with the whole-value `^` / `$` pair the model wraps it in dropped —
//!    because the message is where a caller reads the expression its value had to satisfy,
//!    and the pair is how Smithy turns a find into a whole-string match rather than part of
//!    the character grammar;
//! 2. the clause of a parameter the READ/WRITE DATA PLANE reads off the request line — the
//!    query parameters of the operations this model serves on its `storage-` endpoint —
//!    names the member and the requirement and NO value, whatever constraint class was
//!    broken, while every control-plane parameter keeps quoting the value the request
//!    carried, whatever class IT broke and whichever binding carried it;
//! 3. an enumerated type's admissible values are listed in the order the front door walks
//!    its own table of them — a table keyed on each value's own text — and not in the order
//!    the model declares them, over the value set the SERVICE publishes rather than the one
//!    the shipped schema carries. Both are properties of the enumerated TYPE: one type is
//!    listed alike in every operation, at every depth and in whatever binding carried the
//!    value.
//!
//! The rows therefore vary along everything each rule generalises over: the operation, the
//! member, the binding the value arrived in (`@httpLabel`, `@httpQuery`, a body member and a
//! member of a nested structure), the expression's own shape, the constraint class, which
//! end of a range was broken, and which enumerated type was named — including operations,
//! types and members the issue that prompted these tests never cited. The envelope and the
//! clause frame are written out here rather than read from the emulator's own helpers, so
//! the test pins the wording independently of the code that produces it.
//!
//! Two groups of rows are the RULE's consequence rather than a case any recording of this
//! API covers, and are pinned so the rule's boundary is asserted and not merely described:
//! `ListRuns.maxResults` (a parameter of the CONTROL plane's own query string, which keeps
//! quoting the value) and `ListReadSetUploadParts.partSource` (the one member NAME the two
//! planes share, which a refusal's member path cannot tell apart, so it is stated the data
//! plane's way wherever it is reached).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::private_workflow_runs::{call, emulator, error_code, start_run_body};
use crate::read_set_upload_parts::{get_read_set, upload_part};

/// A sequence store / read set / reference / upload id of the modelled form (10..36 digits)
/// that nothing in an empty emulator carries: the constraint walk refuses a bad request
/// before any handler looks anything up, so a refusal row needs no fixture at all.
const WELL_FORMED_ID: &str = "1234567890";

/// A workflow id of the modelled form (`^[0-9]+$`, 1..18) for the same reason.
const WELL_FORMED_WORKFLOW_ID: &str = "1";

/// The published bounds of the modelled `partNumber` range. Source: the `partNumber` member
/// of `GetReadSetRequest`, `GetReferenceRequest` and `UploadReadSetPartRequest` — `Integer
/// @range(min 1, max 10000)`.
const PART_NUMBER_RANGE: (i32, i32) = (1, 10_000);

/// The published bounds of the modelled `priority` / `storageCapacity` range. Source: those
/// members of `StartRunRequest` and of `DefaultRunSetting` — `Integer @range(min 0, max
/// 100000)`.
const RUN_CAPACITY_RANGE: (i64, i64) = (0, 100_000);

/// The published bounds of the modelled page size. Source: the `maxResults` member of
/// `ListRunsRequest` — `Integer @httpQuery("maxResults") @range(min 1, max 100)`.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The one-violation envelope AWS states a constraint refusal in.
fn envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause for a parameter whose value the refusal QUOTES back.
fn quoted_clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause for a parameter the refusal states NO value of: the member path and the
/// requirement alone.
fn valueless_clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// `Member must have value greater than or equal to <min>`, over the model's own bound.
fn at_least(min: impl std::fmt::Display) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// `Member must have value less than or equal to <max>`, over the model's own bound.
fn at_most(max: impl std::fmt::Display) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// `Member must satisfy enum value set: [ … ]`, over the values as the service lists them.
fn one_of(values: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}

/// Assert one refusal: HTTP 400, the modelled error shape, and the exact message. Answers
/// the message it read so a caller can make further assertions about it.
fn assert_refusal(case: &str, status: u16, body: &Value, expected: &str) -> String {
    assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: refusal carried no message, got {body:?}"))
        .to_string();
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    message
}

// ---------------------------------------------------------------------------------------
// The requests each rule's rows are made with.
// ---------------------------------------------------------------------------------------

/// `GET /sequencestore/{store}/readset/{id}?partNumber=<n>[&file=<file>]`, as the refusal
/// document alone — the success of this operation is a payload of bytes rather than JSON.
async fn read_set_part(
    router: &MultiServiceRouter,
    file: Option<&str>,
    part_number: i32,
) -> (u16, Value) {
    let (status, _payload, body) =
        get_read_set(router, WELL_FORMED_ID, WELL_FORMED_ID, file, part_number).await;
    (status, body)
}

/// `GET /referencestore/{store}/reference/{id}?partNumber=<n>` — the other read of the data
/// plane, which binds the same two query parameters.
async fn reference_part(router: &MultiServiceRouter, part_number: i32) -> (u16, Value) {
    let uri = format!(
        "/referencestore/{WELL_FORMED_ID}/reference/{WELL_FORMED_ID}?partNumber={part_number}"
    );
    call(router, "GET", &uri, None).await
}

/// `PUT /sequencestore/{store}/upload/{upload}/part?partSource=<source>&partNumber=<n>`.
async fn uploaded_part(
    router: &MultiServiceRouter,
    part_source: &str,
    part_number: i32,
) -> (u16, Value) {
    upload_part(
        router,
        WELL_FORMED_ID,
        WELL_FORMED_ID,
        part_source,
        part_number,
        b"the part's bytes",
    )
    .await
}

/// `POST /run` with one extra member, every other required member well-formed.
async fn started_run(router: &MultiServiceRouter, request_id: &str, extra: &Value) -> (u16, Value) {
    let body = start_run_body(WELL_FORMED_WORKFLOW_ID, request_id, extra);
    call(router, "POST", "/run", Some(&body)).await
}

/// `POST /runBatch` with `defaultRunSetting` carrying one extra member.
async fn started_run_batch(
    router: &MultiServiceRouter,
    request_id: &str,
    default_run_setting_extra: &Value,
) -> (u16, Value) {
    let mut setting = json!({
        "workflowId": WELL_FORMED_WORKFLOW_ID,
        "roleArn": "arn:aws:iam::000000000000:role/OmicsWorkflowRole",
    });
    let members = setting.as_object_mut().expect("the setting is an object");
    for (member, value) in default_run_setting_extra
        .as_object()
        .expect("extra members are an object")
    {
        members.insert(member.clone(), value.clone());
    }
    let body = json!({
        "requestId": request_id,
        "defaultRunSetting": setting,
        "batchRunSettings": { "s3UriSettings": "s3://omics-emulator-bucket/settings.json" },
    });
    call(router, "POST", "/runBatch", Some(&body)).await
}

/// `GET /run?maxResults=<n>` — a page size the CONTROL plane reads off the query string.
async fn listed_runs(router: &MultiServiceRouter, max_results: i64) -> (u16, Value) {
    call(router, "GET", &format!("/run?maxResults={max_results}"), None).await
}

/// `POST /sequencestore/{store}/upload/{upload}/parts` with `partSource` as a BODY member —
/// the control plane's listing of an upload's parts, the one operation that declares a
/// member name the data plane also binds to its query string.
async fn listed_upload_parts(
    router: &MultiServiceRouter,
    part_source: &str,
) -> (u16, Value) {
    let uri = format!("/sequencestore/{WELL_FORMED_ID}/upload/{WELL_FORMED_ID}/parts");
    call(router, "POST", &uri, Some(&json!({ "partSource": part_source }))).await
}

// ---------------------------------------------------------------------------------------
// Rule 1: the expression a `@pattern` requirement quotes.
// ---------------------------------------------------------------------------------------

/// One row of the `@pattern` table.
struct PatternCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The value the request carried, which the clause quotes back.
    value: &'static str,
    /// The dotted member path the offending member sits at.
    path: &'static str,
    /// The expression as the model DECLARES it, which must not reach the message.
    declared: &'static str,
    /// The expression as the service publishes it: `declared` without the pair.
    published: &'static str,
}

/// The table: the bindings this service refuses a `@pattern` value in, over expressions of
/// three different shapes. The declared text of each is from
/// `smithy_query(query="operation", operation="<op>")` on this crate's shipped model.
fn pattern_cases() -> Vec<PatternCase> {
    vec![
        PatternCase {
            case: "CancelRun, the run id on the path",
            method: "POST",
            uri: "/run/abc123/cancel".to_string(),
            body: None,
            value: "abc123",
            path: "id",
            declared: "^[0-9]+$",
            published: "[0-9]+",
        },
        PatternCase {
            case: "GetRun, the run id on the path",
            method: "GET",
            uri: "/run/abc123".to_string(),
            body: None,
            value: "abc123",
            path: "id",
            declared: "^[0-9]+$",
            published: "[0-9]+",
        },
        PatternCase {
            case: "DeleteRun, the run id on the path",
            method: "DELETE",
            uri: "/run/0x10".to_string(),
            body: None,
            value: "0x10",
            path: "id",
            declared: "^[0-9]+$",
            published: "[0-9]+",
        },
        PatternCase {
            case: "CancelRunBatch, the batch id in the body",
            method: "POST",
            uri: "/runBatch/cancel".to_string(),
            body: Some(json!({ "batchId": "batch-abc" })),
            value: "batch-abc",
            path: "batchId",
            declared: "^[0-9]+$",
            published: "[0-9]+",
        },
        PatternCase {
            case: "GetBatch, the batch id on the path",
            method: "GET",
            uri: "/runBatch/not-a-number".to_string(),
            body: None,
            value: "not-a-number",
            path: "batchId",
            declared: "^[0-9]+$",
            published: "[0-9]+",
        },
        PatternCase {
            case: "StartRun, a role arn that is not an arn",
            method: "POST",
            uri: "/run".to_string(),
            body: Some(start_run_body(
                WELL_FORMED_WORKFLOW_ID,
                "pattern-role-arn",
                &json!({ "roleArn": "not-an-arn" }),
            )),
            value: "not-an-arn",
            path: "roleArn",
            declared: "^arn:.+$",
            published: "arn:.+",
        },
        PatternCase {
            case: "StartRun, an account id of the wrong width",
            method: "POST",
            uri: "/run".to_string(),
            body: Some(start_run_body(
                WELL_FORMED_WORKFLOW_ID,
                "pattern-owner-id",
                &json!({ "workflowOwnerId": "12345" }),
            )),
            value: "12345",
            path: "workflowOwnerId",
            declared: "^[0-9]{12}$",
            published: "[0-9]{12}",
        },
        PatternCase {
            case: "StartRun, a version name opening with a separator",
            method: "POST",
            uri: "/run".to_string(),
            body: Some(start_run_body(
                WELL_FORMED_WORKFLOW_ID,
                "pattern-version-name",
                &json!({ "workflowVersionName": "-leading-dash" }),
            )),
            value: "-leading-dash",
            path: "workflowVersionName",
            declared: r"^[A-Za-z0-9][A-Za-z0-9\-\._]*$",
            published: r"[A-Za-z0-9][A-Za-z0-9\-\._]*",
        },
    ]
}

#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_whole_value_anchors() {
    let router = emulator();
    for row in pattern_cases() {
        let (status, body) = call(&router, row.method, &row.uri, row.body.as_ref()).await;
        let expected = envelope(&quoted_clause(
            row.value,
            row.path,
            &format!(
                "Member must satisfy regular expression pattern: {}",
                row.published
            ),
        ));
        let message = assert_refusal(row.case, status, &body, &expected);
        assert!(
            !message.contains(row.declared),
            "case {:?}: the model's anchored text {:?} must not reach the message, got {message:?}",
            row.case,
            row.declared
        );
    }
}

// ---------------------------------------------------------------------------------------
// Rule 2: which clauses state the value the request carried.
// ---------------------------------------------------------------------------------------

/// One row of the value-half table: the clause's member and requirement, the value the
/// clause is expected to quote (`None` for a clause that states none), and the answer.
struct ValueHalfCase {
    case: &'static str,
    path: &'static str,
    requirement: String,
    /// `Some(value)` when the clause quotes the value the request carried.
    quoted: Option<&'static str>,
    answer: (u16, Value),
}

#[tokio::test]
async fn the_value_half_of_a_clause_follows_where_the_service_read_the_value() {
    let router = emulator();
    let (first_part, last_part) = PART_NUMBER_RANGE;
    let (least_capacity, most_capacity) = RUN_CAPACITY_RANGE;
    let (least_page, most_page) = MAX_RESULTS_RANGE;
    let rows = vec![
        // The data plane's own query parameters: the member and the requirement, no value —
        // across both of its reads, its write, both ends of a range and an enumerated type.
        ValueHalfCase {
            case: "GetReadSet, a part number below the range",
            path: "partNumber",
            requirement: at_least(first_part),
            quoted: None,
            answer: read_set_part(&router, None, first_part - 1).await,
        },
        ValueHalfCase {
            case: "GetReadSet, a part number above the range",
            path: "partNumber",
            requirement: at_most(last_part),
            quoted: None,
            answer: read_set_part(&router, None, last_part + 1).await,
        },
        ValueHalfCase {
            case: "GetReadSet, a file no read set has",
            path: "file",
            requirement: one_of(&["INDEX", "SOURCE1", "SOURCE2"]),
            quoted: None,
            answer: read_set_part(&router, Some("SOURCE3"), first_part).await,
        },
        ValueHalfCase {
            case: "GetReference, a part number below the range",
            path: "partNumber",
            requirement: at_least(first_part),
            quoted: None,
            answer: reference_part(&router, first_part - 1).await,
        },
        ValueHalfCase {
            case: "UploadReadSetPart, a part number below the range",
            path: "partNumber",
            requirement: at_least(first_part),
            quoted: None,
            answer: uploaded_part(&router, "SOURCE1", first_part - 1).await,
        },
        ValueHalfCase {
            case: "UploadReadSetPart, a part number above the range",
            path: "partNumber",
            requirement: at_most(last_part),
            quoted: None,
            answer: uploaded_part(&router, "SOURCE1", last_part + 1).await,
        },
        ValueHalfCase {
            case: "UploadReadSetPart, a part source no upload takes",
            path: "partSource",
            requirement: one_of(&["SOURCE1", "SOURCE2"]),
            quoted: None,
            answer: uploaded_part(&router, "SOURCE3", first_part).await,
        },
        // The control plane quotes the value whatever carried it: a body member, a member of
        // a nested structure, and a parameter of its OWN query string.
        ValueHalfCase {
            case: "StartRun, a priority below the range",
            path: "priority",
            requirement: at_least(least_capacity),
            quoted: Some("-1"),
            answer: started_run(
                &router,
                "value-priority-low",
                &json!({ "priority": least_capacity - 1 }),
            )
            .await,
        },
        ValueHalfCase {
            case: "StartRun, a priority above the range",
            path: "priority",
            requirement: at_most(most_capacity),
            quoted: Some("100001"),
            answer: started_run(
                &router,
                "value-priority-high",
                &json!({ "priority": most_capacity + 1 }),
            )
            .await,
        },
        ValueHalfCase {
            case: "StartRun, a storage capacity above the range",
            path: "storageCapacity",
            requirement: at_most(most_capacity),
            quoted: Some("100001"),
            answer: started_run(
                &router,
                "value-capacity-high",
                &json!({ "storageCapacity": most_capacity + 1 }),
            )
            .await,
        },
        ValueHalfCase {
            case: "StartRunBatch, a priority below the range inside the nested setting",
            path: "defaultRunSetting.priority",
            requirement: at_least(least_capacity),
            quoted: Some("-1"),
            answer: started_run_batch(
                &router,
                "value-nested-priority",
                &json!({ "priority": least_capacity - 1 }),
            )
            .await,
        },
        ValueHalfCase {
            case: "ListRuns, a page size below the range on the control plane's query string",
            path: "maxResults",
            requirement: at_least(least_page),
            quoted: Some("0"),
            answer: listed_runs(&router, least_page - 1).await,
        },
        ValueHalfCase {
            case: "ListRuns, a page size above the range on the control plane's query string",
            path: "maxResults",
            requirement: at_most(most_page),
            quoted: Some("101"),
            answer: listed_runs(&router, most_page + 1).await,
        },
        // The one member NAME the two planes share. A refusal carries only the member path,
        // so this one cannot be told apart from the data plane's query parameter of the
        // same name and is stated the data plane's way: pinned here so the deviation from
        // the rule's own words is deliberate and visible rather than discovered.
        ValueHalfCase {
            case: "ListReadSetUploadParts, a part source no upload takes, as a body member",
            path: "partSource",
            requirement: one_of(&["SOURCE1", "SOURCE2"]),
            quoted: None,
            answer: listed_upload_parts(&router, "SOURCE3").await,
        },
    ];

    for row in rows {
        let (status, body) = row.answer;
        let expected = match row.quoted {
            Some(value) => envelope(&quoted_clause(value, row.path, &row.requirement)),
            None => envelope(&valueless_clause(row.path, &row.requirement)),
        };
        let message = assert_refusal(row.case, status, &body, &expected);
        if row.quoted.is_none() {
            assert!(
                !message.contains("Value '"),
                "case {:?}: a data-plane parameter's clause quotes no value, got {message:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_value_on_either_published_bound_of_a_range_is_admitted() {
    let router = emulator();
    let (first_part, last_part) = PART_NUMBER_RANGE;
    let (least_capacity, most_capacity) = RUN_CAPACITY_RANGE;
    let (least_page, most_page) = MAX_RESULTS_RANGE;
    // Each published bound is INSIDE the range, so whatever the answer is about, it is not
    // the bound: the refusal that one past the bound produces above must not appear here.
    let rows = vec![
        (
            "GetReadSet, the bottom of the part-number range",
            "partNumber",
            read_set_part(&router, None, first_part).await,
        ),
        (
            "GetReadSet, the top of the part-number range",
            "partNumber",
            read_set_part(&router, None, last_part).await,
        ),
        (
            "UploadReadSetPart, the bottom of the part-number range",
            "partNumber",
            uploaded_part(&router, "SOURCE1", first_part).await,
        ),
        (
            "UploadReadSetPart, the top of the part-number range",
            "partNumber",
            uploaded_part(&router, "SOURCE1", last_part).await,
        ),
        (
            "StartRun, the bottom of the priority range",
            "priority",
            started_run(
                &router,
                "bound-priority-low",
                &json!({ "priority": least_capacity }),
            )
            .await,
        ),
        (
            "StartRun, the top of the priority range",
            "priority",
            started_run(
                &router,
                "bound-priority-high",
                &json!({ "priority": most_capacity }),
            )
            .await,
        ),
        (
            "ListRuns, the bottom of the page-size range",
            "maxResults",
            listed_runs(&router, least_page).await,
        ),
        (
            "ListRuns, the top of the page-size range",
            "maxResults",
            listed_runs(&router, most_page).await,
        ),
    ];

    for (case, member, (status, body)) in rows {
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let range_clause =
            format!("at '{member}' failed to satisfy constraint: Member must have value");
        assert!(
            !message.contains(&range_clause),
            "case {case:?}: a value on a published bound satisfies the range, so {member} must \
             not be refused for it, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Rule 3: the value set an enum clause names, and the order it lists it in.
// ---------------------------------------------------------------------------------------

/// One row of the enumerated-value-set table.
struct EnumCase {
    case: &'static str,
    path: &'static str,
    /// The value the request carried, which this clause quotes back.
    value: &'static str,
    /// The values the type admits, in the order the model DECLARES them.
    declared: &'static [&'static str],
    /// The values the service names, in the order it lists them.
    listed: &'static [&'static str],
    answer: (u16, Value),
}

#[tokio::test]
async fn an_enum_value_set_is_the_published_one_in_the_order_the_service_lists_it() {
    let router = emulator();
    // The declared sets below are from `smithy_query(query="shape", shape="<Enum>")` on this
    // crate's shipped model. The listed set of `RunLogLevel` and of `StorageType` is the one
    // a recorded refusal of this API shows — `StorageType` naming a third value its shipped
    // schema does not declare; the others follow from the same derivation over their own
    // values.
    let rows = vec![
        EnumCase {
            case: "StartRunBatch, a log level inside the nested setting",
            path: "defaultRunSetting.logLevel",
            value: "VERBOSE",
            declared: &["OFF", "FATAL", "ERROR", "ALL"],
            listed: &["ALL", "FATAL", "ERROR", "OFF"],
            answer: started_run_batch(
                &router,
                "enum-nested-log-level",
                &json!({ "logLevel": "VERBOSE" }),
            )
            .await,
        },
        EnumCase {
            case: "StartRun, a log level as a top-level body member",
            path: "logLevel",
            value: "VERBOSE",
            declared: &["OFF", "FATAL", "ERROR", "ALL"],
            listed: &["ALL", "FATAL", "ERROR", "OFF"],
            answer: started_run(&router, "enum-log-level", &json!({ "logLevel": "VERBOSE" })).await,
        },
        EnumCase {
            case: "StartRunBatch, a storage type inside the nested setting",
            path: "defaultRunSetting.storageType",
            value: "EPHEMERAL",
            declared: &["STATIC", "DYNAMIC"],
            listed: &["S3", "STATIC", "DYNAMIC"],
            answer: started_run_batch(
                &router,
                "enum-nested-storage-type",
                &json!({ "storageType": "EPHEMERAL" }),
            )
            .await,
        },
        EnumCase {
            case: "StartRun, a storage type as a top-level body member",
            path: "storageType",
            value: "EPHEMERAL",
            declared: &["STATIC", "DYNAMIC"],
            listed: &["S3", "STATIC", "DYNAMIC"],
            answer: started_run(
                &router,
                "enum-storage-type",
                &json!({ "storageType": "EPHEMERAL" }),
            )
            .await,
        },
        EnumCase {
            case: "StartRun, a retention mode as a top-level body member",
            path: "retentionMode",
            value: "KEEP",
            declared: &["RETAIN", "REMOVE"],
            listed: &["REMOVE", "RETAIN"],
            answer: started_run(
                &router,
                "enum-retention-mode",
                &json!({ "retentionMode": "KEEP" }),
            )
            .await,
        },
        EnumCase {
            case: "ListRuns, a run status on the control plane's query string",
            path: "status",
            value: "PAUSED",
            declared: &[
                "PENDING",
                "STARTING",
                "RUNNING",
                "STOPPING",
                "COMPLETED",
                "DELETED",
                "CANCELLED",
                "FAILED",
            ],
            listed: &[
                "STARTING",
                "COMPLETED",
                "CANCELLED",
                "STOPPING",
                "PENDING",
                "DELETED",
                "FAILED",
                "RUNNING",
            ],
            answer: call(&router, "GET", "/run?status=PAUSED", None).await,
        },
    ];

    for row in rows {
        let (status, body) = row.answer;
        let expected = envelope(&quoted_clause(row.value, row.path, &one_of(row.listed)));
        let message = assert_refusal(row.case, status, &body, &expected);
        // Listing the values in the model's own order is what this rule exists to replace,
        // so a type the two orders disagree on must not be stated that way.
        let in_declaration_order = format!("[{}]", row.declared.join(", "));
        if row.declared != row.listed {
            assert!(
                !message.contains(&in_declaration_order),
                "case {:?}: the model's declaration order {in_declaration_order:?} must not \
                 reach the message, got {message:?}",
                row.case
            );
        }
    }
}
