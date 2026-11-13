// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a `ScheduledQueryIdentifier` is RESOLVED by the operations that
//! address a scheduled query by one — `GetScheduledQuery`, `UpdateScheduledQuery` and
//! `DeleteScheduledQuery`.
//!
//! The rule under test: an operation addressed by a scheduled-query identifier resolves it
//! against the scheduled queries the ACCOUNT HOLDS — "The ARN or name of the scheduled query",
//! so either form reaches the same stored record — and refuses an identifier holding none with
//! HTTP 400 `ResourceNotFoundException` whose sentence names the identifier exactly as the
//! request supplied it. A delete therefore answers its empty response only for a record it
//! actually removed: a second delete of the same identifier is refused like any other
//! identifier naming nothing, so a client can tell "this call removed the query" from "nothing
//! was there". An identifier outside the length the model states for the shape never reaches
//! that resolution at all: it is refused as a constraint failure on the member.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — and every
//! refusal is paired with a read of what the account still holds through
//! `ListScheduledQueries`, so a response that is not computed from the shared store cannot
//! pass.
//!
//! The table spans what the rule generalises over rather than one request: the operation the
//! identifier addresses (all three), the form it takes (a name, the ARN built from one, the ARN
//! of another resource kind, an ARN naming another Region and account, a value that is neither),
//! its relation to what the account holds (never created, differing only in case from a held
//! query, already removed), and its length at each bound the model publishes (1 and 300) and one
//! character past the maximum.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, scheduled_query_arn,
};

use crate::harness::{Logs, REGION};

/// The operations that address a scheduled query by `ScheduledQueryIdentifier`, the create that
/// puts one in the store, and the reader every case proves the store through.
const GET: &str = "GetScheduledQuery";
const UPDATE: &str = "UpdateScheduledQuery";
const DELETE: &str = "DeleteScheduledQuery";
const CREATE: &str = "CreateScheduledQuery";
const LIST: &str = "ListScheduledQueries";

/// Every operation the identifier rule covers, so each row is asserted on all of them.
const ADDRESSED_BY_IDENTIFIER: [&str; 3] = [GET, UPDATE, DELETE];

/// The codes the two refusals travel under, and the status both carry.
const NOT_FOUND: &str = "ResourceNotFoundException";
const VALIDATION: &str = "ValidationException";
const REFUSAL_STATUS: u16 = 400;

/// The IAM role a scheduled query executes under: "The ARN of the IAM role used to execute the
/// query and deliver results."
const ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// The bounds the model publishes for `ScheduledQueryIdentifier`. Source: the `identifier`
/// member of `DeleteScheduledQuery` / `GetScheduledQuery` / `UpdateScheduledQuery`, whose target
/// `ScheduledQueryIdentifier` carries `@length(min: 1, max: 300)`.
const IDENTIFIER_MIN_LENGTH: usize = 1;
const IDENTIFIER_MAX_LENGTH: usize = 300;

/// The sentence a scheduled query that is not there is refused with, pinned LITERALLY here
/// rather than built with the helper the handlers call, so a wording that is wrong but
/// consistent with the implementation cannot pass. The single variable part is the identifier
/// the request supplied, whichever form it named the query by.
fn not_found_message(identifier: &str) -> String {
    format!("Scheduled query with identifier {identifier} not found")
}

/// The sentence a value outside the member's own constraint is refused with — the position and
/// the requirement, and nothing of the value.
fn constraint_refusal(requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at 'identifier' failed to satisfy constraint: \
         {requirement}"
    )
}

/// A request addressing `identifier` on `operation`, carrying whatever else that operation
/// requires. `None` omits the member entirely, which is how the `@required` case is stated.
fn request(operation: &str, identifier: Option<&str>) -> Value {
    let mut body = Map::new();
    if let Some(identifier) = identifier {
        body.insert("identifier".to_string(), json!(identifier));
    }
    if operation == UPDATE {
        // "This operation uses PUT semantics": the members an update must state, all valid, so
        // the only thing a row varies is the identifier.
        body.insert("queryLanguage".to_string(), json!("CWLI"));
        body.insert("queryString".to_string(), json!("fields @message"));
        body.insert("scheduleExpression".to_string(), json!("rate(5 minutes)"));
        body.insert("executionRoleArn".to_string(), json!(ROLE));
    }
    Value::Object(body)
}

/// Put one scheduled query in the store over the wire, like any client's call.
async fn create(logs: &Logs, name: &str) {
    logs.ok(
        CREATE,
        &json!({
            "name": name,
            "queryLanguage": "CWLI",
            "queryString": "fields @timestamp, @message",
            "scheduleExpression": "cron(0 12 * * ? *)",
            "executionRoleArn": ROLE,
        }),
    )
    .await;
}

/// The names `ListScheduledQueries` reports, in the order it reports them — what a client can
/// observe of the queries the account holds.
async fn listed(logs: &Logs) -> Vec<String> {
    let page = logs.ok(LIST, &json!({})).await;
    page.get("scheduledQueries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{LIST} must return scheduledQueries, got {page}"))
        .iter()
        .map(|summary| {
            summary
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every summary carries a name, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Assert `operation` refused `identifier` with `code` and `message`, returning nothing but the
/// failure of whichever part diverges.
async fn assert_refusal(logs: &Logs, case: &str, operation: &str, body: &Value, code: &str, message: &str) {
    let (status, parsed) = logs.call(operation, body).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?} on {operation}: expected HTTP {REFUSAL_STATUS}, got {status} {parsed}"
    );
    let actual_code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: a refusal carries __type, got {parsed}"));
    let actual_code = actual_code
        .rsplit(['#', '/'])
        .next()
        .unwrap_or(actual_code);
    assert_eq!(
        actual_code, code,
        "case {case:?} on {operation}: expected {code}, got {actual_code} {parsed}"
    );
    let actual_message = parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: a refusal carries a message, got {parsed}"));
    assert_eq!(
        actual_message, message,
        "case {case:?} on {operation}: expected the refusal {message:?}, got {actual_message:?}"
    );
}

/// The query the account holds in the resolution sweep, and one whose case a row varies.
const HELD: &str = "orders-errors-hourly";

/// An identifier the account holds no scheduled query under is refused as a missing resource —
/// on every operation that addresses a query by one, whatever form the identifier takes — and
/// the queries the account holds are untouched.
#[tokio::test]
async fn an_identifier_holding_no_query_is_refused_on_every_operation_that_addresses_one() {
    let logs = Logs::new();
    create(&logs, HELD).await;
    let before = listed(&logs).await;

    // One row per shape of identifier the rule covers: a name never created and the ARN built
    // from it, a held query's name differing only in case, the ARN of another resource kind
    // entirely, an ARN naming another Region and account, a value that is neither a name nor an
    // ARN, and the shortest and longest identifiers the shape admits.
    let rows: Vec<(&str, String)> = vec![
        ("a name no query was created under", "orders-missing".to_string()),
        (
            "the ARN form of a name no query was created under",
            scheduled_query_arn(REGION, "orders-missing"),
        ),
        (
            "a held query's name differing only in case",
            HELD.to_uppercase(),
        ),
        (
            "the ARN of another resource kind",
            log_group_arn(REGION, "/aws/lambda/orders", false),
        ),
        (
            "an ARN naming another Region and account",
            "arn:aws:logs:us-east-1:123456789012:scheduled-query:orders-missing".to_string(),
        ),
        (
            "a value that is neither a name nor an ARN",
            "not-an-arn-and-not-a-name".to_string(),
        ),
        (
            "the shortest identifier the shape admits (@length min 1)",
            "q".repeat(IDENTIFIER_MIN_LENGTH),
        ),
        (
            "the longest identifier the shape admits (@length max 300)",
            "q".repeat(IDENTIFIER_MAX_LENGTH),
        ),
    ];

    for (case, identifier) in rows {
        for operation in ADDRESSED_BY_IDENTIFIER {
            let expected = not_found_message(&identifier);
            assert_refusal(
                &logs,
                case,
                operation,
                &request(operation, Some(&identifier)),
                NOT_FOUND,
                &expected,
            )
            .await;
        }

        // Nothing a refused call named was created, written or removed.
        let after = listed(&logs).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused call leaves the account's queries as they were, expected \
             {before:?}, got {after:?}"
        );
        // The held query still answers its own report, so no refused update reached it.
        let report = logs.ok(GET, &json!({"identifier": HELD})).await;
        let query_string = report.get("queryString").and_then(Value::as_str);
        assert_eq!(
            query_string,
            Some("fields @timestamp, @message"),
            "case {case:?}: a refused update writes nothing, expected the created query string, \
             got {query_string:?}"
        );
    }
}

/// A delete answers its empty response only for a record it actually removed: the first delete
/// of a held query is accepted with an empty body, and every later delete of that identifier —
/// under either form, and whatever the name's length — is refused as a query that is not there.
#[tokio::test]
async fn a_delete_answers_an_empty_response_only_for_the_record_it_removed() {
    // The name the query is created under, and the form of the identifier the delete addresses
    // it by: an ordinary name under each form, and a name at each length bound the shape
    // publishes, so the rule is asserted across the span of admissible names. A name at the
    // maximum has no admissible ARN form — the ARN is built from the name, so it is longer than
    // the identifier the shape admits — which is why that row is addressed by its name.
    let rows: [(&str, String, bool); 4] = [
        (
            "an ordinary name, deleted by its name",
            HELD.to_string(),
            false,
        ),
        (
            "an ordinary name, deleted by its ARN",
            "payments-latency-daily".to_string(),
            true,
        ),
        (
            "the shortest name the shape admits (@length min 1), deleted by its ARN",
            "q".repeat(IDENTIFIER_MIN_LENGTH),
            true,
        ),
        (
            "the longest name the shape admits (@length max 300)",
            "q".repeat(IDENTIFIER_MAX_LENGTH),
            false,
        ),
    ];

    for (case, name, by_arn) in rows.iter() {
        let logs = Logs::new();
        create(&logs, name).await;
        let arn = scheduled_query_arn(REGION, name);
        let identifier = if *by_arn { arn.clone() } else { name.clone() };

        let (status, body) = logs
            .call(DELETE, &json!({"identifier": identifier}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete that removes the record is accepted, expected HTTP 200, \
             got {status} {body}"
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(Map::is_empty),
            "case {case:?}: the response of {DELETE} carries no members, got {body}"
        );
        let held = listed(&logs).await;
        assert!(
            held.is_empty(),
            "case {case:?}: the removed query is reported by no reader, got {held:?}"
        );

        // Every later call addressing the removed query — twice over, so a refusal that only
        // fires once cannot pass — is refused in the sentence naming the identifier it asked
        // for, under either form of it.
        //
        // Both forms of the identifier are asked, except where the ARN built from the name is
        // itself past the length the shape admits, which no request can carry.
        let mut forms = vec![name.clone()];
        if arn.chars().count() <= IDENTIFIER_MAX_LENGTH {
            forms.push(arn.clone());
        }
        for attempt in 1..=2 {
            for form in forms.clone() {
                for operation in ADDRESSED_BY_IDENTIFIER {
                    let case = format!("{case} (attempt {attempt}, {form:?})");
                    let expected = not_found_message(&form);
                    assert_refusal(
                        &logs,
                        &case,
                        operation,
                        &request(operation, Some(&form)),
                        NOT_FOUND,
                        &expected,
                    )
                    .await;
                }
            }
        }

        // A create under the freed name is accepted again, and the delete of THAT record is the
        // only call that answers an empty response for it.
        create(&logs, name).await;
        let (status, body) = logs
            .call(DELETE, &json!({"identifier": identifier}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete of the recreated record is accepted, expected HTTP 200, \
             got {status} {body}"
        );
        let expected = not_found_message(&identifier);
        assert_refusal(
            &logs,
            case,
            DELETE,
            &request(DELETE, Some(&identifier)),
            NOT_FOUND,
            &expected,
        )
        .await;
    }
}

/// An identifier outside the length the model states for the shape never reaches the resolution
/// at all: it is refused as a constraint failure on that member — on every operation addressed
/// by one — while an identifier ON either published bound reaches the operation, which answers
/// about the query rather than about the member.
#[tokio::test]
async fn an_identifier_outside_the_modelled_length_is_refused_on_the_member() {
    let logs = Logs::new();
    create(&logs, HELD).await;

    // Source: `ScheduledQueryIdentifier` is `@length(min: 1, max: 300)` and the member is
    // `@required`, so these are the three ways a request can fail the member itself.
    let rows: Vec<(&str, Option<String>, String)> = vec![
        (
            "no identifier at all",
            None,
            constraint_refusal("Member must not be null"),
        ),
        (
            "an empty identifier, one character short of the minimum",
            Some(String::new()),
            constraint_refusal(&format!(
                "Member must have length greater than or equal to {IDENTIFIER_MIN_LENGTH}"
            )),
        ),
        (
            "an identifier one character past the maximum",
            Some("q".repeat(IDENTIFIER_MAX_LENGTH + 1)),
            constraint_refusal(&format!(
                "Member must have length less than or equal to {IDENTIFIER_MAX_LENGTH}"
            )),
        ),
    ];

    for (case, identifier, message) in rows {
        for operation in ADDRESSED_BY_IDENTIFIER {
            assert_refusal(
                &logs,
                case,
                operation,
                &request(operation, identifier.as_deref()),
                VALIDATION,
                &message,
            )
            .await;
        }
    }

    // On the bounds themselves the request reaches the operation: the identifier is resolved,
    // and the answer is about the scheduled query rather than about the member.
    for (case, identifier) in [
        ("on the minimum length", "q".repeat(IDENTIFIER_MIN_LENGTH)),
        ("on the maximum length", "q".repeat(IDENTIFIER_MAX_LENGTH)),
    ] {
        let expected = not_found_message(&identifier);
        assert_refusal(
            &logs,
            case,
            DELETE,
            &request(DELETE, Some(&identifier)),
            NOT_FOUND,
            &expected,
        )
        .await;
    }

    // The query the account holds is still there, and still reports what its create stated: no
    // refused call wrote or removed anything.
    let held = listed(&logs).await;
    assert_eq!(
        held,
        vec![HELD.to_string()],
        "a refused call leaves the account's queries as they were, expected {:?}, got {held:?}",
        vec![HELD.to_string()]
    );
}
