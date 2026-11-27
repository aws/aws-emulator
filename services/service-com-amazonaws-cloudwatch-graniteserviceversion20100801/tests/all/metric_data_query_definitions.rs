// SPDX-License-Identifier: CC0-1.0

//! The METRIC DATA QUERY DEFINITION store, end to end: the write that saves a metric query, the
//! reader that reports it, the listing that pages through the summaries and the delete that
//! removes one.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over — the query language, the form and the size of the path, the expression, the
//! Region the request is signed for and the page size asked for — rather than over one recorded
//! request:
//!
//! * **a definition is persisted and reported back as it was written**: a create mints the id,
//!   stamps both instants and answers the ARN of the resource; the reader reports every member
//!   the write left, expression included, and the listing reports the summary members without it.
//! * **an update replaces the path, the expression and the type and keeps the identity and the
//!   creation time** (`PutMetricDataQueryDefinition`: "An update replaces the path, expression,
//!   and type, and keeps the creation time.").
//! * **the path is unique in the account and Region**: a path ANOTHER definition already uses is
//!   refused as a conflict, whether the request creates or updates, while a definition keeping
//!   its own path is not.
//! * **the path must satisfy the requirements its member states in prose**: no leading or
//!   trailing slash, no empty folder name, at most 1,015 bytes encoded as UTF-8.
//! * **a definition belongs to the Region it was written in**: the same id and the same path in
//!   another Region name another Region's definition.
//! * **an id this Region holds no definition under is a not-found** for the reader and for an
//!   update, while the delete succeeds whether or not a definition was there to remove.
//! * **the listing pages through every definition of the Region exactly once, in path order**,
//!   and owes a token only while one is left over.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use crate::harness::{
    build_router, build_router_with_engine, call, call_in_region, error_code, error_message, ok,
    DEFAULT_REGION,
};

/// The account every ARN below carries, the one the emulator signs for.
fn account() -> String {
    ::engine::emulator_account_id().to_string()
}

/// A second Region, for the rows that assert a definition belongs to the Region it was written
/// in.
const OTHER_REGION: &str = "eu-west-1";

/// The ARN a definition of `region` with the id `id` is reported by: "The resource ARN has the
/// format arn:aws:cloudwatch:Region:account-id:metric-query-definition/Id"
/// (`PutMetricDataQueryDefinition`).
fn expected_arn(region: &str, id: &str) -> String {
    format!(
        "arn:aws:cloudwatch:{region}:{}:metric-query-definition/{id}",
        account()
    )
}

/// Whether `id` is a `DefinitionId`: 1..64 characters of letters, digits and hyphens, the
/// modelled constraint of the member
/// (`smithy_query(query="shape",shape="DefinitionId")`, length 1..64, pattern
/// `^[a-zA-Z0-9-]{1,64}$`).
fn is_definition_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

/// One definition a row writes: its path, its expression and the language that expression is in.
struct Row {
    /// What this row is, named in every assertion.
    case: &'static str,
    /// The path the definition takes.
    path: String,
    /// The expression it saves.
    expression: String,
    /// The language of that expression.
    query_type: &'static str,
}

/// The definitions the round-trip rows write.
///
/// The rows span the class: every value of `QueryType`
/// (`smithy_query(query="shape",shape="QueryType")` — `PROMQL`, `SQL`, `METRIC_MATH`), a path of
/// one segment and paths organised into folders, paths carrying spaces, punctuation, symbols and
/// non-ASCII letters with combining marks (the character classes `DefinitionPath`'s pattern
/// admits), the shortest path and expression the model allows and the longest path this service
/// accepts.
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            case: "one segment, a metric math expression",
            path: "latency".to_string(),
            expression: "m1 + m2".to_string(),
            query_type: "METRIC_MATH",
        },
        Row {
            case: "folders, a Metrics Insights query",
            path: "team/web/latency".to_string(),
            expression: "SELECT AVG(CPUUtilization) FROM SCHEMA(\"AWS/EC2\", InstanceId)"
                .to_string(),
            query_type: "SQL",
        },
        Row {
            case: "spaces, punctuation and symbols in the folder names, a PromQL query",
            path: "team one/web (prod)/latency p99 ~ 50%".to_string(),
            expression: "sum(rate(http_requests_total[5m]))".to_string(),
            query_type: "PROMQL",
        },
        Row {
            case: "non-ASCII letters with a combining mark",
            path: "équipe/wéb\u{0301}/latence".to_string(),
            expression: "m1".to_string(),
            query_type: "METRIC_MATH",
        },
        Row {
            case: "the shortest path and expression the model admits",
            path: "q".to_string(),
            expression: "1".to_string(),
            query_type: "SQL",
        },
        Row {
            case: "the longest path this service accepts",
            path: "a".repeat(1_015),
            expression: "x".repeat(4_096),
            query_type: "PROMQL",
        },
    ]
}

/// The request that CREATES a definition: `Id` left out, which is what asks for one to be created
/// ("To create a definition, omit Id.", `PutMetricDataQueryDefinition`).
fn create_request(path: &str, expression: &str, query_type: &str) -> Value {
    json!({"Path": path, "Expression": expression, "Type": query_type})
}

/// Create the definition `row` names, requiring the success, and answer the body.
async fn created(router: &MultiServiceRouter, row: &Row) -> Value {
    ok(
        router,
        "PutMetricDataQueryDefinition",
        &create_request(&row.path, &row.expression, row.query_type),
    )
    .await
}

/// The member `member` of `body` as text, or the empty string when the body carries none.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The member `member` of `body` as a number, or `None` when the body carries none.
fn number(body: &Value, member: &str) -> Option<f64> {
    body.get(member).and_then(Value::as_f64)
}

/// The `Items` of a listing body.
fn items(body: &Value) -> Vec<Value> {
    body.get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The paths a listing body reports, in the order it reports them.
fn listed_paths(body: &Value) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| text(item, "Path"))
        .collect::<Vec<String>>()
}

#[tokio::test]
async fn every_created_definition_round_trips_through_the_reader_and_the_listing() {
    let router = build_router();
    let mut written: Vec<(String, Row)> = Vec::new();

    for row in round_trip_rows() {
        let put = created(&router, &row).await;
        let id = text(&put, "Id");
        assert!(
            is_definition_id(&id),
            "case {:?}: expected a minted DefinitionId, got {put}",
            row.case
        );
        assert!(
            written.iter().all(|(other, _)| other != &id),
            "case {:?}: every definition carries its own id, got the repeated {id:?} among {:?}",
            row.case,
            written.iter().map(|(id, _)| id).collect::<Vec<&String>>()
        );
        assert_eq!(
            text(&put, "Arn"),
            expected_arn(DEFAULT_REGION, &id),
            "case {:?}: expected the ARN of the definition, got {put}",
            row.case
        );
        assert_eq!(
            text(&put, "Path"),
            row.path,
            "case {:?}: expected the path the write submitted, got {}",
            row.case,
            text(&put, "Path")
        );
        assert_eq!(
            text(&put, "Expression"),
            row.expression,
            "case {:?}: expected the expression the write submitted",
            row.case
        );
        assert_eq!(
            text(&put, "Type"),
            row.query_type,
            "case {:?}: expected the type the write submitted, got {put}",
            row.case
        );
        let created_at = number(&put, "CreatedAt");
        let updated_at = number(&put, "UpdatedAt");
        assert!(
            created_at.is_some_and(|stamp| stamp > 0.0) && created_at == updated_at,
            "case {:?}: expected a create to stamp both instants at the same moment, got \
             CreatedAt {created_at:?} and UpdatedAt {updated_at:?}",
            row.case
        );

        // The reader reports the definition the write persisted, expression included.
        let read = ok(
            &router,
            "GetMetricDataQueryDefinition",
            &json!({"Id": id.clone()}),
        )
        .await;
        assert_eq!(
            read, put,
            "case {:?}: expected the reader to report the definition the write answered with, \
             got {read}",
            row.case
        );
        written.push((id, row));
    }

    // The listing reports every definition of the Region, ordered by path, as the SUMMARY that
    // "doesn't include the expression" (`ListMetricDataQueryDefinitions`).
    let listed = ok(&router, "ListMetricDataQueryDefinitions", &json!({})).await;
    let mut expected: Vec<String> = written.iter().map(|(_, row)| row.path.clone()).collect();
    expected.sort();
    assert_eq!(
        listed_paths(&listed),
        expected,
        "expected every written definition listed once in path order"
    );
    assert!(
        listed.get("NextToken").is_none(),
        "a listing that delivered every definition owes no token, got {listed}"
    );
    for item in items(&listed) {
        let path = text(&item, "Path");
        let (id, row) = written
            .iter()
            .find(|(_, row)| row.path == path)
            .unwrap_or_else(|| panic!("case {path:?}: the listing reports a definition written"));
        assert_eq!(
            text(&item, "Id"),
            *id,
            "case {:?}: expected the summary to carry the definition's id, got {item}",
            row.case
        );
        assert_eq!(
            text(&item, "Type"),
            row.query_type,
            "case {:?}: expected the summary to carry the definition's type, got {item}",
            row.case
        );
        assert!(
            number(&item, "UpdatedAt").is_some_and(|stamp| stamp > 0.0),
            "case {:?}: expected the summary to carry the instant of the last update, got {item}",
            row.case
        );
        assert!(
            item.get("Expression").is_none(),
            "case {:?}: a summary carries no expression, got {item}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_update_replaces_the_path_expression_and_type_and_keeps_the_identity_and_creation_time()
{
    let router = build_router();

    // The rows differ along what the update CHANGES: the path alone, the expression alone, the
    // language of the expression (and with it the whole triple), and nothing at all — a write
    // that restates the definition it already stored.
    let rows: Vec<(&str, Value)> = vec![
        ("the path alone", json!({"Path": "updated/path"})),
        (
            "the expression alone",
            json!({"Expression": "sum(rate(errors_total[1m]))"}),
        ),
        (
            "the path, the expression and the type together",
            json!({
                "Path": "team/web/p99",
                "Expression": "SELECT MAX(Latency) FROM SCHEMA(\"AWS/ApiGateway\", ApiName)",
                "Type": "SQL",
            }),
        ),
        ("nothing: the definition restated", json!({})),
    ];

    for (case, patch) in rows {
        let row = Row {
            case: "the definition an update replaces",
            path: format!("original/{}", case.replace(' ', "-").replace(',', "")),
            expression: "m1".to_string(),
            query_type: "PROMQL",
        };
        let created = created(&router, &row).await;
        let id = text(&created, "Id");

        let mut request = create_request(&row.path, &row.expression, row.query_type);
        request["Id"] = Value::from(id.clone());
        for (member, value) in patch.as_object().expect("the patch is an object") {
            request[member.as_str()] = value.clone();
        }
        let updated = ok(&router, "PutMetricDataQueryDefinition", &request).await;

        assert_eq!(
            text(&updated, "Id"),
            id,
            "case {case:?}: an update keeps the definition's id, got {updated}"
        );
        assert_eq!(
            text(&updated, "Arn"),
            expected_arn(DEFAULT_REGION, &id),
            "case {case:?}: an update keeps the definition's ARN, got {updated}"
        );
        assert_eq!(
            number(&updated, "CreatedAt"),
            number(&created, "CreatedAt"),
            "case {case:?}: an update keeps the creation time, got {updated}"
        );
        assert!(
            number(&updated, "UpdatedAt") >= number(&created, "CreatedAt"),
            "case {case:?}: an update stamps UpdatedAt at or after the creation, got {updated}"
        );
        for member in ["Path", "Expression", "Type"] {
            let submitted = request
                .get(member)
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                text(&updated, member),
                submitted,
                "case {case:?}: an update replaces {member}, got {updated}"
            );
        }

        // What the reader reports afterwards is what the update left, so the replacement is in
        // the store rather than only in the answer.
        let read = ok(
            &router,
            "GetMetricDataQueryDefinition",
            &json!({"Id": id.clone()}),
        )
        .await;
        assert_eq!(
            read, updated,
            "case {case:?}: expected the reader to report what the update left, got {read}"
        );
    }
}

#[tokio::test]
async fn a_path_another_definition_already_uses_is_refused_as_a_conflict() {
    let router = build_router();
    let held = Row {
        case: "the definition holding the path",
        path: "team/web/latency".to_string(),
        expression: "m1".to_string(),
        query_type: "METRIC_MATH",
    };
    let other = Row {
        case: "a second definition, at its own path",
        path: "team/web/errors".to_string(),
        expression: "m2".to_string(),
        query_type: "METRIC_MATH",
    };
    let held_body = created(&router, &held).await;
    let other_body = created(&router, &other).await;
    let other_id = text(&other_body, "Id");

    // The rows differ along WHICH request asks for a path: a create, an update of another
    // definition, an update of the definition that already holds it (no conflict: the path is
    // not another definition's), and a create at a path nobody holds.
    let taking_path = create_request(&held.path, "m3", "SQL");    let mut moving_other = create_request(&held.path, &other.expression, other.query_type);
    moving_other["Id"] = Value::from(other_id.clone());
    let mut keeping_own = create_request(&held.path, &held.expression, held.query_type);
    keeping_own["Id"] = Value::from(text(&held_body, "Id"));
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a create at a path already used",
            taking_path.clone(),
            409,
            "ConflictException",
        ),
        (
            "an update moving another definition onto it",
            moving_other,
            409,
            "ConflictException",
        ),
        (
            "an update keeping the definition's own path",
            keeping_own,
            200,
            "",
        ),
        (
            "a create at a path nobody uses",
            create_request("team/web/saturation", "m4", "SQL"),
            200,
            "",
        ),
    ];

    for (case, request, status, code) in rows {
        let (answered, body) = call(&router, "PutMetricDataQueryDefinition", &request).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with body {body}"
        );
        if status == 409 {
            assert_eq!(
                error_code(&body),
                code,
                "case {case:?}: expected the conflict class, got {body}"
            );
            assert_eq!(
                error_message(&body),
                "The path must be unique in your account and Region.",
                "case {case:?}: expected the requirement the request broke, got {body}"
            );
        }
    }

    // A refused request left the store as it was: the definition holding the path is the one the
    // first write created, and the second definition is still at its own path.
    let read = ok(
        &router,
        "GetMetricDataQueryDefinition",
        &json!({"Id": text(&held_body, "Id")}),
    )
    .await;
    assert_eq!(
        text(&read, "Expression"),
        held.expression,
        "a refused create changes nothing about the definition holding the path, got {read}"
    );
    let moved = ok(
        &router,
        "GetMetricDataQueryDefinition",
        &json!({"Id": other_id}),
    )
    .await;
    assert_eq!(
        text(&moved, "Path"),
        other.path,
        "a refused update leaves the definition at its own path, got {moved}"
    );
}

#[tokio::test]
async fn an_id_no_definition_of_the_region_holds_is_a_not_found() {
    let router = build_router();
    let row = Row {
        case: "a definition of the default Region",
        path: "team/web/latency".to_string(),
        expression: "m1".to_string(),
        query_type: "SQL",
    };
    let id = text(&created(&router, &row).await, "Id");

    // The rows differ along WHICH id is named and which operation names it: an id nothing ever
    // minted, the id of a definition that was deleted, and the id of a definition of ANOTHER
    // Region — each asked for through the reader and through an update.
    ok(
        &router,
        "DeleteMetricDataQueryDefinition",
        &json!({"Id": id.clone()}),
    )
    .await;
    let elsewhere = text(
        &ok(
            &router,
            "PutMetricDataQueryDefinition",
            &create_request("team/web/elsewhere", "m1", "SQL"),
        )
        .await,
        "Id",
    );

    let rows: Vec<(&str, String, &str)> = vec![
        (
            "an id nothing minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
            DEFAULT_REGION,
        ),
        ("the id of a deleted definition", id, DEFAULT_REGION),
        (
            "the id of another Region's definition",
            elsewhere,
            OTHER_REGION,
        ),
    ];

    for (case, missing, region) in rows {
        for operation in [
            "GetMetricDataQueryDefinition",
            "PutMetricDataQueryDefinition",
        ] {
            let mut request = json!({"Id": missing.clone()});
            if operation == "PutMetricDataQueryDefinition" {
                request["Path"] = Value::from("team/web/updated");
                request["Expression"] = Value::from("m1");
                request["Type"] = Value::from("SQL");
            }
            let (status, body) = call_in_region(&router, region, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {case:?} through {operation}: expected HTTP 404, got {status} with body \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} through {operation}: expected the not-found class, got {body}"
            );
            assert_eq!(
                text(&body, "ResourceType"),
                "MetricDataQueryDefinition",
                "case {case:?} through {operation}: expected the kind of resource named, got \
                 {body}"
            );
            assert_eq!(
                text(&body, "ResourceId"),
                missing,
                "case {case:?} through {operation}: expected the id the request named, got {body}"
            );
            assert!(
                body.get("Message").is_none() && body.get("message").is_none(),
                "case {case:?} through {operation}: this class is answered by its code alone, \
                 got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_delete_removes_the_definition_and_succeeds_whether_or_not_one_was_there() {
    let router = build_router();
    let kept = Row {
        case: "the definition the delete leaves alone",
        path: "team/web/kept".to_string(),
        expression: "m1".to_string(),
        query_type: "SQL",
    };
    let removed = Row {
        case: "the definition the delete removes",
        path: "team/web/removed".to_string(),
        expression: "m2".to_string(),
        query_type: "PROMQL",
    };
    created(&router, &kept).await;
    let removed_id = text(&created(&router, &removed).await, "Id");

    // The rows differ along what the id names when the delete runs: a definition that is there,
    // the same id a second time, and an id nothing ever minted. Every row is the same success.
    let rows: Vec<(&str, String)> = vec![
        ("a definition that exists", removed_id.clone()),
        ("the same id again", removed_id.clone()),
        (
            "an id nothing minted",
            "11111111-1111-4111-8111-111111111111".to_string(),
        ),
    ];
    for (case, id) in rows {
        let (status, body) = call(
            &router,
            "DeleteMetricDataQueryDefinition",
            &json!({"Id": id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a delete is idempotent, expected HTTP 200, got {status} with body \
             {body}"
        );
    }

    let (status, body) = call(
        &router,
        "GetMetricDataQueryDefinition",
        &json!({"Id": removed_id}),
    )
    .await;
    assert_eq!(
        status, 404,
        "a deleted definition is gone, expected HTTP 404, got {status} with body {body}"
    );
    let listed = ok(&router, "ListMetricDataQueryDefinitions", &json!({})).await;
    assert_eq!(
        listed_paths(&listed),
        vec![kept.path.clone()],
        "the listing carries only the definition the delete left"
    );
}

#[tokio::test]
async fn every_path_outside_the_requirements_its_member_states_is_refused() {
    let router = build_router();

    // The invalid rows span the three requirements the member states in prose: "It can't start or
    // end with a slash. It can't contain an empty folder name (//). It can be at most 1,015 bytes
    // when encoded as UTF-8." (`PutMetricDataQueryDefinitionInput$Path`) — each tested one byte
    // or one character past the bound where it has one.
    let refused: Vec<(&str, String)> = vec![
        ("a leading slash", "/team/web".to_string()),
        ("a trailing slash", "team/web/".to_string()),
        ("a slash alone", "/".to_string()),
        ("an empty folder name", "team//web".to_string()),
        (
            "an empty folder name at the start of the path",
            "team///web".to_string(),
        ),
        ("one byte past the longest path", "a".repeat(1_016)),
        (
            "a path inside the modelled character length but past the byte bound",
            "é".repeat(600),
        ),
    ];
    for (case, path) in refused {
        let (status, body) = call(
            &router,
            "PutMetricDataQueryDefinition",
            &create_request(&path, "m1", "SQL"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the client-input class, got {body}"
        );
        assert!(
            error_message(&body).starts_with("Path "),
            "case {case:?}: expected a refusal naming the member at fault, got {body}"
        );
    }

    // The accepted rows are the same requirements satisfied, each at the bound: a path exactly as
    // long as the service accepts, in one-byte and in multi-byte characters, and a path whose
    // folder names are single characters.
    let accepted: Vec<(&str, String)> = vec![
        ("the longest path, one byte per character", "a".repeat(1_015)),
        (
            "the longest path, two bytes per character",
            format!("{}a", "é".repeat(507)),
        ),
        ("single-character folder names", "a/b/c".to_string()),
    ];
    for (case, path) in accepted {
        let (status, body) = call(
            &router,
            "PutMetricDataQueryDefinition",
            &create_request(&path, "m1", "SQL"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the path to be accepted, got {status} with body {body}"
        );
        assert_eq!(
            text(&body, "Path"),
            path,
            "case {case:?}: expected the path the write submitted, got {body}"
        );
    }

    // Only the accepted paths are in the store: a refused write stored nothing.
    let listed = ok(&router, "ListMetricDataQueryDefinitions", &json!({})).await;
    assert_eq!(
        listed_paths(&listed).len(),
        3,
        "expected only the accepted paths in the store, got {listed}"
    );
}

#[tokio::test]
async fn a_definition_belongs_to_the_region_it_was_written_in() {
    let router = build_router();
    let path = "team/web/latency";

    // The same path in two Regions is two definitions, because "The path must be unique in your
    // account and Region" (`PutMetricDataQueryDefinition`) scopes the uniqueness to the Region.
    let mut ids: Vec<(&str, String)> = Vec::new();
    for region in [DEFAULT_REGION, OTHER_REGION] {
        let (status, body) = call_in_region(
            &router,
            region,
            "PutMetricDataQueryDefinition",
            &create_request(path, "m1", "SQL"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected the path to be free in this Region, got {status} with \
             body {body}"
        );
        let id = text(&body, "Id");
        assert_eq!(
            text(&body, "Arn"),
            expected_arn(region, &id),
            "case {region:?}: expected the ARN to carry the Region the definition lives in, got \
             {body}"
        );
        ids.push((region, id));
    }
    assert!(
        ids[0].1 != ids[1].1,
        "two Regions' definitions are two definitions, got the same id {:?}",
        ids[0].1
    );

    // Each Region's listing and reader answer for its own definition only.
    for (region, id) in &ids {
        let listed =
            ok_in_region(&router, region, "ListMetricDataQueryDefinitions", &json!({})).await;
        assert_eq!(
            items(&listed).len(),
            1,
            "case {region:?}: expected this Region's one definition, got {listed}"
        );
        assert_eq!(
            text(&items(&listed)[0], "Id"),
            *id,
            "case {region:?}: expected the definition written in this Region, got {listed}"
        );
    }
}

/// A call in `region` requiring a 200, the Region-scoped half of the harness's `ok`.
async fn ok_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, answered) = call_in_region(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} in {region}: expected HTTP 200, got {status} with body {answered}"
    );
    answered
}

#[tokio::test]
async fn the_listing_pages_through_every_definition_in_path_order() {
    let router = build_router();
    // Written in an order that is NOT the path order, so a listing that reported insertion order
    // would fail the rows below.
    let paths = [
        "team/web/latency",
        "alpha",
        "team/api/errors",
        "zeta/one",
        "team/api/count",
    ];
    for path in paths {
        ok(
            &router,
            "PutMetricDataQueryDefinition",
            &create_request(path, "m1", "SQL"),
        )
        .await;
    }
    let mut expected: Vec<String> = paths.iter().map(|path| (*path).to_string()).collect();
    expected.sort();

    // The rows differ along the page size asked for: one definition at a time, a size that does
    // not divide the selection, exactly the selection, more than the selection, and the member
    // left out (the default).
    let rows: Vec<(&str, Option<i64>)> = vec![
        ("one at a time", Some(1)),
        ("two at a time", Some(2)),
        ("exactly the selection", Some(5)),
        ("more than the selection", Some(100)),
        ("the default page size", None),
    ];
    for (case, max_results) in rows {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages: usize = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = max_results {
                request["MaxResults"] = Value::from(size);
            }
            if let Some(cursor) = &token {
                request["NextToken"] = Value::from(cursor.clone());
            }
            let page = ok(&router, "ListMetricDataQueryDefinitions", &request).await;
            let delivered = listed_paths(&page);
            if let Some(size) = max_results {
                assert!(
                    i64::try_from(delivered.len()).unwrap_or(i64::MAX) <= size,
                    "case {case:?}: a page delivers at most the size asked for, got {page}"
                );
            }
            walked.extend(delivered);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= paths.len() + 1,
                "case {case:?}: the walk terminates, got {pages} pages for {} definitions",
                paths.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every definition once, in path order"
        );
    }

    // A token this listing did not mint names no position, so the page served is the first — the
    // one rule every listing of this service that SERVES such a token follows.
    let served = ok(
        &router,
        "ListMetricDataQueryDefinitions",
        &json!({"NextToken": "not-a-cursor-this-listing-minted"}),
    )
    .await;
    assert_eq!(
        listed_paths(&served),
        expected,
        "a token this listing cannot read a cursor out of is ignored and the first page served"
    );
}

/// One tagging row: the tags a create submits, and the tag set the definition must hold
/// afterwards.
struct TagRow {
    /// What this row is, named in every assertion.
    case: &'static str,
    /// The `Tags` member the create carries, or `Value::Null` for a create that carries none.
    submitted: Value,
    /// The key-value pairs the definition must then hold, in order.
    held: Vec<(String, String)>,
}

/// `pairs` as the owned key-value pairs a row expects to be held.
fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn tags_are_applied_when_the_request_creates_a_definition_and_kept_across_an_update() {
    let (router, engine) = build_router_with_engine();

    // The rows differ along the tags the create submits: none at all, the member present and
    // empty, one tag and several.
    let rows = vec![
        TagRow {
            case: "no Tags member",
            submitted: Value::Null,
            held: pairs(&[]),
        },
        TagRow {
            case: "the member present and empty",
            submitted: json!([]),
            held: pairs(&[]),
        },
        TagRow {
            case: "one tag",
            submitted: json!([{"Key": "team", "Value": "web"}]),
            held: pairs(&[("team", "web")]),
        },
        TagRow {
            case: "several tags",
            submitted: json!([{"Key": "team", "Value": "web"}, {"Key": "env", "Value": "prod"}]),
            held: pairs(&[("team", "web"), ("env", "prod")]),
        },
    ];

    for row in rows {
        let case = row.case;
        let path = format!("tagged/{}", case.replace(' ', "-"));
        let mut request = create_request(&path, "m1", "SQL");
        if !row.submitted.is_null() {
            request["Tags"] = row.submitted.clone();
        }
        let id = text(
            &ok(&router, "PutMetricDataQueryDefinition", &request).await,
            "Id",
        );

        assert_eq!(
            stored_tags(&engine, &id),
            row.held,
            "case {case:?}: expected the tags the create submitted to be held on the definition"
        );

        // "Tags are applied only when the request creates a definition, that is, when you omit
        // Id." (`PutMetricDataQueryDefinitionInput$Tags`) — so an update neither adds the tags it
        // carries nor drops the ones the create applied.
        let mut update = create_request(&path, "m2", "PROMQL");
        update["Id"] = Value::from(id.clone());
        update["Tags"] = json!([{"Key": "ignored", "Value": "by-the-update"}]);
        ok(&router, "PutMetricDataQueryDefinition", &update).await;
        assert_eq!(
            stored_tags(&engine, &id),
            row.held,
            "case {case:?}: expected an update to leave the definition's tags where they were"
        );
    }
}

/// The tags the store holds for the definition `id` of the default Region, as key-value pairs in
/// the order they are held.
fn stored_tags(engine: &engine::InMemoryEngine, id: &str) -> Vec<(String, String)> {
    cloudwatch::service_state(engine)
        .expect("the CloudWatch store exists")
        .lock()
        .metric_data_query_definition(DEFAULT_REGION, id)
        .map(|stored| {
            stored
                .tags
                .iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_region_already_holding_every_definition_it_can_hold_refuses_another() {
    let (router, engine) = build_router_with_engine();

    // The Region is filled to one short of its capacity through the store's own write — the same
    // entry point the handler calls — so the rows below are the two writes that straddle the
    // bound rather than a thousand round trips.
    let capacity = cloudwatch::MAX_METRIC_DATA_QUERY_DEFINITIONS;
    {
        let state = cloudwatch::service_state(&*engine).expect("the CloudWatch store exists");
        let mut store = state.lock();
        for index in 0..capacity - 1 {
            store
                .put_metric_data_query_definition(
                    DEFAULT_REGION,
                    cloudwatch::MetricDataQueryDefinitionWrite {
                        id: None,
                        path: &format!("filled/{index}"),
                        expression: "m1",
                        query_type: "SQL",
                        tags: &[],
                    },
                    1_767_225_600.0,
                )
                .unwrap_or_else(|refusal| {
                    panic!("filling the Region to {capacity} definitions: {refusal:?}")
                });
        }
    }

    // The rows differ along what the write asks for at the bound: the create that fits, the
    // create past it, and an UPDATE past it — which adds no definition and is therefore still
    // served.
    let last = ok(
        &router,
        "PutMetricDataQueryDefinition",
        &create_request("filled/last", "m1", "SQL"),
    )
    .await;
    let (status, body) = call(
        &router,
        "PutMetricDataQueryDefinition",
        &create_request("filled/one-too-many", "m1", "SQL"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a create past the capacity\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "case \"a create past the capacity\": expected the quota class, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!(
            "You can have up to {capacity} metric data query definitions in each Region of your \
             account."
        ),
        "case \"a create past the capacity\": expected the limit the request was measured \
         against, got {body}"
    );

    let mut update = create_request("filled/last-renamed", "m2", "PROMQL");
    update["Id"] = Value::from(text(&last, "Id"));
    let updated = ok(&router, "PutMetricDataQueryDefinition", &update).await;
    assert_eq!(
        text(&updated, "Path"),
        "filled/last-renamed",
        "case \"an update at the capacity\": an update adds no definition and is served, got \
         {updated}"
    );
}
