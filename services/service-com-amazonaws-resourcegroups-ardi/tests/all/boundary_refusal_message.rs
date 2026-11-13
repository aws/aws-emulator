// SPDX-License-Identifier: CC0-1.0

//! How this service answers a request it refuses AT THE BOUNDARY — before any handler
//! runs — because a value broke one of the constraints the model declares.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served emulator
//! uses (the harness is the sibling suite's `Emulator`), so these exercise the whole
//! restJson1 path: routing, boundary validation, and serialization.
//!
//! The one rule under test, asserted over a TABLE of operations, members and constraint
//! classes rather than one row: such a request is answered
//!
//! * `400` with the error code `BadRequestException` — the one modelled error documented
//!   as "The request includes one or more parameters that violate validation rules." — in
//!   the body's `__type` and in the `x-amzn-errortype` header restJson1 resolves an error
//!   shape from; and
//! * with its text in the `Message` member, which is the member EVERY error shape of this
//!   model declares and none of them renames
//!   (`smithy_query(query="shape",shape="BadRequestException")`), carrying the AWS
//!   constraint envelope: `N validation error(s) detected:` over one clause per violation,
//!   each naming the member path as the API spells it and the requirement the value failed
//!   — where a `@pattern` requirement quotes the expression the SERVICE validates against,
//!   without the whole-value anchors the model writes it with, and states no submitted
//!   value, while every other class quotes the value the request carried.
//!
//! A boundary refusal is the one error class whose body the framework renders from a
//! synthetic shape of its own, so this is where that body is judged to be this service's.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator};

/// The modelled `TagSyncTaskArn` expression, as a clause about it quotes it: the model's
/// own pattern with the whole-value anchors dropped and the two ARN segments this service
/// validates WIDER than its published model spells them — its service segment admits the
/// non-production stages, and its region segment a prefix of up to four letters.
///
/// Source: `smithy_query(query="shape",shape="TagSyncTaskArn")`, whose pattern is this
/// expression wrapped in `^`…`$` with `resource-groups` and `[a-z]{2}` in those two
/// places.
const TAG_SYNC_TASK_ARN: &str = "arn:aws(-[a-z]+)*:resource-groups(-(test|beta|gamma))?:[a-z]{2,4}(-[a-z]+)+-\\d{1}:[0-9]{12}:group/[a-zA-Z0-9_\\.-]{1,150}/[a-z0-9]{26}/tag-sync-task/[a-z0-9]{26}";

/// The modelled `GroupStringV2` expression, quoted the same way — the OTHER ARN pattern of
/// this model a clause can be about, so the segment spelling above is a property of the
/// expression rather than of one shape.
///
/// Source: `smithy_query(query="shape",shape="GroupStringV2")`.
const GROUP_STRING_V2: &str = "[a-zA-Z0-9_\\.-]{1,300}|[a-zA-Z0-9_\\.-]{1,150}/[a-z0-9]{26}|arn:aws(-[a-z]+)*:resource-groups(-(test|beta|gamma))?:[a-z]{2,4}(-[a-z]+)+-\\d{1}:[0-9]{12}:group/([a-zA-Z0-9_\\.-]{1,300}|[a-zA-Z0-9_\\.-]{1,150}/[a-z0-9]{26})";

/// The modelled `NextToken` expression, quoted the same way — an expression carrying
/// neither ARN segment, so it is quoted as the model declares it, minus its anchors.
///
/// Source: `smithy_query(query="shape",shape="NextToken")` (`^[a-zA-Z0-9+/]*={0,2}$`).
const NEXT_TOKEN: &str = "[a-zA-Z0-9+/]*={0,2}";

/// The modelled `CreateGroupName` expression, quoted the same way.
///
/// Source: `smithy_query(query="shape",shape="CreateGroupName")` (`^[a-zA-Z0-9_\.-]+$`).
const CREATE_GROUP_NAME: &str = "[a-zA-Z0-9_\\.-]+";

/// The modelled `RoleArn` expression, quoted the same way.
///
/// Source: `smithy_query(query="shape",shape="RoleArn")`.
const ROLE_ARN: &str = "arn:(aws[a-zA-Z-]*)?:iam::\\d{12}:role/?[a-zA-Z_0-9+=,.@\\-_/]+";

/// Values that are not a `TagSyncTaskArn`, spanning how an ARN can miss that expression:
/// not an ARN at all, an ARN of this service naming no task, and a task ARN whose
/// qualifying ids are outside the character class the expression admits.
fn malformed_task_arns() -> Vec<(&'static str, &'static str)> {
    vec![
        ("not an ARN at all", "not-an-arn-at-all"),
        (
            "an ARN of this service that names no task",
            "arn:aws:resource-groups:us-west-2:000000000000:not-a-valid-task-arn",
        ),
        (
            "a task ARN whose ids are uppercase",
            "arn:aws:resource-groups:us-west-2:000000000000:group/app/01JABCDEFGHIJKLMNOPQRSTUV/tag-sync-task/01JABCDEFGHIJKLMNOPQRSTUV",
        ),
        (
            "a task ARN missing its task segment",
            "arn:aws:resource-groups:us-west-2:000000000000:group/app/01jabcdefghijklmnopqrstuv",
        ),
    ]
}

/// The envelope one violation is reported in.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a `@pattern` requirement is stated in: the member, the expression, and no
/// submitted value.
fn pattern_clause(member: &str, expression: &str) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {expression}"
    )
}

/// The clause a `@range` minimum is stated in: the member, the bound, and the value the
/// request carried.
fn minimum_clause(value: &str, member: &str, bound: &str) -> String {
    format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must have \
         value greater than or equal to {bound}"
    )
}

/// The clause a `@range` maximum is stated in.
fn maximum_clause(value: &str, member: &str, bound: &str) -> String {
    format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must have \
         value less than or equal to {bound}"
    )
}

/// The clause an absent required member is stated in.
fn absent_clause(member: &str) -> String {
    format!("Value null at '{member}' failed to satisfy constraint: Member must not be null")
}

/// One row of the table: a request that breaks one modelled constraint, and the whole
/// `Message` this service answers it with.
struct Row {
    case: String,
    path: &'static str,
    body: Value,
    message: String,
}

/// The rows: the `@pattern` class over four members of four operations, the `@range` class
/// at both of its bounds, and an absent required member — so the rule is judged over the
/// constraint CLASSES and not over one member.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // The two operations addressed by a tag-sync task ARN, over every malformed form.
    for (operation, path) in [
        ("GetTagSyncTask", "/get-tag-sync-task"),
        ("CancelTagSyncTask", "/cancel-tag-sync-task"),
    ] {
        for (label, value) in malformed_task_arns() {
            rows.push(Row {
                case: format!("{operation} with {label}"),
                path,
                body: json!({ "TaskArn": value }),
                message: one_error(&pattern_clause("taskArn", TAG_SYNC_TASK_ARN)),
            });
        }
        rows.push(Row {
            case: format!("{operation} with no TaskArn at all"),
            path,
            body: json!({}),
            message: one_error(&absent_clause("taskArn")),
        });
    }
    rows.push(Row {
        case: "ListGroupingStatuses with a Group outside its pattern".to_string(),
        path: "/list-grouping-statuses",
        body: json!({ "Group": " " }),
        message: one_error(&pattern_clause("group", GROUP_STRING_V2)),
    });
    rows.push(Row {
        case: "ListGroupingStatuses with a NextToken outside its pattern".to_string(),
        path: "/list-grouping-statuses",
        body: json!({ "Group": "some-group", "NextToken": "not a token!" }),
        message: one_error(&pattern_clause("nextToken", NEXT_TOKEN)),
    });
    rows.push(Row {
        case: "CreateGroup with a Name outside its pattern".to_string(),
        path: "/groups",
        body: json!({ "Name": "has a space" }),
        message: one_error(&pattern_clause("name", CREATE_GROUP_NAME)),
    });
    rows.push(Row {
        case: "StartTagSyncTask with a RoleArn outside its pattern".to_string(),
        path: "/start-tag-sync-task",
        body: json!({
            "Group": "some-group",
            "RoleArn": "not-a-role-arn-but-long-enough",
            "TagKey": "Stage",
            "TagValue": "test",
        }),
        message: one_error(&pattern_clause("roleArn", ROLE_ARN)),
    });
    // The other classes, whose clauses DO quote the value the request carried: the
    // modelled `MaxResults` range at both of its published bounds.
    rows.push(Row {
        case: "ListGroupingStatuses below the MaxResults range".to_string(),
        path: "/list-grouping-statuses",
        body: json!({ "Group": "some-group", "MaxResults": 0 }),
        message: one_error(&minimum_clause("0", "maxResults", "1")),
    });
    rows.push(Row {
        case: "ListGroupingStatuses past the MaxResults range".to_string(),
        path: "/list-grouping-statuses",
        body: json!({ "Group": "some-group", "MaxResults": 51 }),
        message: one_error(&maximum_clause("51", "maxResults", "50")),
    });
    rows
}

/// Every boundary refusal is `400 BadRequestException`, names that shape in the
/// `x-amzn-errortype` header, and reports its whole text in the `Message` member — with no
/// second member carrying it.
#[tokio::test]
async fn every_boundary_refusal_states_its_text_in_the_message_member() {
    let emulator = Emulator::new();
    for Row {
        case,
        path,
        body,
        message,
    } in rows()
    {
        let (status, answered, error_type) = emulator.call_reporting_error_type(path, &body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {status} {answered}",
        );
        assert_eq!(
            error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the error shape in x-amzn-errortype, got {error_type:?}",
        );
        assert_eq!(
            answered.get("Message").and_then(Value::as_str),
            Some(message.as_str()),
            "case {case:?}: expected the Message {message:?}, got {answered}",
        );
        assert!(
            answered.get("message").is_none(),
            "case {case:?}: the text must be carried under ONE member, got {answered}",
        );
    }
}

/// A request breaking SEVERAL constraints is one answer counting them: the envelope opens
/// with the count and joins the clauses, each of them the clause that violation is stated
/// with on its own.
#[tokio::test]
async fn a_refusal_over_several_violations_counts_and_joins_its_clauses() {
    let emulator = Emulator::new();
    let (status, answered, _) = emulator
        .call_reporting_error_type(
            "/list-grouping-statuses",
            &json!({ "Group": "some-group", "MaxResults": 0, "NextToken": "not a token!" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered)),
        (400, "BadRequestException"),
        "expected 400 BadRequestException, got {status} {answered}",
    );
    let stated = answered
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a Message, got {answered}"));
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "expected the envelope to count both violations, got {stated:?}",
    );
    for clause in [
        minimum_clause("0", "maxResults", "1"),
        pattern_clause("nextToken", NEXT_TOKEN),
    ] {
        assert!(
            stated.contains(&clause),
            "expected the clause {clause:?} among the stated ones, got {stated:?}",
        );
    }
}
