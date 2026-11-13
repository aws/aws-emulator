// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEXT of this service's refusals: for each error class, the
//! message CloudWatch Logs words it with — its fixed text, plus the variable parts it takes
//! from the request and from the store, and nothing else.
//!
//! The rules under test, one per suite below:
//!
//! * a missing resource the request identifies nothing of (the account-level policy a
//!   `(policyType, policyName)` pair names) is refused with the fixed missing-resource
//!   sentence of its class, interpolating neither member;
//! * a request over a quota is refused with the fixed text of the limit class, naming
//!   neither the quota nor the resource it belongs to;
//! * a `policyDocument` that cannot be READ as the JSON structure its policy type requires
//!   is refused with the fixed text for a malformed document, while a readable document
//!   rejected for WHAT it says keeps the sentence of the rule it breaks;
//! * a value outside a `@pattern` states the expression AWS PUBLISHES for that shape, which
//!   for a log group, log stream, metric-filter and destination name is the model's
//!   expression without its `^`…`$` anchors;
//! * a log group or log stream name past the 512 characters either may have is refused with
//!   the service's own sentence for that pair, while another member's length bound keeps the
//!   constraint template;
//! * a resource the request DOES identify is named in the refusal: an integration by its
//!   name, a log anomaly detector by the ARN that was looked for, a transformer by the log
//!   group it was looked for on, and a log group a data protection policy read cannot find
//!   by its name and its owning account.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! message a handler does not actually answer cannot pass.
//!
//! Each table spans what its rule generalises over — the policy type, the operation, the
//! member, the identifier form, the way the resource came to be absent, the form of the
//! value that was rejected — rather than the one request any single report happens to cite,
//! which is one row of each.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    anomaly_detector_arn, log_group_arn, INTEGRATION_TYPE_OPENSEARCH,
};

use crate::account_policies::{delete_body, put_body, POLICY_TYPES};
use crate::account_policy_selection_criteria::document_for;
use crate::harness::{Logs, REGION};

/// The code CloudWatch Logs answers when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when a parameter is specified incorrectly.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when a request would take the account past the resources it may hold.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The code the delivery family declares for a refusal — the only refusal shape in its
/// modelled error set, used by the boundary row of the length rule.
const VALIDATION: &str = "ValidationException";

/// The account every ARN this emulator mints carries, which is also the owner a data
/// protection policy read names.
const ACCOUNT: &str = "000000000000";

/// The fixed missing-resource sentence, as the data protection path of this service inflects
/// it.
const DATA_PROTECTION_NOT_FOUND: &str = "Specified resource does not exists!";

/// The fixed missing-resource sentence everywhere else.
const UNIDENTIFIED_NOT_FOUND: &str = "Specified resource does not exist!";

/// The fixed text of the limit class.
const LIMIT_TEXT: &str = "Resource limit exceeded.";

/// The fixed text for a `policyDocument` the service cannot read as the structure its policy
/// type requires.
const MALFORMED_DOCUMENT: &str = "Policy document contains invalid JSON format.";

/// The whole message an over-long log group or log stream name is refused with. The 512 is
/// the `@length` maximum the model declares on both `LogGroupName` and `LogStreamName`.
const LOG_NAME_LENGTH_TEXT: &str =
    "Invalid LogGroup or LogStream, both must have length less than or equal to 512.";

/// The characters a log group name and a log stream name may each be long. Source: the
/// model's `LogGroupName` and `LogStreamName` shapes (`@length` 1..512 on each).
const MAX_LOG_NAME_CHARS: usize = 512;

/// The expression AWS publishes for `LogStreamName`, which `FilterName` and
/// `DestinationName` share: the model's `^[^:*]*$` without its anchors.
const LOG_STREAM_NAME_PATTERN: &str = r"[^:*]*";

/// The expression AWS publishes for `LogGroupName`: the model's `^[\.\-_/#A-Za-z0-9]+$`
/// without its anchors.
const LOG_GROUP_NAME_PATTERN: &str = r"[\.\-_/#A-Za-z0-9]+";

/// The clause one refused member is stated in, inside the envelope AWS wraps one violation
/// in.
///
/// `pub(crate)` so the one statement of that envelope is shared with the refusal-text suite
/// rather than transcribed twice.
pub(crate) fn one_violation(member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// The requirement clause a value outside `pattern` states. `pub(crate)` for the same reason
/// as [`one_violation`] above.
pub(crate) fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// Create one log group and answer it, so a case about something else is not refused for the
/// group.
async fn create_group(logs: &Logs, name: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name})).await;
}

/// The fixed sentence a delete of an account-level policy of `policy_type` the account does
/// not hold is refused with.
fn account_policy_not_found(policy_type: &str) -> &'static str {
    if policy_type == "DATA_PROTECTION_POLICY" {
        DATA_PROTECTION_NOT_FOUND
    } else {
        UNIDENTIFIED_NOT_FOUND
    }
}

/// A delete of a `(policyType, policyName)` pair the account does not hold states the fixed
/// missing-resource sentence of its class and interpolates neither member — for every
/// modelled policy type, whether the pair was never put or was put and deleted, and whatever
/// the name looks like.
#[tokio::test]
async fn an_account_policy_delete_states_the_fixed_text_of_its_class() {
    let logs = Logs::new();

    for policy_type in POLICY_TYPES {
        let expected = account_policy_not_found(policy_type);

        // One pair that was put and deleted, so the table has an already-removed row beside
        // the never-put ones; the accepted put and delete also show the refusals below follow
        // the store.
        let held = "put-then-deleted";
        logs.ok(
            "PutAccountPolicy",
            &put_body(held, policy_type, &document_for(policy_type), None),
        )
        .await;
        logs.ok("DeleteAccountPolicy", &delete_body(held, policy_type))
            .await;

        for (case, policy_name) in [
            ("already deleted", held.to_string()),
            ("never put", "never-put".to_string()),
            ("a name differing only in case", held.to_ascii_uppercase()),
            ("a name with a dot and a slash", "team.a/policy".to_string()),
            ("the empty name", String::new()),
        ] {
            let (code, message) = logs
                .refusal("DeleteAccountPolicy", &delete_body(&policy_name, policy_type))
                .await;
            assert_eq!(
                code, NOT_FOUND,
                "case {policy_type} {case:?}: expected {NOT_FOUND}, got {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {policy_type} {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(policy_type),
                "case {policy_type} {case:?}: the fixed text names no policy type, got {message:?}"
            );
            assert!(
                policy_name.is_empty() || !message.contains(&policy_name),
                "case {policy_type} {case:?}: the fixed text names no policy, got {message:?}"
            );
        }
    }
}

/// A put that would take the account past the policies of one type it may hold is refused
/// with the fixed text of the limit class — for a type whose quota is one unscoped policy and
/// for a type whose quota is a number of scoped ones — naming neither the quota nor the type.
#[tokio::test]
async fn a_put_over_a_policy_quota_states_the_fixed_text_of_its_class() {
    // (case, the policy type, how many policies of it the account may hold, whether each is
    // scoped). Source: `PutAccountPolicy`'s quota sentences — "Each account can have only one
    // account-level data protection policy", "one account-level subscription filter policy per
    // Region", and for a metric extraction policy "up to 5 policies that are each scoped to a
    // subset of log groups".
    let rows: [(&str, &str, usize, bool); 3] = [
        ("one unscoped data protection policy", "DATA_PROTECTION_POLICY", 1, false),
        (
            "one unscoped subscription filter policy",
            "SUBSCRIPTION_FILTER_POLICY",
            1,
            false,
        ),
        (
            "five scoped metric extraction policies",
            "METRIC_EXTRACTION_POLICY",
            5,
            true,
        ),
    ];

    for (case, policy_type, quota, scoped) in rows {
        let logs = Logs::new();
        let document = document_for(policy_type);
        for index in 0..quota {
            let criteria = scoped.then(|| format!(r#"LogGroupNamePrefix IN ["/quota/{index}"]"#));
            logs.ok(
                "PutAccountPolicy",
                &put_body(
                    &format!("held-{index}"),
                    policy_type,
                    &document,
                    criteria.as_deref(),
                ),
            )
            .await;
        }

        let criteria = scoped.then(|| r#"LogGroupNamePrefix IN ["/quota/over"]"#.to_string());
        let (code, message) = logs
            .refusal(
                "PutAccountPolicy",
                &put_body("one-too-many", policy_type, &document, criteria.as_deref()),
            )
            .await;
        assert_eq!(
            code, LIMIT_EXCEEDED,
            "case {case:?}: expected {LIMIT_EXCEEDED}, got {code} {message:?}"
        );
        assert_eq!(
            message, LIMIT_TEXT,
            "case {case:?}: expected {LIMIT_TEXT:?}, got {message:?}"
        );
        assert!(
            !message.contains(policy_type) && !message.contains(&quota.to_string()),
            "case {case:?}: the fixed text names neither the type nor the quota, got {message:?}"
        );
    }
}

/// A field index policy document whose JSON is not the structure the type's grammar states is
/// refused with the fixed malformed-document text, on the account-level put and on the
/// log-group-level put alike; a document the service CAN read and then rejects for what it
/// says keeps the sentence of the rule it breaks.
#[tokio::test]
async fn a_policy_document_in_the_wrong_json_form_states_the_malformed_text() {
    const GROUP: &str = "/wording/index-policy";
    let logs = Logs::new();
    create_group(&logs, GROUP).await;

    // The forms a document can fail to BE the structure the grammar states: not JSON at all,
    // JSON that is not the policy object, and a member carrying a JSON type the grammar does
    // not admit.
    let malformed: Vec<(&str, String)> = vec![
        ("not JSON at all", "not a policy".to_string()),
        ("a JSON array rather than the object", json!(["Fields"]).to_string()),
        ("a JSON string rather than the object", json!("Fields").to_string()),
        ("an empty object", json!({}).to_string()),
        ("Fields written as a string", json!({"Fields": "requestId"}).to_string()),
        ("Fields written as an object", json!({"Fields": {"a": 1}}).to_string()),
        (
            "a Fields element that is not a field name",
            json!({"Fields": ["requestId", 7]}).to_string(),
        ),
        (
            "FieldsV2 written as an array",
            json!({"FieldsV2": [{"RequestId": {"type": "FACET"}}]}).to_string(),
        ),
    ];

    for (case, document) in &malformed {
        for (operation, body) in [
            (
                "PutAccountPolicy",
                put_body("wording", "FIELD_INDEX_POLICY", document, None),
            ),
            (
                "PutIndexPolicy",
                json!({"logGroupIdentifier": GROUP, "policyDocument": document}),
            ),
        ] {
            let (code, message) = logs.refusal(operation, &body).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?} ({operation}): expected {INVALID_PARAMETER}, got {code} {message:?}"
            );
            assert_eq!(
                message, MALFORMED_DOCUMENT,
                "case {case:?} ({operation}): expected {MALFORMED_DOCUMENT:?}, got {message:?}"
            );
        }
    }

    // The boundary: a document the service reads and then rejects for WHAT IT SAYS states that
    // rule instead — the type's own documented requirements ("It must contain at least one
    // field index", "Supported types are FIELD_INDEX and FACET").
    let readable: Vec<(&str, String, &str)> = vec![
        (
            "no field index at all",
            json!({"Fields": []}).to_string(),
            "at least one field index",
        ),
        (
            "a type outside the index types",
            json!({"FieldsV2": {"RequestId": {"type": "SORT_KEY"}}}).to_string(),
            "Valid values are:",
        ),
    ];
    for (case, document, states) in &readable {
        let (code, message) = logs
            .refusal(
                "PutAccountPolicy",
                &put_body("wording", "FIELD_INDEX_POLICY", document, None),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_ne!(
            message, MALFORMED_DOCUMENT,
            "case {case:?}: a readable document is not refused as malformed, got {message:?}"
        );
        assert!(
            message.contains(states),
            "case {case:?}: expected the refusal to state {states:?}, got {message:?}"
        );
    }

    // A document the grammar DOES admit is still accepted, so none of the above refused on the
    // form of the call rather than on the document.
    logs.ok(
        "PutIndexPolicy",
        &json!({
            "logGroupIdentifier": GROUP,
            "policyDocument": json!({"Fields": ["requestId"]}).to_string(),
        }),
    )
    .await;
}

/// A log group or log stream name past the 512 characters either may have is refused with the
/// service's own sentence for that pair — whichever operation and member carried it — while a
/// name AT the bound is not refused and another member's length bound keeps the constraint
/// template.
#[tokio::test]
async fn an_over_long_log_name_states_the_services_own_length_text() {
    const GROUP: &str = "/wording/length";
    let logs = Logs::new();
    create_group(&logs, GROUP).await;

    let over_group = "g".repeat(MAX_LOG_NAME_CHARS + 1);
    let far_over_group = "g".repeat(MAX_LOG_NAME_CHARS * 2);
    let over_stream = "s".repeat(MAX_LOG_NAME_CHARS + 1);

    // One row per member the two shapes are reached by, and per form of an over-long value
    // (one character past the bound, and far past it).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "CreateLogGroup one character past the bound",
            "CreateLogGroup",
            json!({"logGroupName": over_group}),
        ),
        (
            "CreateLogGroup far past the bound",
            "CreateLogGroup",
            json!({"logGroupName": far_over_group}),
        ),
        (
            "CreateLogStream logStreamName past the bound",
            "CreateLogStream",
            json!({"logGroupName": GROUP, "logStreamName": over_stream}),
        ),
        (
            "GetLogEvents logStreamName past the bound",
            "GetLogEvents",
            json!({"logGroupName": GROUP, "logStreamName": over_stream}),
        ),
        (
            "DescribeLogStreams logStreamNamePrefix past the bound",
            "DescribeLogStreams",
            json!({"logGroupName": GROUP, "logStreamNamePrefix": over_stream}),
        ),
        (
            "DescribeLogGroups logGroupNamePrefix past the bound",
            "DescribeLogGroups",
            json!({"logGroupNamePrefix": over_group}),
        ),
        (
            "PutLogEvents logGroupName past the bound",
            "PutLogEvents",
            json!({
                "logGroupName": over_group,
                "logStreamName": "a-stream",
                "logEvents": [{"timestamp": crate::harness::now_ms(), "message": "e"}],
            }),
        ),
    ];

    for (case, operation, body) in &rows {
        let (code, message) = logs.refusal(operation, body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_eq!(
            message, LOG_NAME_LENGTH_TEXT,
            "case {case:?}: expected {LOG_NAME_LENGTH_TEXT:?}, got {message:?}"
        );
    }

    // At the bound neither name is refused: the sentence above is the bound and not the length
    // of any one name.
    let longest_group = "g".repeat(MAX_LOG_NAME_CHARS);
    logs.ok("CreateLogGroup", &json!({"logGroupName": longest_group}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({
            "logGroupName": longest_group,
            "logStreamName": "s".repeat(MAX_LOG_NAME_CHARS),
        }),
    )
    .await;

    // The boundary: a member that is neither name states the constraint template, with its own
    // bound. Source: the model's `DeliveryId` shape (`@length` 1..64).
    let (code, message) = logs
        .refusal("DeleteDelivery", &json!({"id": "a".repeat(65)}))
        .await;
    let expected = one_violation("id", "Member must have length less than or equal to 64");
    assert_eq!(
        code, VALIDATION,
        "case \"a member that is neither name\": expected {VALIDATION}, got {code} {message:?}"
    );
    assert_eq!(
        message, expected,
        "case \"a member that is neither name\": expected {expected:?}, got {message:?}"
    );
}

/// A value outside a `@pattern` states the expression AWS PUBLISHES for that shape: for a log
/// stream, metric-filter and destination name, and for a log group name, the model's
/// expression with its `^`…`$` anchors dropped — whichever operation and member carried the
/// value, and whichever character put it outside the class.
#[tokio::test]
async fn a_value_outside_a_pattern_states_the_published_expression() {
    const GROUP: &str = "/wording/pattern";
    let logs = Logs::new();
    create_group(&logs, GROUP).await;

    // (case, operation, body, the member refused, the published expression it states)
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a log stream name with a colon",
            "CreateLogStream",
            json!({"logGroupName": GROUP, "logStreamName": "has:colon"}),
            "logStreamName",
            LOG_STREAM_NAME_PATTERN,
        ),
        (
            "a log stream name with a star",
            "GetLogEvents",
            json!({"logGroupName": GROUP, "logStreamName": "has*star"}),
            "logStreamName",
            LOG_STREAM_NAME_PATTERN,
        ),
        (
            "a log stream name prefix with a colon",
            "DescribeLogStreams",
            json!({"logGroupName": GROUP, "logStreamNamePrefix": "pre:fix"}),
            "logStreamNamePrefix",
            LOG_STREAM_NAME_PATTERN,
        ),
        (
            "a metric filter name with a star",
            "DeleteMetricFilter",
            json!({"logGroupName": GROUP, "filterName": "filter*"}),
            "filterName",
            LOG_STREAM_NAME_PATTERN,
        ),
        (
            "a destination name with a colon",
            "DeleteDestination",
            json!({"destinationName": "dest:1"}),
            "destinationName",
            LOG_STREAM_NAME_PATTERN,
        ),
        (
            "a log group name with a space and a bang",
            "CreateLogGroup",
            json!({"logGroupName": "bad name!"}),
            "logGroupName",
            LOG_GROUP_NAME_PATTERN,
        ),
        (
            "a log group name with a colon",
            "DeleteLogGroup",
            json!({"logGroupName": "has:colon"}),
            "logGroupName",
            LOG_GROUP_NAME_PATTERN,
        ),
    ];

    for (case, operation, body, member, pattern) in &rows {
        let (code, message) = logs.refusal(operation, body).await;
        let expected = one_violation(member, &pattern_requirement(pattern));
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains("^[") && !message.ends_with('$'),
            "case {case:?}: the published expression carries no anchors, got {message:?}"
        );
    }

    // The boundary: a shape AWS publishes no other form for states the model's expression as
    // it is spelled, anchors included. Source: the model's `DeliveryId` (`^[0-9A-Za-z]+$`).
    let (_, message) = logs
        .refusal("GetDelivery", &json!({"id": "delivery-1"}))
        .await;
    let expected = one_violation("id", &pattern_requirement("^[0-9A-Za-z]+$"));
    assert_eq!(
        message, expected,
        "case \"a shape with no published form\": expected {expected:?}, got {message:?}"
    );
}

/// An `integrationName` the account does not hold is refused by NAMING that integration, on
/// the read and on the delete alike, whatever the name looks like and however it came to be
/// absent.
#[tokio::test]
async fn an_integration_the_account_does_not_hold_is_refused_by_name() {
    let logs = Logs::new();

    // One integration that was created and deleted, so the table has an already-removed row
    // beside the never-created ones.
    let created = "wording-integration";
    logs.ok(
        "PutIntegration",
        &json!({
            "integrationName": created,
            "integrationType": INTEGRATION_TYPE_OPENSEARCH,
            "resourceConfig": {"openSearchResourceConfig": {
                "dataSourceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/CloudWatchOpenSearch"),
                "dashboardViewerPrincipals": [format!("arn:aws:iam::{ACCOUNT}:user/viewer")],
                "retentionDays": 30,
            }},
        }),
    )
    .await;
    logs.ok("GetIntegration", &json!({"integrationName": created}))
        .await;
    logs.ok("DeleteIntegration", &json!({"integrationName": created}))
        .await;

    let names: Vec<(&str, String)> = vec![
        ("already deleted", created.to_string()),
        ("never created", "nothing-created".to_string()),
        ("a name differing only in case", created.to_ascii_uppercase()),
        ("a dotted name", "team.a/integration".to_string()),
        ("the longest name IntegrationName admits", "n".repeat(50)),
    ];

    for (case, name) in &names {
        for operation in ["GetIntegration", "DeleteIntegration"] {
            let (code, message) = logs
                .refusal(operation, &json!({"integrationName": name}))
                .await;
            let expected = format!("Integration with name {name} does not exist.");
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?} ({operation}): expected {NOT_FOUND}, got {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} ({operation}): expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// An `anomalyDetectorArn` the account does not hold is refused by stating the ARN that was
/// looked for, on every operation that resolves one, whatever form the ARN takes and however
/// it came to be absent.
#[tokio::test]
async fn an_anomaly_detector_arn_the_account_does_not_hold_is_refused_by_arn() {
    const GROUP: &str = "/wording/detected";
    let logs = Logs::new();
    create_group(&logs, GROUP).await;
    let group_arn = log_group_arn(REGION, GROUP, false);

    // One detector the account holds, and one it held and deleted.
    let held = created_detector(&logs, &group_arn).await;
    let deleted = created_detector(&logs, &group_arn).await;
    logs.ok(
        "DeleteLogAnomalyDetector",
        &json!({"anomalyDetectorArn": deleted}),
    )
    .await;

    let detector_id = held
        .rsplit(':')
        .next()
        .expect("a detector ARN carries an id")
        .to_string();
    let absent: Vec<(&str, String)> = vec![
        ("a detector this account already deleted", deleted.clone()),
        (
            "an id nothing minted",
            anomaly_detector_arn(REGION, "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ),
        (
            "this detector's id in another Region",
            anomaly_detector_arn("us-east-1", &detector_id),
        ),
        (
            "this detector's id in another account",
            format!("arn:aws:logs:{REGION}:123456789012:anomaly-detector:{detector_id}"),
        ),
        ("an ARN of another resource kind", group_arn.clone()),
        ("a value that is not an ARN at all", "detector-1".to_string()),
    ];

    for (case, arn) in &absent {
        // One row per operation that resolves an `anomalyDetectorArn`: the read, the delete,
        // the update, the finding-suppression write and the anomaly enumeration.
        let requests: Vec<(&str, Value)> = vec![
            (
                "GetLogAnomalyDetector",
                json!({"anomalyDetectorArn": arn}),
            ),
            (
                "DeleteLogAnomalyDetector",
                json!({"anomalyDetectorArn": arn}),
            ),
            (
                "UpdateLogAnomalyDetector",
                json!({"anomalyDetectorArn": arn, "enabled": true}),
            ),
            (
                "UpdateAnomaly",
                json!({"anomalyDetectorArn": arn, "anomalyId": "a".repeat(36)}),
            ),
            ("ListAnomalies", json!({"anomalyDetectorArn": arn})),
        ];
        for (operation, body) in &requests {
            let (code, message) = logs.refusal(operation, body).await;
            let expected = format!("The specified ARN does not exist or already deleted : {arn}");
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?} ({operation}): expected {NOT_FOUND}, got {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} ({operation}): expected {expected:?}, got {message:?}"
            );
        }
    }

    // The detector the account DOES hold still answers, so none of the refusals above was the
    // shape of the call.
    logs.ok(
        "GetLogAnomalyDetector",
        &json!({"anomalyDetectorArn": held}),
    )
    .await;
}

/// Create one anomaly detector over `group_arn` and answer its ARN.
async fn created_detector(logs: &Logs, group_arn: &str) -> String {
    let created = logs
        .ok(
            "CreateLogAnomalyDetector",
            &json!({"logGroupArnList": [group_arn]}),
        )
        .await;
    created
        .get("anomalyDetectorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a create answers an anomalyDetectorArn, got {created}"))
        .to_string()
}

/// A delete of the transformer of a log group that holds none names the kind of resource that
/// is missing and the log group it was looked for on — whichever identifier form named that
/// group, and whether it never held one or held one a delete removed.
#[tokio::test]
async fn a_transformer_delete_names_the_group_it_found_none_on() {
    let logs = Logs::new();

    // The groups differ in what a name can carry, so the group in the message is the one the
    // request named rather than a fixed sentence.
    for group in [
        "/wording/transformer-a",
        "wording.transformer-b",
        "WordingTransformerC",
    ] {
        create_group(&logs, group).await;
        let (_, group_arn) = logs.group_arns(group).await;
        let expected = format!("Transformer does not exist on log group '{group}'");

        for (form, identifier) in [("name", group.to_string()), ("arn", group_arn.clone())] {
            let (code, message) = logs
                .refusal("DeleteTransformer", &json!({"logGroupIdentifier": identifier}))
                .await;
            assert_eq!(
                code, NOT_FOUND,
                "case {group} never held one, by {form}: expected {NOT_FOUND}, got {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {group} never held one, by {form}: expected {expected:?}, got {message:?}"
            );
        }

        // A group that HELD one and no longer does is refused identically: the refusal follows
        // the state.
        logs.ok(
            "PutTransformer",
            &json!({
                "logGroupIdentifier": group,
                "transformerConfig": [{"parseJSON": {"source": "@message"}}],
            }),
        )
        .await;
        logs.ok("DeleteTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        let (code, message) = logs
            .refusal("DeleteTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {group} deleted twice: expected {NOT_FOUND}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {group} deleted twice: expected {expected:?}, got {message:?}"
        );
    }
}

/// A data protection policy read of a log group the account does not hold names the group that
/// was looked for AND the account that would own it — whichever identifier form named it —
/// while a group that exists and holds no policy is still the fixed missing-resource text.
#[tokio::test]
async fn a_data_protection_read_names_the_missing_group_and_its_owner() {
    const PRESENT: &str = "/wording/holds-no-policy";
    let logs = Logs::new();
    create_group(&logs, PRESENT).await;

    for group in [
        "/wording/never-created",
        "wording.never-created",
        "WordingNeverCreated",
    ] {
        let arn = log_group_arn(REGION, group, false);
        let expected = format!("Log group '{group}' for owner '{ACCOUNT}' cannot be found");
        for (form, identifier) in [("name", group.to_string()), ("arn", arn)] {
            let (code, message) = logs
                .refusal(
                    "GetDataProtectionPolicy",
                    &json!({"logGroupIdentifier": identifier}),
                )
                .await;
            assert_eq!(
                code, NOT_FOUND,
                "case {group} by {form}: expected {NOT_FOUND}, got {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {group} by {form}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The boundary: the group is there and the POLICY is not, which the request identifies
    // nothing of — so the refusal is the fixed text, naming neither the group nor its owner.
    let (code, message) = logs
        .refusal(
            "GetDataProtectionPolicy",
            &json!({"logGroupIdentifier": PRESENT}),
        )
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case \"a group holding no policy\": expected {NOT_FOUND}, got {code} {message:?}"
    );
    assert_eq!(
        message, UNIDENTIFIED_NOT_FOUND,
        "case \"a group holding no policy\": expected {UNIDENTIFIED_NOT_FOUND:?}, got {message:?}"
    );
}
