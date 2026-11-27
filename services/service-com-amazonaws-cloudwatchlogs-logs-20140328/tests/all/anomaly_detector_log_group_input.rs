// SPDX-License-Identifier: CC0-1.0

//! What `CreateLogAnomalyDetector` says about the log groups it is asked to watch, end to end
//! over the wire.
//!
//! Three rules, each asserted over a table that spans what it generalises over:
//!
//! * the request has to PROVIDE the required input: a `logGroupArnList` carrying no ARN
//!   provides none of it, which is a request-validation failure — `ValidationException` with
//!   the sentence naming the input that must be provided — rather than a value the service
//!   rejected;
//! * "You can specify only one log group ARN", so a list naming several asks for a detector
//!   this operation does not create, and the refusal states that documented restriction;
//! * a detector "regularly scans one or more log groups" named by their ARNs, so an ARN that
//!   resolves to no log group of this account in this Region is a missing resource, and the
//!   refusal names the identifier the request wrote and the account that would own that group.
//!
//! A refused create persists nothing, which is read back through `ListLogAnomalyDetectors`,
//! and the accepted create is proved through `GetLogAnomalyDetector`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// The operations under test and the reads a create is proved through.
const CREATE: &str = "CreateLogAnomalyDetector";
const GET: &str = "GetLogAnomalyDetector";
const LIST: &str = "ListLogAnomalyDetectors";
const CREATE_GROUP: &str = "CreateLogGroup";

/// The codes this surface answers: a request-validation failure, a value the service will not
/// accept, and a resource that is not there.
const VALIDATION: &str = "ValidationException";
const INVALID_PARAMETER: &str = "InvalidParameterException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// AWS's fixed sentence for a create that provided no log group ARN at all.
const LOG_GROUPS_REQUIRED: &str = "LogGroupArn(s) must be provided.";

/// The restriction `logGroupArnList` documents, which is what a create naming several log
/// groups is refused with.
const ONE_LOG_GROUP: &str = "You can specify only one log group ARN.";

/// A log group's ARN without the trailing `:*`, which is the form `logGroupArnList` carries.
fn group_arn(region: &str, account: &str, name: &str) -> String {
    format!("arn:aws:logs:{region}:{account}:log-group:{name}")
}

/// The account the emulator serves, which is the owner a missing-group refusal names.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// Create the log group `name` and answer the ARN form a detector watches it by.
async fn watched_group(logs: &Logs, name: &str) -> String {
    logs.ok(CREATE_GROUP, &json!({"logGroupName": name})).await;
    let (_wildcard, arn) = logs.group_arns(name).await;
    arn
}

/// The detectors the account holds, through the wire enumeration.
async fn detectors(logs: &Logs) -> Vec<Value> {
    let page = logs.ok(LIST, &json!({})).await;
    page.get("anomalyDetectors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A create that provides no log group ARN is refused as the missing required input it is —
/// `ValidationException` with AWS's sentence — whatever else the request carries, and persists
/// nothing.
#[tokio::test]
async fn a_create_that_provides_no_log_group_is_refused_as_a_missing_input() {
    let logs = Logs::new();
    let watched = watched_group(&logs, "anomaly/provided").await;

    // The rows differ in what the request carries BESIDE the empty list: nothing, a detector
    // name, the two configuration members the service defaults, a filter pattern and tags.
    // The rule is about the list, so every row is answered identically.
    let bodies: Vec<(&str, Value)> = vec![
        ("the empty list alone", json!({"logGroupArnList": []})),
        (
            "the empty list with a detector name",
            json!({"logGroupArnList": [], "detectorName": "named"}),
        ),
        (
            "the empty list with the configuration members",
            json!({
                "logGroupArnList": [],
                "evaluationFrequency": "FIFTEEN_MIN",
                "anomalyVisibilityTime": 21,
            }),
        ),
        (
            "the empty list with a filter pattern and tags",
            json!({
                "logGroupArnList": [],
                "filterPattern": "ERROR",
                "tags": {"team": "platform"},
            }),
        ),
    ];

    for (case, body) in &bodies {
        let (code, message) = logs.refusal(CREATE, body).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (VALIDATION, LOG_GROUPS_REQUIRED),
            "case {case:?}: expected ({VALIDATION}, {LOG_GROUPS_REQUIRED:?}), got \
             ({code}, {message:?})"
        );
    }

    // The same request with the one log group it has to name is accepted, so none of the rows
    // above was refused for the shape of the call.
    let created = logs
        .ok(CREATE, &json!({"logGroupArnList": [watched]}))
        .await;
    assert!(
        created.get("anomalyDetectorArn").is_some(),
        "case \"one log group named\": the create answers the detector's ARN, got {created}"
    );
    assert_eq!(
        detectors(&logs).await.len(),
        1,
        "case \"after the refusals\": only the accepted create stored a detector"
    );
}

/// A create naming more log groups than one detector watches is refused with the documented
/// restriction, and stores nothing.
#[tokio::test]
async fn a_create_naming_several_log_groups_is_refused() {
    let logs = Logs::new();
    let first = watched_group(&logs, "anomaly/one").await;
    let second = watched_group(&logs, "anomaly/two").await;
    let third = watched_group(&logs, "anomaly/three").await;

    // Two and three ARNs, and the same ARN twice: the rule counts the entries the list
    // carries, not the distinct groups they name.
    let lists: Vec<(&str, Value)> = vec![
        ("two log groups", json!([first, second])),
        ("three log groups", json!([first, second, third])),
        ("the same log group twice", json!([first, first])),
    ];

    for (case, list) in &lists {
        let (code, message) = logs
            .refusal(CREATE, &json!({"logGroupArnList": list}))
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, ONE_LOG_GROUP),
            "case {case:?}: expected ({INVALID_PARAMETER}, {ONE_LOG_GROUP:?}), got \
             ({code}, {message:?})"
        );
    }
    assert_eq!(
        detectors(&logs).await,
        Vec::<Value>::new(),
        "case \"after the refusals\": a refused create stores no detector"
    );

    // One of those ARNs on its own is accepted, so the rule is the count and nothing else.
    logs.ok(CREATE, &json!({"logGroupArnList": [first]})).await;
    assert_eq!(
        detectors(&logs).await.len(),
        1,
        "case \"one of the same log groups\": the create is accepted"
    );
}

/// A `logGroupArnList` ARN that resolves to no log group this account holds in this Region is
/// refused as the missing resource it is, and the refusal names the identifier the request
/// wrote and the account that would own that group.
#[tokio::test]
async fn a_log_group_the_account_does_not_hold_is_named_by_the_refusal() {
    let logs = Logs::new();
    let held_name = "anomaly/watched";
    let held = watched_group(&logs, held_name).await;

    // The rows differ along what makes the ARN resolve to nothing: a name never created, a
    // held name in another case, a name carrying separators, the held group named in another
    // Region, and the held group named in another account. Each is still a well-formed log
    // group ARN, so the refusal is about the resource and not about the value's form.
    let absent: Vec<(&str, String)> = vec![
        (
            "a name never created",
            group_arn(REGION, &own_account(), "anomaly/never-created"),
        ),
        (
            "a held name in another case",
            group_arn(REGION, &own_account(), "ANOMALY/WATCHED"),
        ),
        (
            "a name carrying separators and punctuation",
            group_arn(REGION, &own_account(), "team/one.group-2_final"),
        ),
        (
            "the held group in another Region",
            group_arn("us-east-1", &own_account(), held_name),
        ),
        (
            "the held group in another account",
            group_arn(REGION, "123456789012", held_name),
        ),
    ];

    for (case, arn) in &absent {
        let expected = format!(
            "Log group '{arn}' for owner '{}' cannot be found.",
            own_account()
        );
        let (code, message) = logs
            .refusal(CREATE, &json!({"logGroupArnList": [arn]}))
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (NOT_FOUND, expected.as_str()),
            "case {case:?}: expected ({NOT_FOUND}, {expected:?}), got ({code}, {message:?})"
        );
    }
    assert_eq!(
        detectors(&logs).await,
        Vec::<Value>::new(),
        "case \"after the refusals\": a refused create stores no detector"
    );

    // The group the account DOES hold is accepted, and the detector it created is readable
    // back under the ARN the create answered.
    let created = logs
        .ok(CREATE, &json!({"logGroupArnList": [held]}))
        .await;
    let arn = created
        .get("anomalyDetectorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the create answers an anomalyDetectorArn, got {created}"))
        .to_string();
    let read = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
    assert_eq!(
        read.get("logGroupArnList"),
        Some(&json!([held])),
        "case \"the held log group\": the detector watches the group the create named, got {read}"
    );
}
