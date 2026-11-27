// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service words a request the framework refused for
//! failing a constraint the model declares.
//!
//! The rule: a CloudWatch Logs constraint refusal names the INPUT POSITION that failed
//! and the modeled constraint it failed — `N validation error(s) detected: Value at
//! '<member path>' failed to satisfy constraint: <the model's constraint>` — and never
//! interpolates the value submitted there, nor a `null` for a member the request did not
//! carry at all (the recorded `PutStorageTierPolicy` refusal for an absent `storageTier`
//! states `Value at 'storageTier' … Member must not be null`). An enum's value set is
//! listed in ascending order of its values, and a `@pattern` is stated as AWS publishes the
//! expression for that shape — the model's expression, except for a shape whose published
//! form drops its `^`…`$` anchors ([`DELIVERY_SOURCE_NAME_PATTERN`],
//! [`LOG_GROUP_NAME_PATTERN`]). The CODE such a
//! refusal carries is the one the
//! served operation declares, which `tests/all/refusal_error_codes.rs` is about: the
//! delivery and S3-table-source operations below declare `ValidationException`, while
//! `CreateLogGroup` and `DescribeLogGroups` declare `InvalidParameterException`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so the
//! message asserted here is the one a client reads off the wire.
//!
//! The table below spans the class the rule generalises over rather than one recorded
//! request: every constraint kind the model puts on this service's inputs (`@required`,
//! `@length` min and max, `@range` min and max, `@pattern`, an enum value set), the forms
//! a rejected value takes (empty, over-long, punctuated, out of range), and operations
//! beyond the ones any single report cites. Each constraint is cited from the model shape
//! it is declared on.

use serde_json::{json, Value};

use crate::harness::{error_code, Logs};

/// An ARN-typed member this service declares no constraint on (`Arn` in the model has
/// neither `@length` nor `@pattern`), so it is valid input and the refusal under test is
/// the only violation in the response.
const SOME_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:probe";

/// The ARN of an S3 Table Integration, which is the ONE form
/// `ListSourcesForS3TableIntegration$integrationArn` resolves an integration from — the ARN
/// the service that owns one mints, in this account and the Region the calls are signed for
/// — so a row about another member of that operation is refused for THAT member and not for
/// its ARN.
const S3_TABLE_INTEGRATION_ARN: &str =
    "arn:aws:observabilityadmin:eu-west-2:000000000000:s3tableintegration/probe";

/// One refused request and the whole message this service must answer it with.
struct Case {
    /// What this row is about, named in every assertion.
    name: &'static str,
    operation: &'static str,
    body: Value,
    /// The error code the SERVED OPERATION declares for a refusal.
    code: &'static str,
    /// The complete `message` member of the 400 body.
    message: String,
}

/// The code the delivery and S3-table-source operations below declare — the only refusal
/// shape in their modelled error set.
const VALIDATION: &str = "ValidationException";

/// The code `CreateLogGroup` and `DescribeLogGroups` declare: they list
/// `InvalidParameterException` and no `ValidationException` at all.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The clause `DeliveryId`'s `@pattern` states. Source: the model's `DeliveryId` shape
/// (`@length` 1..64, `@pattern ^[0-9A-Za-z]+$`), reached by `GetDelivery.id`,
/// `DeleteDelivery.id` and `UpdateDeliveryConfiguration.id`.
const DELIVERY_ID_PATTERN: &str =
    "Member must satisfy regular expression pattern: ^[0-9A-Za-z]+$";

/// The clause `DeliverySourceName`'s `@pattern` states. The model's `DeliverySourceName`
/// shape carries `@length` 1..60 and `@pattern ^[\w-]*$`, and the expression AWS publishes
/// for that shape is the same one UNANCHORED, which is the form its recorded refusal states.
/// Reached by `name` of the four delivery-source operations and by
/// `CreateDelivery.deliverySourceName`.
const DELIVERY_SOURCE_NAME_PATTERN: &str =
    "Member must satisfy regular expression pattern: [\\w-]*";

/// The clause `LogGroupName`'s `@pattern` states. The model's `LogGroupName` shape carries
/// `@length` 1..512 and `@pattern ^[\.\-_/#A-Za-z0-9]+$`, and the expression AWS publishes
/// for that shape is the same one UNANCHORED — the form the model's own documentation of a
/// log group name states ("Pattern: [\.\-_/#A-Za-z0-9]+", in the `logGroupIdentifier` docs
/// of `PutLogGroupDeletionProtection`). Reached by the `logGroupName` member of every
/// operation that names a log group by name.
const LOG_GROUP_NAME_PATTERN: &str =
    "Member must satisfy regular expression pattern: [\\.\\-_/#A-Za-z0-9]+";

/// The envelope one refused clause is wrapped in.
fn one(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a present-but-invalid value at `path` is refused with: the position and the
/// requirement, and nothing of the value.
fn at(path: &str, requirement: &str) -> String {
    one(&format!(
        "Value at '{path}' failed to satisfy constraint: {requirement}"
    ))
}

/// Every row of the table: one request the model refuses, paired with the message the
/// rule requires.
fn cases() -> Vec<Case> {
    vec![
        // `@pattern`, three operations reaching the same `DeliveryId` shape, three forms
        // of a value outside it: a dash, a slash and a space.
        Case {
            name: "GetDelivery id with a dash",
            operation: "GetDelivery",
            body: json!({"id": "delivery-1"}),
            code: VALIDATION,
            message: at("id", DELIVERY_ID_PATTERN),
        },
        Case {
            name: "DeleteDelivery id with a slash",
            operation: "DeleteDelivery",
            body: json!({"id": "abc/def"}),
            code: VALIDATION,
            message: at("id", DELIVERY_ID_PATTERN),
        },
        Case {
            name: "UpdateDeliveryConfiguration id with a space",
            operation: "UpdateDeliveryConfiguration",
            body: json!({"id": "abc def"}),
            code: VALIDATION,
            message: at("id", DELIVERY_ID_PATTERN),
        },
        // `@required`: the subject is the same `Value at '<path>'`, because the refusal
        // reports the POSITION rather than anything about what reached it.
        Case {
            name: "GetDelivery without the required id",
            operation: "GetDelivery",
            body: json!({}),
            code: VALIDATION,
            message: at("id", "Member must not be null"),
        },
        Case {
            name: "DisassociateSourceFromS3TableIntegration without the required identifier",
            operation: "DisassociateSourceFromS3TableIntegration",
            body: json!({}),
            code: VALIDATION,
            message: at("identifier", "Member must not be null"),
        },
        // `@length` max. Source: `DeliveryId` 1..64.
        Case {
            name: "DeleteDelivery id one character past the maximum length",
            operation: "DeleteDelivery",
            body: json!({"id": "a".repeat(65)}),
            code: VALIDATION,
            message: at("id", "Member must have length less than or equal to 64"),
        },
        // `@length` min, at the empty string. Source: `S3TableIntegrationSourceIdentifier`
        // 1..2048 (no `@pattern`), so the length is the only constraint that can fire.
        Case {
            name: "DisassociateSourceFromS3TableIntegration empty identifier",
            operation: "DisassociateSourceFromS3TableIntegration",
            body: json!({"identifier": ""}),
            code: VALIDATION,
            message: at(
                "identifier",
                "Member must have length greater than or equal to 1",
            ),
        },
        Case {
            name: "DisassociateSourceFromS3TableIntegration identifier past the maximum length",
            operation: "DisassociateSourceFromS3TableIntegration",
            body: json!({"identifier": "a".repeat(2049)}),
            code: VALIDATION,
            message: at(
                "identifier",
                "Member must have length less than or equal to 2048",
            ),
        },
        // `@pattern` and `@length` on a second shape. Source: `DeliverySourceName`
        // (1..60, `^[\w-]*$`) and `LogType` (1..255, `^[\w]*$`), both on
        // `PutDeliverySource`.
        Case {
            name: "PutDeliverySource name with a space and a slash",
            operation: "PutDeliverySource",
            body: json!({
                "name": "invalid name/with space",
                "resourceArn": SOME_ARN,
                "logType": "WAFLOGS",
            }),
            code: VALIDATION,
            message: at("name", DELIVERY_SOURCE_NAME_PATTERN),
        },
        Case {
            name: "PutDeliverySource name one character past the maximum length",
            operation: "PutDeliverySource",
            body: json!({
                "name": "n".repeat(61),
                "resourceArn": SOME_ARN,
                "logType": "WAFLOGS",
            }),
            code: VALIDATION,
            message: at("name", "Member must have length less than or equal to 60"),
        },
        Case {
            name: "PutDeliverySource logType with a dash",
            operation: "PutDeliverySource",
            body: json!({
                "name": "valid-source-name",
                "resourceArn": SOME_ARN,
                "logType": "WAF-LOGS",
            }),
            code: VALIDATION,
            message: at(
                "logType",
                "Member must satisfy regular expression pattern: ^[\\w]*$",
            ),
        },
        // `@range`, both bounds. Source: `ListSourcesForS3TableIntegrationMaxResults`
        // 1..100.
        Case {
            name: "ListSourcesForS3TableIntegration maxResults one past the maximum",
            operation: "ListSourcesForS3TableIntegration",
            body: json!({"integrationArn": S3_TABLE_INTEGRATION_ARN, "maxResults": 101}),
            code: VALIDATION,
            message: at("maxResults", "Member must have value less than or equal to 100"),
        },
        Case {
            name: "ListSourcesForS3TableIntegration maxResults one below the minimum",
            operation: "ListSourcesForS3TableIntegration",
            body: json!({"integrationArn": S3_TABLE_INTEGRATION_ARN, "maxResults": 0}),
            code: VALIDATION,
            message: at(
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
        },
        // A `@range` on another operation, with a different maximum — and one that
        // declares `InvalidParameterException` rather than `ValidationException`. Source:
        // `DescribeLimit` 1..50, reached by `DescribeLogGroups.limit`: the rule is the
        // service's, not these six operations'.
        Case {
            name: "DescribeLogGroups limit one past the maximum",
            operation: "DescribeLogGroups",
            body: json!({"limit": 51}),
            code: INVALID_PARAMETER,
            message: at("limit", "Member must have value less than or equal to 50"),
        },
        // A `@pattern` whose published form drops the model's anchors, on two operations
        // reaching the same `LogGroupName` shape, with two forms of a value outside it.
        Case {
            name: "CreateLogGroup logGroupName with a space and a bang",
            operation: "CreateLogGroup",
            body: json!({"logGroupName": "bad name!"}),
            code: INVALID_PARAMETER,
            message: at("logGroupName", LOG_GROUP_NAME_PATTERN),
        },
        Case {
            name: "PutLogEvents logGroupName with a space and a star",
            operation: "PutLogEvents",
            body: json!({
                "logGroupName": "bad name!*",
                "logStreamName": "stream",
                "logEvents": [{"timestamp": 1_700_000_000_000_i64, "message": "hello"}],
            }),
            code: INVALID_PARAMETER,
            message: at("logGroupName", LOG_GROUP_NAME_PATTERN),
        },
        // An enum value set. Source: the model's `LogGroupClass` enum — declared STANDARD,
        // INFREQUENT_ACCESS, DELIVERY, ESSENTIALS — listed in ascending order of its values.
        Case {
            name: "CreateLogGroup logGroupClass outside the enum",
            operation: "CreateLogGroup",
            body: json!({"logGroupName": "valid-name", "logGroupClass": "NOT_A_CLASS"}),
            code: INVALID_PARAMETER,
            message: at(
                "logGroupClass",
                "Member must satisfy enum value set: [DELIVERY, ESSENTIALS, INFREQUENT_ACCESS, STANDARD]",
            ),
        },
    ]
}

/// Every refused request is answered 400 with the code the served operation declares and
/// the message the rule requires — the member path and the modeled constraint, with the
/// submitted value withheld.
#[tokio::test]
async fn a_refused_request_states_the_position_and_the_constraint_only() {
    let logs = Logs::new();
    for case in cases() {
        let Case {
            name,
            operation,
            body,
            code,
            message: expected,
        } = case;
        let (status, parsed) = logs.call(operation, &body).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            code,
            "case {name:?}: expected error code {code}, got {parsed}"
        );
        let actual = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: the error body must carry a message, got {parsed}"));
        assert_eq!(
            actual, expected,
            "case {name:?}: expected message {expected:?}, got {actual:?}"
        );
        // The value-echoing clause the renderer must never produce, whatever the
        // constraint and whatever the value: `Value '<submitted>' at '<path>'`.
        assert!(
            !actual.contains("Value '"),
            "case {name:?}: the message must not quote the submitted value, got {actual:?}"
        );
    }
}

/// A value INSIDE the declared bound is not refused: each published bound of
/// `ListSourcesForS3TableIntegrationMaxResults` (1 and 100) and of `DescribeLimit`
/// (1 and 50) passes the constraint walk, so the request reaches its operation — a 200 from
/// both readers — instead of a 400 naming that member.
#[tokio::test]
async fn a_value_on_either_published_bound_is_not_refused() {
    let logs = Logs::new();
    // The integration the S3-table rows read: an `integrationArn` is resolved against the
    // integrations the account holds, so a row about `maxResults` needs one to exist.
    crate::harness::seed_s3_table_integration(&logs, "eu-west-2", "probe");
    // (case, operation, member, request, the status the request must reach)
    let rows: [(&str, &str, &str, Value, u16); 4] = [
        (
            "ListSourcesForS3TableIntegration maxResults at the minimum",
            "ListSourcesForS3TableIntegration",
            "maxResults",
            json!({"integrationArn": S3_TABLE_INTEGRATION_ARN, "maxResults": 1}),
            200,
        ),
        (
            "ListSourcesForS3TableIntegration maxResults at the maximum",
            "ListSourcesForS3TableIntegration",
            "maxResults",
            json!({"integrationArn": S3_TABLE_INTEGRATION_ARN, "maxResults": 100}),
            200,
        ),
        (
            "DescribeLogGroups limit at the minimum",
            "DescribeLogGroups",
            "limit",
            json!({"limit": 1}),
            200,
        ),
        (
            "DescribeLogGroups limit at the maximum",
            "DescribeLogGroups",
            "limit",
            json!({"limit": 50}),
            200,
        ),
    ];
    for (name, operation, member, body, expected) in rows {
        let (status, parsed) = logs.call(operation, &body).await;
        assert_eq!(
            status, expected,
            "case {name:?}: a value on the published bound reaches the operation, \
             expected HTTP {expected}, got {status} {parsed}"
        );
        let message = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !message.contains(&format!("at '{member}'")),
            "case {name:?}: no clause may name that member, got {message:?}"
        );
    }
}

/// Two violations in one request are counted and joined into the plural envelope, each
/// clause stating its own position and constraint and neither quoting a submitted value.
#[tokio::test]
async fn two_violations_render_as_two_clauses_without_values() {
    let logs = Logs::new();
    // `maxResults` is outside `@range` 1..100 and `nextToken` is shorter than
    // `NextToken`'s `@length` minimum of 1.
    let (status, parsed) = logs
        .call(
            "ListSourcesForS3TableIntegration",
            &json!({
                "integrationArn": S3_TABLE_INTEGRATION_ARN,
                "maxResults": 101,
                "nextToken": "",
            }),
        )
        .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} {parsed}");
    let actual = parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the error body must carry a message, got {parsed}"));
    assert!(
        actual.starts_with("2 validation errors detected: "),
        "case \"two violations\": expected the plural envelope, got {actual:?}"
    );
    for clause in [
        "Value at 'maxResults' failed to satisfy constraint: \
         Member must have value less than or equal to 100",
        "Value at 'nextToken' failed to satisfy constraint: \
         Member must have length greater than or equal to 1",
    ] {
        assert!(
            actual.contains(clause),
            "case \"two violations\": expected a clause {clause:?}, got {actual:?}"
        );
    }
    assert!(
        !actual.contains("Value '"),
        "case \"two violations\": no clause may quote the submitted value, got {actual:?}"
    );
}
