// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE refusal this service answers a request body that does
//! not satisfy the schema its model publishes with, over the whole operation surface.
//!
//! The harness (the router, `call`, `create_body`, `Answer`) is the one
//! `replication_configuration_template_lifecycle` builds, and the tag map is
//! `launch_configuration_templates`' `one_tag`, so there is one assembly of the service and
//! one copy of each helper rather than two.
//!
//! Two sibling suites already hold this rule where their own subject met it:
//! `launch_configuration_templates`' rule 2 (every enumerated member of the launch-template
//! create, at the ways a value can fall outside its set) and
//! `replication_configuration_template_lifecycle`'s `requests_outside_a_modelled_constraint_are_refused`
//! (the replication create's own constraints). What is new here is the BREADTH the rule
//! claims and neither suite reaches: the operations the refusal spans — including four this
//! emulator has yet to write — the violation classes beyond an enumeration and a list length,
//! the depth one sits at, and both sides of each published bound.
//!
//! The rule under test: a value in the request body that falls outside a rule the model
//! declares for it — an enumeration's value set, a string's length or pattern, a number's
//! range, a list's or a map's length, or a member the shape requires that the body leaves
//! out, at any depth — is one failure of the body-schema check this service makes ahead of
//! any operation, and is answered the same way every time: HTTP 400, the error code
//! `BadRequestException`, and the fixed message `Invalid request body`, which names neither
//! the member, nor its value, nor the rule it failed. Nothing is created for such a request.
//! A body the schema ALLOWS — including one sitting exactly on a published bound — is handed
//! to the operation, which then answers on its own terms.
//!
//! The rows span that class rather than one request: every violation class the model
//! declares for this service, each on a different member, and both sides of each published
//! bound. They also cross implemented operations with operations this emulator has yet to
//! write, because the check is the SERVICE's and runs ahead of every handler — an unreadable
//! body is this refusal even where a readable one is still a 501.
//!
//! Every constraint a row cites comes from the model, read with
//! `smithy_query(query="operation", operation="<Operation>")` and
//! `smithy_query(query="shape", shape="PITPolicy,TagsMap,ARN,PITPolicyRule")` against this
//! crate's `smithy-model/`, never from the implementation. The enumeration value sets come
//! from the generated `ops::types` constants for the same reason.

use serde_json::{json, Value};
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::ops::types::{
    LAUNCH_DISPOSITION_VALUES, P_I_T_POLICY_RULE_UNITS_VALUES, RECOVERY_MODE_VALUES,
    REPLICATION_CONFIGURATION_EBS_ENCRYPTION_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::launch_configuration_templates::one_tag;
use crate::replication_configuration_template_lifecycle::{call, create_body, drs, items, Answer};

/// The error code and the whole message a body-schema refusal carries, written out as the
/// literals a client reads rather than taken from the implementation's constants.
///
/// `pub(crate)`: this suite owns the rule service-wide, so a suite asserting the refusal of
/// one operation's own request reads the same two literals rather than re-declaring them
/// (`crate::recovery_plan_update_bounds_and_surfaces`).
pub(crate) const BAD_REQUEST_CODE: &str = "BadRequestException";
pub(crate) const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// `PaginationToken` is `length(min: 0, max: 2048)`.
const PAGINATION_TOKEN_MAX: usize = 2048;

/// `DescribeLaunchConfigurationTemplates$maxResults` targets `MaxResultsType`,
/// `range(min: 1, max: 1000)`.
const DESCRIBE_TEMPLATES_MAX_RESULTS: (i64, i64) = (1, 1000);

/// `DescribeLaunchConfigurationTemplates$launchConfigurationTemplateIDs` is
/// `length(min: 0, max: 1)`.
const DESCRIBE_TEMPLATES_IDS_MAX: usize = 1;

/// `ReplicationServersSecurityGroupsIDs` is `length(min: 0, max: 32)`.
const SECURITY_GROUPS_MAX: i64 = 32;

/// `PITPolicy` is `length(min: 3, max: 3)`.
const PIT_POLICY_LENGTH: i64 = 3;

/// An `exportBucketArn` of exactly the modelled minimum length (`ARN` is
/// `length(min: 20, max: 2048)`, `pattern ^arn:.{16,2044}$`): `arn:` plus 16 characters.
const SHORTEST_ALLOWED_ARN: &str = "arn:aws:s3:::bucket1";

/// The same value one character short, so it satisfies neither the length minimum nor the
/// pattern's 16-character tail.
const TOO_SHORT_ARN: &str = "arn:aws:s3:::bucket";

/// A launch configuration template id matching the modelled `^lct-[0-9a-zA-Z]{17}$` that
/// nothing minted: enough for a body to be READABLE, which is all these rows need.
const UNMINTED_TEMPLATE_ID: &str = "lct-00000000000000000";

/// A source server id matching the modelled `^s-[0-9a-zA-Z]{17}$`.
const UNREGISTERED_SOURCE_SERVER_ID: &str = "s-0123456789abcdef0";

/// A staging-area subnet id matching the modelled `^subnet-[0-9a-fA-F]{8,}$`.
const SUBNET_ID: &str = "subnet-0123456789abcdef0";

/// `len` ASCII characters — a value whose only interesting property is its length.
fn text(len: usize) -> String {
    "a".repeat(len)
}

/// `count` `PITPolicyRule`s, every member inside its modelled range and the units taken
/// from the model's own value set — so the only thing a row varies is the list's length.
fn pit_rules(count: i64) -> Value {
    let rules: Vec<Value> = (0..count)
        .map(|index| {
            let units = P_I_T_POLICY_RULE_UNITS_VALUES
                [usize::try_from(index).unwrap_or(0) % P_I_T_POLICY_RULE_UNITS_VALUES.len()];
            json!({
                "ruleID": index + 1,
                "units": units,
                "interval": 1,
                "retentionDuration": 7,
                "enabled": true,
            })
        })
        .collect();
    Value::Array(rules)
}

/// `count` security group ids matching the modelled `^sg-[0-9a-fA-F]{8,}$`.
fn security_groups(count: i64) -> Value {
    let ids: Vec<Value> = (0..count)
        .map(|index| Value::String(format!("sg-{index:08x}")))
        .collect();
    Value::Array(ids)
}

/// Whether `answer` is the body-schema refusal: the status, the error code and the whole
/// message together, because a client branches on all three.
fn is_body_schema_refusal(answer: &Answer) -> bool {
    answer.status == 400
        && answer.error_code.as_deref() == Some(BAD_REQUEST_CODE)
        && answer.member("message").and_then(Value::as_str) == Some(INVALID_REQUEST_BODY)
}

/// Assert that `answer` is that refusal, naming the case and what came back instead.
fn assert_body_schema_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some(BAD_REQUEST_CODE),
        "case {case}: expected the error code {BAD_REQUEST_CODE}, got {answer:?}"
    );
    assert_eq!(
        answer.member("message").and_then(Value::as_str),
        Some(INVALID_REQUEST_BODY),
        "case {case}: expected the message {INVALID_REQUEST_BODY:?} and no constraint \
         detail, got {answer:?}"
    );
}

/// Every stored launch configuration template and replication configuration template, so a
/// refusal can be shown to have created nothing.
async fn stored_counts(router: &MultiServiceRouter) -> (usize, usize) {
    let templates = call(router, "DescribeLaunchConfigurationTemplates", &json!({})).await;
    assert_eq!(templates.status, 200, "describe templates: {templates:?}");
    let replication = call(
        router,
        "DescribeReplicationConfigurationTemplates",
        &json!({}),
    )
    .await;
    assert_eq!(
        replication.status, 200,
        "describe replication templates: {replication:?}"
    );
    (items(&templates).len(), items(&replication).len())
}

// ---------------------------------------------------------------------------
// The refusal
// ---------------------------------------------------------------------------

/// A body outside the model's schema is 400 `BadRequestException` / `Invalid request body`,
/// whatever the violated rule, the member, the depth it sits at, or the operation — and the
/// request creates nothing. The rows walk the violation classes this service's model
/// declares: an enumeration's value set, a string's length and pattern, an integer's range,
/// a list's length on both sides, a map key's and a map value's length, the same rules on a
/// member nested inside a list element, and a required member left out at both the top level
/// and inside a list element.
#[tokio::test]
async fn a_body_outside_the_models_schema_is_one_refusal_on_every_operation() {
    // The out-of-set values below are only violations while the model's value sets do not
    // list them, so the sets themselves are checked first.
    let outside: [(&str, &[&str], &str); 3] = [
        ("recoveryMode", RECOVERY_MODE_VALUES, "BOGUS_MODE"),
        ("launchDisposition", LAUNCH_DISPOSITION_VALUES, "started"),
        (
            "ebsEncryption",
            REPLICATION_CONFIGURATION_EBS_ENCRYPTION_VALUES,
            "MAYBE",
        ),
    ];
    for (member, listed, value) in outside {
        assert!(
            !listed.contains(&value),
            "case {member}: {value:?} must be outside the modelled value set {listed:?}"
        );
    }

    let long_token = text(PAGINATION_TOKEN_MAX + 1);

    // A valid pitPolicy whose SECOND rule has an interval of 0, below the modelled
    // `range(min: 1)` of `StrictlyPositiveInteger`: the violation is nested in a list
    // element, not at the top level.
    let mut rule_below_range = pit_rules(PIT_POLICY_LENGTH);
    rule_below_range.as_array_mut().expect("the rules")[1]["interval"] = json!(0);

    // The same, with the `@required` `units` member taken out of one rule.
    let mut rule_missing_required = pit_rules(PIT_POLICY_LENGTH);
    rule_missing_required.as_array_mut().expect("the rules")[2]
        .as_object_mut()
        .expect("a rule")
        .remove("units");

    // A create body with the model-`@required` `ebsEncryption` taken out of it.
    let mut without_required = create_body(SUBNET_ID, json!({}));
    without_required
        .as_object_mut()
        .expect("an object body")
        .remove("ebsEncryption");

    // (case, operation, body)
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "recoveryMode outside its enumeration's value set",
            "CreateLaunchConfigurationTemplate",
            json!({"recoveryMode": "BOGUS_MODE"}),
        ),
        (
            "launchDisposition in the wrong case for its value set",
            "CreateLaunchConfigurationTemplate",
            json!({"launchDisposition": "started"}),
        ),
        (
            "exportBucketArn shorter than its length minimum and off its pattern",
            "CreateLaunchConfigurationTemplate",
            json!({"exportBucketArn": TOO_SHORT_ARN}),
        ),
        (
            "pitPolicy one element below its length bound",
            "CreateReplicationConfigurationTemplate",
            create_body(
                SUBNET_ID,
                json!({"pitPolicy": pit_rules(PIT_POLICY_LENGTH - 1)}),
            ),
        ),
        (
            "pitPolicy one element above its length bound",
            "CreateReplicationConfigurationTemplate",
            create_body(
                SUBNET_ID,
                json!({"pitPolicy": pit_rules(PIT_POLICY_LENGTH + 1)}),
            ),
        ),
        (
            "a pitPolicy rule's interval below its range minimum",
            "CreateReplicationConfigurationTemplate",
            create_body(SUBNET_ID, json!({"pitPolicy": rule_below_range})),
        ),
        (
            "a pitPolicy rule missing a member its shape requires",
            "CreateReplicationConfigurationTemplate",
            create_body(SUBNET_ID, json!({"pitPolicy": rule_missing_required})),
        ),
        (
            "replicationServersSecurityGroupsIDs one element above its length maximum",
            "CreateReplicationConfigurationTemplate",
            create_body(
                SUBNET_ID,
                json!({
                    "replicationServersSecurityGroupsIDs": security_groups(SECURITY_GROUPS_MAX + 1)
                }),
            ),
        ),
        (
            "a security group id off its member pattern",
            "CreateReplicationConfigurationTemplate",
            create_body(
                SUBNET_ID,
                json!({"replicationServersSecurityGroupsIDs": ["sg-nope"]}),
            ),
        ),
        (
            "bandwidthThrottling below its range minimum",
            "CreateReplicationConfigurationTemplate",
            create_body(SUBNET_ID, json!({"bandwidthThrottling": -1})),
        ),
        (
            "ebsEncryption outside its enumeration's value set",
            "CreateReplicationConfigurationTemplate",
            create_body(SUBNET_ID, json!({"ebsEncryption": "MAYBE"})),
        ),
        (
            "stagingAreaSubnetId off its pattern",
            "CreateReplicationConfigurationTemplate",
            create_body("vpc-0123456789abcdef0", json!({})),
        ),
        (
            "a member the create's shape requires left out",
            "CreateReplicationConfigurationTemplate",
            without_required,
        ),
        (
            "maxResults below its range minimum on a read",
            "DescribeLaunchConfigurationTemplates",
            json!({"maxResults": DESCRIBE_TEMPLATES_MAX_RESULTS.0 - 1}),
        ),
        (
            "maxResults above its range maximum on a read",
            "DescribeLaunchConfigurationTemplates",
            json!({"maxResults": DESCRIBE_TEMPLATES_MAX_RESULTS.1 + 1}),
        ),
        (
            "an id selection one element above its length maximum",
            "DescribeLaunchConfigurationTemplates",
            json!({"launchConfigurationTemplateIDs": [UNMINTED_TEMPLATE_ID, UNMINTED_TEMPLATE_ID]}),
        ),
        (
            "a required id left out of a delete",
            "DeleteLaunchConfigurationTemplate",
            json!({}),
        ),
        (
            "a template id off its pattern on an operation this emulator has not written",
            "UpdateLaunchConfigurationTemplate",
            json!({"launchConfigurationTemplateID": "lct-short"}),
        ),
        (
            "an enumeration violated on an operation this emulator has not written",
            "UpdateLaunchConfigurationTemplate",
            json!({
                "launchConfigurationTemplateID": UNMINTED_TEMPLATE_ID,
                "launchDisposition": "started",
            }),
        ),
        (
            "a required member left out of an operation this emulator has not written",
            "GetLaunchConfiguration",
            json!({}),
        ),
        (
            "a source server id off its pattern",
            "DescribeRecoverySnapshots",
            json!({"sourceServerID": "s-nope"}),
        ),
        (
            "maxResults below its range minimum on a paginated read",
            "DescribeJobs",
            json!({"maxResults": 0}),
        ),
        (
            "nextToken one character above its length maximum",
            "DescribeJobs",
            json!({"nextToken": long_token}),
        ),
    ];

    let router = drs();
    for (case, operation, body) in &rows {
        let answer = call(&router, operation, body).await;
        assert_body_schema_refusal(&format!("{operation}: {case}"), &answer);
    }

    // No refused row created anything: both stores are still empty.
    assert_eq!(
        stored_counts(&router).await,
        (0, 0),
        "a refused body must create nothing"
    );
}

// ---------------------------------------------------------------------------
// The other side of each bound
// ---------------------------------------------------------------------------

/// A body sitting exactly ON a published bound satisfies the schema and is served, so the
/// refusal above is the constraint's and not a blanket refusal of the member. The rows are
/// the bounds the table above steps one past: the shortest allowed ARN, both ends of the
/// read's `maxResults` range, the longest allowed id selection, the exact `pitPolicy`
/// length, the smallest allowed `bandwidthThrottling` and the longest allowed
/// security-group list.
#[tokio::test]
async fn a_value_on_a_published_bound_is_served() {
    let router = drs();

    // (case, body) for the create that reports the template it stored.
    let creates: Vec<(&str, Value)> = vec![
        (
            "an exportBucketArn of exactly the minimum length",
            json!({"exportBucketArn": SHORTEST_ALLOWED_ARN}),
        ),
        (
            "a tag map, which the create's schema places no bound on",
            json!({"tags": one_tag("owner", "disaster-recovery")}),
        ),
        (
            "every enumerated member at a value its set lists",
            json!({
                "launchDisposition": LAUNCH_DISPOSITION_VALUES[0],
                "recoveryMode": RECOVERY_MODE_VALUES[0],
            }),
        ),
    ];
    let mut created = Vec::new();
    for (case, body) in &creates {
        let answer = call(&router, "CreateLaunchConfigurationTemplate", body).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected the modelled 201, got {answer:?}"
        );
        let id = answer
            .member("launchConfigurationTemplate")
            .and_then(|template| template.get("launchConfigurationTemplateID"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: no template id in {answer:?}"));
        created.push(id.to_string());
    }

    // The replication create at the bounds of three of its own constraints at once.
    let answer = call(
        &router,
        "CreateReplicationConfigurationTemplate",
        &create_body(
            SUBNET_ID,
            json!({
                "pitPolicy": pit_rules(PIT_POLICY_LENGTH),
                "bandwidthThrottling": 0,
                "replicationServersSecurityGroupsIDs": security_groups(SECURITY_GROUPS_MAX),
            }),
        ),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case the replication create at every bound: expected 201, got {answer:?}"
    );

    // Each end of the read's range, and the longest id selection it accepts.
    let reads: Vec<(&str, Value)> = vec![
        (
            "maxResults at its range minimum",
            json!({"maxResults": DESCRIBE_TEMPLATES_MAX_RESULTS.0}),
        ),
        (
            "maxResults at its range maximum",
            json!({"maxResults": DESCRIBE_TEMPLATES_MAX_RESULTS.1}),
        ),
        (
            "an id selection of exactly the maximum length",
            json!({"launchConfigurationTemplateIDs": &created[..DESCRIBE_TEMPLATES_IDS_MAX]}),
        ),
    ];
    for (case, body) in &reads {
        let answer = call(&router, "DescribeLaunchConfigurationTemplates", body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {answer:?}"
        );
        assert!(
            !items(&answer).is_empty(),
            "case {case}: the templates created above must be reported, got {answer:?}"
        );
    }

    // Everything the bounds allowed was stored: three launch templates and one
    // replication template.
    assert_eq!(
        stored_counts(&router).await,
        (creates.len(), 1),
        "every body on a bound must have been created"
    );
}

// ---------------------------------------------------------------------------
// The check is the service's, ahead of every handler
// ---------------------------------------------------------------------------

/// For an operation this emulator has yet to write, an unreadable body is still the
/// service's body-schema refusal, while a body the schema allows reaches the handler and is
/// answered on its handler's own terms — never as this refusal. That is what makes the check
/// the service's rather than any one operation's. The rows are the unwritten operations that
/// declare a constrained input member, each with a body that violates it and one that does
/// not.
#[tokio::test]
async fn an_unwritten_operation_refuses_an_unreadable_body_and_passes_a_readable_one() {
    // (operation, a body outside the schema, a body the schema allows)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "UpdateLaunchConfigurationTemplate",
            json!({"launchConfigurationTemplateID": "lct-short"}),
            json!({
                "launchConfigurationTemplateID": UNMINTED_TEMPLATE_ID,
                "launchDisposition": LAUNCH_DISPOSITION_VALUES[0],
            }),
        ),
        (
            "GetLaunchConfiguration",
            json!({"sourceServerID": "s-nope"}),
            json!({"sourceServerID": UNREGISTERED_SOURCE_SERVER_ID}),
        ),
        (
            "DescribeRecoverySnapshots",
            json!({"sourceServerID": UNREGISTERED_SOURCE_SERVER_ID, "maxResults": 0}),
            json!({"sourceServerID": UNREGISTERED_SOURCE_SERVER_ID, "maxResults": 1}),
        ),
        (
            "DescribeJobs",
            json!({"maxResults": 0}),
            json!({"maxResults": 1}),
        ),
    ];

    let router = drs();
    for (operation, refused, allowed) in &rows {
        let answer = call(&router, operation, refused).await;
        assert_body_schema_refusal(&format!("{operation}: a body outside the schema"), &answer);

        let answer = call(&router, operation, allowed).await;
        assert!(
            !is_body_schema_refusal(&answer),
            "case {operation}: a body the schema allows must reach the handler, \
             got the body-schema refusal {answer:?}"
        );
    }
}
