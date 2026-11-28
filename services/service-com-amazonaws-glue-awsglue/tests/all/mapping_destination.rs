// SPDX-License-Identifier: CC0-1.0

//! The destination a mapping request has to state, over the two operations that take one:
//! `GetMapping` (the field-by-field proposal) and `GetPlan` (the code that performs it).
//!
//! RULE: both operations take the two members that say where the mapped data goes — `Sinks`,
//! "the target tables", and `Location`, "the parameters for the mapping" — and the model
//! makes neither `@required` (`smithy_query operation=GetPlan,GetMapping`), so the service
//! enforces the either/or itself: a request that states NEITHER is refused with HTTP 400 and
//! `InvalidInputException`, while one that states either of them is served. A `Location` is a
//! structure of three lists of arguments, one per kind of location (`smithy_query
//! shape=Location`), so it states a destination only when one of those lists carries an
//! argument — a structure whose members are all absent or empty addresses nothing, exactly as
//! an absent `Location` does.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: which of the two members is stated,
//! whether the other is absent or empty, how many target tables are named (one, two), which
//! member of `Location` carries the arguments (S3, JDBC, DynamoDB), whether an empty member
//! list stands before the one that carries them, and whether both members are stated at once.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::wire::{emulator, ok};

/// The database both operations read their tables out of.
const DATABASE: &str = "destination_db";

/// The source table every request maps from.
const SOURCE: &str = "orders";

/// The table a row names as a target.
const TARGET: &str = "orders_copy";

/// The `Message` the refusal carries: AWS's own sentence for a mapping request that states no
/// destination, which states the rule and so names nothing of the request.
const MISSING_DESTINATION: &str = "Either Sinks or Location must be non-empty.";

/// One row: the case, the `Sinks` and `Location` members the request carries (`Value::Null`
/// for a member it does not carry at all), and whether the request states a destination and so
/// has to be served.
struct Row {
    case: &'static str,
    sinks: Value,
    location: Value,
    served: bool,
}

/// One target table, as a `CatalogEntry` naming the stored table.
fn target() -> Value {
    json!({ "DatabaseName": DATABASE, "TableName": TARGET })
}

/// The rows: every shape of the two members the rule turns on.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "neither member is stated",
            sinks: Value::Null,
            location: Value::Null,
            served: false,
        },
        Row {
            case: "an empty Sinks list and no Location",
            sinks: json!([]),
            location: Value::Null,
            served: false,
        },
        Row {
            case: "no Sinks and a Location stating nothing",
            sinks: Value::Null,
            location: json!({}),
            served: false,
        },
        Row {
            case: "a Location whose every member list is empty",
            sinks: Value::Null,
            location: json!({ "S3": [], "Jdbc": [], "DynamoDB": [] }),
            served: false,
        },
        Row {
            case: "an empty Sinks list beside a Location whose only list is empty",
            sinks: json!([]),
            location: json!({ "S3": [] }),
            served: false,
        },
        Row {
            case: "one target table",
            sinks: json!([target()]),
            location: Value::Null,
            served: true,
        },
        Row {
            case: "two target tables",
            sinks: json!([target(), target()]),
            location: Value::Null,
            served: true,
        },
        Row {
            case: "an S3 location alone",
            sinks: Value::Null,
            location: json!({ "S3": [{ "Name": "path", "Value": "s3://bucket/out/" }] }),
            served: true,
        },
        Row {
            case: "a JDBC location alone",
            sinks: json!([]),
            location: json!({ "Jdbc": [{ "Name": "connectionName", "Value": "warehouse" }] }),
            served: true,
        },
        Row {
            case: "a DynamoDB location alone",
            sinks: Value::Null,
            location: json!({
                "DynamoDB": [{ "Name": "dynamodb.output.tableName", "Value": "orders_out" }],
            }),
            served: true,
        },
        Row {
            case: "a location whose first list is empty and a later one carries an argument",
            sinks: Value::Null,
            location: json!({
                "S3": [],
                "DynamoDB": [{ "Name": "dynamodb.output.tableName", "Value": "orders_out" }],
            }),
            served: true,
        },
        Row {
            case: "both a target table and a location",
            sinks: json!([target()]),
            location: json!({ "S3": [{ "Name": "path", "Value": "s3://bucket/out/" }] }),
            served: true,
        },
    ]
}

/// The request body a row sends, naming `source` as the source table: the members the row
/// states, plus the `Mapping` list `GetPlan` requires (`GetMapping` carries no such member and
/// ignores it, so one builder serves both).
fn request(row: &Row, source: &str) -> Value {
    let mut body = json!({
        "Source": { "DatabaseName": DATABASE, "TableName": source },
        "Mapping": [{ "SourcePath": "id" }],
    });
    if !row.sinks.is_null() {
        body["Sinks"] = row.sinks.clone();
    }
    if !row.location.is_null() {
        body["Location"] = row.location.clone();
    }
    body
}

/// An emulator holding the source and target tables every row names.
async fn emulator_with_tables() -> MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    for table in [SOURCE, TARGET] {
        ok(
            &router,
            "CreateTable",
            &json!({
                "DatabaseName": DATABASE,
                "TableInput": {
                    "Name": table,
                    "StorageDescriptor": { "Columns": [{ "Name": "id", "Type": "int" }] },
                },
            }),
        )
        .await;
    }
    router
}

/// `GetMapping` serves a request that states a destination and refuses one that states none.
#[tokio::test]
async fn get_mapping_needs_a_destination() {
    let router = emulator_with_tables().await;

    for row in &rows() {
        let body = request(row, SOURCE);
        if row.served {
            let answered = ok(&router, "GetMapping", &body).await;
            assert!(
                answered["Mapping"].is_array(),
                "case {case:?}: a request stating a destination must be proposed a mapping, \
                 got {answered}",
                case = row.case,
            );
        } else {
            assert_refusal(
                &router,
                row.case,
                "GetMapping",
                &body,
                "InvalidInputException",
                MISSING_DESTINATION,
            )
            .await;
        }
    }
}

/// `GetPlan` follows the same rule: no destination, no plan.
#[tokio::test]
async fn get_plan_needs_a_destination() {
    let router = emulator_with_tables().await;

    for row in &rows() {
        let body = request(row, SOURCE);
        if row.served {
            let answered = ok(&router, "GetPlan", &body).await;
            assert!(
                answered["PythonScript"].is_string(),
                "case {case:?}: a request stating a destination must be planned, got {answered}",
                case = row.case,
            );
        } else {
            assert_refusal(
                &router,
                row.case,
                "GetPlan",
                &body,
                "InvalidInputException",
                MISSING_DESTINATION,
            )
            .await;
        }
    }
}

/// Whether the request states a destination is a property of the REQUEST, so it is answered
/// before any table it names is looked up: a request stating none is refused for that, and not
/// for the source table it also names and the catalog does not hold.
#[tokio::test]
async fn the_destination_is_judged_before_the_tables_are_looked_up() {
    let router = emulator_with_tables().await;
    let absent = Row {
        case: "no destination and a source the catalog does not hold",
        sinks: Value::Null,
        location: Value::Null,
        served: false,
    };

    for operation in ["GetMapping", "GetPlan"] {
        assert_refusal(
            &router,
            &format!("{operation}: {case}", case = absent.case),
            operation,
            &request(&absent, "no_such_tbl"),
            "InvalidInputException",
            MISSING_DESTINATION,
        )
        .await;
    }
}

/// The refusal is of a request, never of the store: after one, both operations still serve a
/// request that does state a destination, over the tables the catalog held all along.
#[tokio::test]
async fn a_refused_request_leaves_both_operations_serving() {
    let router = emulator_with_tables().await;
    let refused = Row {
        case: "neither member is stated",
        sinks: Value::Null,
        location: Value::Null,
        served: false,
    };
    let stated = Row {
        case: "one target table",
        sinks: json!([target()]),
        location: Value::Null,
        served: true,
    };

    for operation in ["GetMapping", "GetPlan"] {
        assert_refusal(
            &router,
            &format!("{operation}: {case}", case = refused.case),
            operation,
            &request(&refused, SOURCE),
            "InvalidInputException",
            MISSING_DESTINATION,
        )
        .await;
        let answered = ok(&router, operation, &request(&stated, SOURCE)).await;
        let member = if operation == "GetMapping" {
            "Mapping"
        } else {
            "PythonScript"
        };
        assert!(
            !answered[member].is_null(),
            "case {case:?}: {operation} must still answer a {member} after a refusal, got \
             {answered}",
            case = stated.case,
        );
    }
}
