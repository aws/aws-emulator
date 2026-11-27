// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the logical-destination subscription plumbing:
//! `PutDestination` -> `PutDestinationPolicy` -> `PutSubscriptionFilter`.
//!
//! The rules under test:
//!
//! * `roleArn` is required only for the destination classes CloudWatch Logs delivers to by
//!   assuming the role the request passes — a stream of one of the two streaming services
//!   named as direct destinations. A logical destination created with `PutDestination`
//!   carries the role it delivers through already, and a Lambda function is invoked
//!   directly, so a subscription against either is configured without `roleArn`.
//! * An access policy "authorizes cross-account users to deliver their log events to the
//!   associated destination", so its statements have to identify who is authorized: a
//!   statement that allows the wildcard principal and narrows it with no condition is
//!   refused, while a concrete principal — and the wildcard narrowed by a condition, the
//!   organization-grant form — is accepted and round-trips verbatim.
//! * A log group holds at most five subscription filters AT A TIME: the quota counts the
//!   filters attached, not the puts made, so an update is always accepted and freeing a
//!   slot lets the refused put in. The refusal carries the service's fixed limit text.
//! * A destination lives in the REGION that holds the log groups it is subscribed by, so a
//!   destination ARN naming another Region configures a delivery that could never be made:
//!   it is refused and nothing is stored, for a per-group filter and for the account-level
//!   subscription filter policy alike.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables span what those rules generalise over rather than the one request each was
//! reported against: destination class and owning account, the forms a `Principal` is
//! written in, and every count from an empty log group to one past the quota.

use serde_json::{json, Value};

use crate::account_policies::{describe, member, put_body, subscription_filter_document};
use crate::harness::{Logs, REGION};

/// The operations these suites drive.
const PUT_FILTER: &str = "PutSubscriptionFilter";
const DESCRIBE_FILTERS: &str = "DescribeSubscriptionFilters";
const DELETE_FILTER: &str = "DeleteSubscriptionFilter";
const PUT_DESTINATION: &str = "PutDestination";
const PUT_POLICY: &str = "PutDestinationPolicy";
const DESCRIBE_DESTINATIONS: &str = "DescribeDestinations";
const CREATE_GROUP: &str = "CreateLogGroup";

/// The refusals the operations under test model.
const INVALID_PARAMETER: &str = "InvalidParameterException";
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The fixed text CloudWatch Logs answers a request that would take a log group past the
/// subscription filters it can hold: it names neither the quota nor anything from the
/// request.
const LIMIT_EXCEEDED_MESSAGE: &str = "Resource limit exceeded.";

/// The fixed text CloudWatch Logs answers an `accessPolicy` its IAM policy parser will not
/// take.
const POLICY_PARSE_FAILED: &str = "Error occurred while parsing accessPolicy. Please check \
                                   if the accessPolicy has been constructed correctly using \
                                   IAM grammar.";

/// The subscription filters one log group can hold ATTACHED at a time — the count the
/// service accepts before it refuses the next put.
const ATTACHED_FILTER_QUOTA: usize = 5;

/// Another account than the emulator's own, so a cross-account resource is distinguishable
/// from a same-account one.
const OTHER_ACCOUNT: &str = "210987654321";

/// An ARN in another account, of the form the owning service publishes.
fn other_account_arn(service: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{REGION}:{OTHER_ACCOUNT}:{resource}")
}

/// An ARN in the emulator's OWN account — the account every ARN it mints carries.
fn own_account_arn(service: &str, resource: &str) -> String {
    format!(
        "arn:aws:{service}:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    )
}

/// An IAM role ARN — the `roleArn` form, which carries no region because IAM is global.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{OTHER_ACCOUNT}:role/{name}")
}

/// The same form in the emulator's OWN account: the only account a role can be PASSED to
/// this service from, which is what a logical destination is created with.
fn own_account_role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// One `PutSubscriptionFilter` body; `role` is left out of the request when it is `None`.
fn put_filter_body(group: &str, name: &str, destination: &str, role: Option<&str>) -> Value {
    let mut body = json!({
        "logGroupName": group,
        "filterName": name,
        "filterPattern": "",
        "destinationArn": destination,
    });
    if let Some(role) = role {
        body["roleArn"] = json!(role);
    }
    body
}

/// The subscription filters the service reports for one log group, through a wire
/// `DescribeSubscriptionFilters`.
async fn filters_of(logs: &Logs, group: &str) -> Vec<Value> {
    let body = json!({"logGroupName": group});
    let response = logs.ok(DESCRIBE_FILTERS, &body).await;
    response
        .get("subscriptionFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("{DESCRIBE_FILTERS} {body}: must report subscriptionFilters, got {response}")
        })
        .clone()
}

/// The one filter the service reports under `name` on `group`.
async fn filter_of(logs: &Logs, group: &str, name: &str) -> Value {
    filters_of(logs, group)
        .await
        .into_iter()
        .find(|filter| filter.get("filterName") == Some(&json!(name)))
        .unwrap_or_else(|| panic!("case {group}/{name}: the filter must be reported"))
}

/// The names the service reports for one log group's filters.
async fn filter_names(logs: &Logs, group: &str) -> Vec<String> {
    filters_of(logs, group)
        .await
        .iter()
        .filter_map(|filter| {
            filter
                .get("filterName")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

/// A log group, created.
async fn create_group(logs: &Logs, group: &str) {
    logs.ok(CREATE_GROUP, &json!({"logGroupName": group})).await;
}

/// The `accessPolicy` the service reports for one destination, or `None` when it holds
/// none.
async fn reported_policy(logs: &Logs, name: &str) -> Option<String> {
    let body = json!({"DestinationNamePrefix": name});
    let response = logs.ok(DESCRIBE_DESTINATIONS, &body).await;
    response
        .get("destinations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("{DESCRIBE_DESTINATIONS} {body}: must report destinations, got {response}")
        })
        .iter()
        .find(|destination| destination.get("destinationName") == Some(&json!(name)))
        .unwrap_or_else(|| panic!("case {name}: the destination must be reported"))
        .get("accessPolicy")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// One access policy statement, as a client writes it: the `Principal` form under test,
/// and the condition (if any) that narrows it.
fn statement(principal: Value, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Sid": "AllowCrossAccountSubscription",
        "Effect": "Allow",
        "Action": "logs:PutSubscriptionFilter",
        "Resource": own_account_arn("logs", "destination:policy-dest"),
    });
    if !principal.is_null() {
        statement["Principal"] = principal;
    }
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// One statement wrapped in the policy document that carries it.
fn policy(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// The documented cross-account flow, end to end: a logical destination created with
/// `PutDestination` (which is where its role is supplied), an access policy attached to it,
/// and a subscription filter pointed at it with NO `roleArn` — accepted, and readable back
/// through `DescribeSubscriptionFilters` with the destination it names and no role.
#[tokio::test]
async fn the_documented_cross_account_flow_is_accepted() {
    let logs = Logs::new();
    let group = "cross-account-group";
    create_group(&logs, group).await;

    let destination_name = "log-destination";
    let created = logs
        .ok(
            PUT_DESTINATION,
            &json!({
                "destinationName": destination_name,
                "targetArn": other_account_arn("kinesis", "stream/log-events"),
                "roleArn": own_account_role_arn("CWLtoKinesisRole"),
            }),
        )
        .await;
    let destination_arn = created
        .get("destination")
        .and_then(|destination| destination.get("arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{PUT_DESTINATION} must report the destination ARN, got {created}"))
        .to_string();

    let access_policy = policy(json!([statement(
        json!({"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")}),
        None,
    )]));
    logs.ok(
        PUT_POLICY,
        &json!({"destinationName": destination_name, "accessPolicy": access_policy}),
    )
    .await;

    logs.ok(
        PUT_FILTER,
        &put_filter_body(group, "to-logical-destination", &destination_arn, None),
    )
    .await;

    let found = filter_of(&logs, group, "to-logical-destination").await;
    assert_eq!(
        found.get("destinationArn").and_then(Value::as_str),
        Some(destination_arn.as_str()),
        "case the documented flow: the filter reports the destination it was pointed at, got {found}"
    );
    assert_eq!(
        found.get("roleArn"),
        None,
        "case the documented flow: no role was configured, got {found}"
    );
    let policy_back = reported_policy(&logs, destination_name).await;
    assert_eq!(
        policy_back.as_deref(),
        Some(access_policy.as_str()),
        "case the documented flow: the destination reports the attached policy, got {policy_back:?}"
    );
}

/// `roleArn` is required for exactly the destination classes CloudWatch Logs delivers to by
/// assuming it. The table spans every destination class the operation documents, in this
/// account and in another, and every class is also put WITH a role to show the member stays
/// optional where it is not required.
#[tokio::test]
async fn a_role_is_required_only_where_the_delivery_assumes_one() {
    let logs = Logs::new();
    let group = "role-rule-group";
    create_group(&logs, group).await;
    let name = "one-filter";
    let role = role_arn("CWLtoTargetRole");

    // (case, destinationArn, whether a put without roleArn is refused).
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a Kinesis stream in another account",
            other_account_arn("kinesis", "stream/log-events"),
            true,
        ),
        (
            "a Kinesis stream in this account",
            own_account_arn("kinesis", "stream/log-events"),
            true,
        ),
        (
            "a Firehose delivery stream in another account",
            other_account_arn("firehose", "deliverystream/log-events"),
            true,
        ),
        (
            "a Firehose delivery stream in this account",
            own_account_arn("firehose", "deliverystream/log-events"),
            true,
        ),
        (
            "a Lambda function in this account",
            own_account_arn("lambda", "function:log-consumer"),
            false,
        ),
        (
            "a logical destination in this account",
            own_account_arn("logs", "destination:same-account-destination"),
            false,
        ),
        (
            "a logical destination in another account",
            other_account_arn("logs", "destination:cross-account-destination"),
            false,
        ),
        (
            "a logical destination whose name carries separators",
            other_account_arn("logs", "destination:team/one.dest-2_final"),
            false,
        ),
    ];

    for (case, destination, needs_role) in &rows {
        let body = put_filter_body(group, name, destination, None);
        if *needs_role {
            let (code, message) = logs.refusal(PUT_FILTER, &body).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: a delivery that assumes a role is refused without one, \
                 got {code} {message}"
            );
            assert!(
                message.contains("roleArn"),
                "case {case:?}: the refusal names the member to set, got {message:?}"
            );
            let stored = filter_names(&logs, group).await;
            assert_eq!(
                stored,
                Vec::<String>::new(),
                "case {case:?}: a refused put stores nothing, got {stored:?}"
            );
        } else {
            logs.ok(PUT_FILTER, &body).await;
            let found = filter_of(&logs, group, name).await;
            assert_eq!(
                (
                    found.get("destinationArn").and_then(Value::as_str),
                    found.get("roleArn"),
                ),
                (Some(destination.as_str()), None),
                "case {case:?}: the filter is configured with no role, got {found}"
            );
            logs.ok(
                DELETE_FILTER,
                &json!({"logGroupName": group, "filterName": name}),
            )
            .await;
        }

        // Whether or not the class requires one, a put that DOES pass a role is accepted
        // and reports it back.
        logs.ok(
            PUT_FILTER,
            &put_filter_body(group, name, destination, Some(&role)),
        )
        .await;
        let found = filter_of(&logs, group, name).await;
        assert_eq!(
            found.get("roleArn").and_then(Value::as_str),
            Some(role.as_str()),
            "case {case:?}: the role the put named is reported, got {found}"
        );
        logs.ok(
            DELETE_FILTER,
            &json!({"logGroupName": group, "filterName": name}),
        )
        .await;
    }
}

/// An access policy has to identify the principals it authorizes, and they have to be the
/// ACCOUNTS whose claims it grants: a statement allowing the wildcard principal with no
/// condition to narrow it is refused with the service's fixed parse sentence and attaches
/// nothing — as are a document that is no IAM policy at all and a grant to a service
/// principal — while a concrete account, a wildcard narrowed by a condition, and a wildcard
/// under `Deny` are accepted and round-trip verbatim.
#[tokio::test]
async fn an_access_policy_that_authorizes_every_principal_is_refused() {
    let logs = Logs::new();
    let destination_name = "policy-dest";
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "destinationName": destination_name,
            "targetArn": other_account_arn("kinesis", "stream/log-events"),
            "roleArn": own_account_role_arn("CWLtoKinesisRole"),
        }),
    )
    .await;

    // A policy the destination already holds, so a refused put can be shown to leave the
    // attached policy in place.
    let held = policy(json!([statement(
        json!({"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")}),
        None,
    )]));
    logs.ok(
        PUT_POLICY,
        &json!({"destinationName": destination_name, "accessPolicy": held}),
    )
    .await;

    // Every form a wildcard `Principal` is written in, unnarrowed: the bare string, the map
    // from principal type to the wildcard, the wildcard inside a list, a service principal,
    // a statement carrying an empty condition block, and a document whose SECOND statement
    // is the wildcard one.
    let refused: Vec<(&str, String)> = vec![
        ("the bare wildcard principal", policy(json!([statement(json!("*"), None)]))),
        (
            "the wildcard under the AWS principal type",
            policy(json!([statement(json!({"AWS": "*"}), None)])),
        ),
        (
            "the wildcard inside a principal list",
            policy(json!([statement(
                json!({"AWS": [format!("arn:aws:iam::{OTHER_ACCOUNT}:root"), "*"]}),
                None,
            )])),
        ),
        (
            "the wildcard under the Service principal type",
            policy(json!([statement(json!({"Service": "*"}), None)])),
        ),
        (
            "the wildcard with an empty condition block",
            policy(json!([statement(json!("*"), Some(json!({})))])),
        ),
        (
            "one narrowed statement and one wildcard statement",
            policy(json!([
                statement(
                    json!({"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")}),
                    None,
                ),
                statement(json!("*"), None),
            ])),
        ),
        (
            "a single statement object rather than a list",
            policy(statement(json!("*"), None)),
        ),
        // The same sentence refuses a document that is not an IAM policy at all: one whose
        // statement identifies no principal to authorize, and one carrying no statement. And
        // one whose grant is not a grant to an ACCOUNT: what this policy authorizes is
        // another account's claim on the destination, so a SERVICE principal is a permission
        // of that service rather than this document's grant.
        (
            "a statement naming no principal at all",
            policy(json!([statement(Value::Null, None)])),
        ),
        ("a document carrying no statements", "{}".to_string()),
        (
            "a grant to a service principal",
            policy(json!([statement(
                json!({"Service": "logs.amazonaws.com"}),
                None,
            )])),
        ),
    ];

    for (case, document) in &refused {
        let (code, message) = logs
            .refusal(
                PUT_POLICY,
                &json!({"destinationName": destination_name, "accessPolicy": document}),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: a policy the IAM parser will not take is refused, \
             got {code} {message}"
        );
        assert_eq!(
            message, POLICY_PARSE_FAILED,
            "case {case:?}: expected {POLICY_PARSE_FAILED:?}, got {message:?}"
        );
        let reported = reported_policy(&logs, destination_name).await;
        assert_eq!(
            reported.as_deref(),
            Some(held.as_str()),
            "case {case:?}: a refused put leaves the attached policy in place, got {reported:?}"
        );
    }

    // The same statements with the principals identified, which the service accepts and
    // reports back verbatim.
    let accepted: Vec<(&str, String)> = vec![
        (
            "one account root",
            policy(json!([statement(
                json!({"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")}),
                None,
            )])),
        ),
        (
            "an account id on its own",
            policy(json!([statement(json!({"AWS": OTHER_ACCOUNT}), None)])),
        ),
        (
            "two accounts",
            policy(json!([statement(
                json!({"AWS": [
                    format!("arn:aws:iam::{OTHER_ACCOUNT}:root"),
                    format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
                ]}),
                None,
            )])),
        ),
        (
            "one named role",
            policy(json!([statement(
                json!({"AWS": role_arn("LogSender")}),
                None,
            )])),
        ),
        (
            "the wildcard narrowed to an organization",
            policy(json!([statement(
                json!({"AWS": "*"}),
                Some(json!({"StringEquals": {"aws:PrincipalOrgID": "o-1234567890"}})),
            )])),
        ),
        (
            "the bare wildcard narrowed to an organization",
            policy(json!([statement(
                json!("*"),
                Some(json!({"StringEquals": {"aws:PrincipalOrgID": "o-1234567890"}})),
            )])),
        ),
        (
            "a Deny over the wildcard",
            policy(json!([{
                "Sid": "DenyEveryoneElse",
                "Effect": "Deny",
                "Principal": "*",
                "Action": "logs:PutSubscriptionFilter",
                "Resource": "*",
            }])),
        ),
    ];

    for (case, document) in &accepted {
        // These rows walk between grant kinds, so the put that replaces an account grant
        // with an organization grant carries `forceUpdate` — the affirmation that one
        // update needs, which every other put is unaffected by.
        logs.ok(
            PUT_POLICY,
            &json!({
                "destinationName": destination_name,
                "accessPolicy": document,
                "forceUpdate": true,
            }),
        )
        .await;
        let reported = reported_policy(&logs, destination_name).await;
        assert_eq!(
            reported.as_deref(),
            Some(document.as_str()),
            "case {case:?}: the attached policy round-trips verbatim, got {reported:?}"
        );
    }
}

/// The per-group quota counts the filters a log group HOLDS: every count up to the quota is
/// accepted, the one past it is refused with the fixed limit sentence, an update at the
/// quota is accepted because it adds nothing, and deleting a filter frees its slot for the
/// put that was just refused.
#[tokio::test]
async fn the_quota_counts_attached_filters_rather_than_puts() {
    let logs = Logs::new();
    let role = role_arn("CWLtoTargetRole");

    // One destination per filter, spanning the classes a group can fan out to, so the
    // quota is shown to count filters rather than distinct destinations.
    let destinations: Vec<(String, Option<&str>)> = vec![
        (
            other_account_arn("kinesis", "stream/log-events"),
            Some(role.as_str()),
        ),
        (
            other_account_arn("firehose", "deliverystream/log-events"),
            Some(role.as_str()),
        ),
        (own_account_arn("lambda", "function:log-consumer"), None),
        (other_account_arn("logs", "destination:cross-account"), None),
        (
            own_account_arn("kinesis", "stream/same-account"),
            Some(role.as_str()),
        ),
    ];
    assert_eq!(
        destinations.len(),
        ATTACHED_FILTER_QUOTA,
        "the table attaches exactly the quota's worth of filters, got {}",
        destinations.len()
    );

    // The quota is per log group, so each group takes its own.
    for group in ["fan-out-group", "fan-out-group-other"] {
        create_group(&logs, group).await;
        for (index, (destination, role)) in destinations.iter().enumerate() {
            let name = format!("filter-{index}");
            logs.ok(PUT_FILTER, &put_filter_body(group, &name, destination, *role))
                .await;
            let held = filter_names(&logs, group).await;
            assert_eq!(
                held.len(),
                index + 1,
                "case {group} filter {}: the group holds every filter attached so far, got {held:?}",
                index + 1
            );
        }

        // One past the quota: refused with the fixed sentence, and nothing stored.
        let over = put_filter_body(
            group,
            "one-too-many",
            &destinations[0].0,
            destinations[0].1,
        );
        let (code, message) = logs.refusal(PUT_FILTER, &over).await;
        assert_eq!(
            code, LIMIT_EXCEEDED,
            "case {group}: a put past {ATTACHED_FILTER_QUOTA} attached filters is refused, \
             got {code} {message}"
        );
        assert_eq!(
            message, LIMIT_EXCEEDED_MESSAGE,
            "case {group}: expected {LIMIT_EXCEEDED_MESSAGE:?}, got {message:?}"
        );
        assert_eq!(
            filter_names(&logs, group).await.len(),
            ATTACHED_FILTER_QUOTA,
            "case {group}: a refused put attaches nothing"
        );

        // At the quota, a put under a name the group already holds is an UPDATE, so it is
        // accepted and the count does not move.
        let mut update = put_filter_body(group, "filter-0", &destinations[0].0, destinations[0].1);
        update["filterPattern"] = json!("ERROR");
        logs.ok(PUT_FILTER, &update).await;
        let updated = filter_of(&logs, group, "filter-0").await;
        assert_eq!(
            updated.get("filterPattern").and_then(Value::as_str),
            Some("ERROR"),
            "case {group}: the update is stored, got {updated}"
        );
        assert_eq!(
            filter_names(&logs, group).await.len(),
            ATTACHED_FILTER_QUOTA,
            "case {group}: an update at the quota adds no filter"
        );

        // Freeing one slot lets the previously refused put in: the limit counts attached
        // filters, not calls.
        logs.ok(
            DELETE_FILTER,
            &json!({"logGroupName": group, "filterName": "filter-1"}),
        )
        .await;
        logs.ok(PUT_FILTER, &over).await;
        let held = filter_names(&logs, group).await;
        assert_eq!(
            held,
            vec![
                "filter-0".to_string(),
                "filter-2".to_string(),
                "filter-3".to_string(),
                "filter-4".to_string(),
                "one-too-many".to_string(),
            ],
            "case {group}: the freed slot took the refused filter, got {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// The REGION a subscription delivery's destination has to be in.
// ---------------------------------------------------------------------------------------

/// The account-level policy type whose document configures a subscription delivery, and the
/// operation that puts one. Source: the model's `PolicyType` enum.
const SUBSCRIPTION_FILTER_POLICY: &str = "SUBSCRIPTION_FILTER_POLICY";
const PUT_ACCOUNT_POLICY: &str = "PutAccountPolicy";
const DELETE_ACCOUNT_POLICY: &str = "DeleteAccountPolicy";

/// Regions that are NOT the one a case makes its request in, spanning the forms a wrong
/// Region comes in: another continent, a neighbouring Region, another partition, a Region
/// name that is a PREFIX of the request's, and the request's own Region in another case.
const OTHER_REGIONS: [&str; 5] = [
    "us-east-1",
    "eu-west-1",
    "us-gov-west-1",
    "eu-west",
    "EU-WEST-2",
];

/// The destination classes `PutSubscriptionFilter` delivers to, each as the `(vendor,
/// resource)` segments of its ARN and whether CloudWatch Logs reaches it by assuming the
/// `roleArn` the put names. Source: `PutSubscriptionFilter$destinationArn` — "An Amazon
/// Kinesis stream", "A logical destination (specified using an ARN)", "A Kinesis Data
/// Firehose delivery stream", "A Lambda function"; the role is required for the two
/// streaming classes, as the suite above pins.
const DESTINATION_CLASSES: [(&str, &str, &str, bool); 4] = [
    ("a Kinesis stream", "kinesis", "stream/log-events", true),
    (
        "a Firehose delivery stream",
        "firehose",
        "deliverystream/log-events",
        true,
    ),
    (
        "a Lambda function",
        "lambda",
        "function:log-consumer",
        false,
    ),
    (
        "a logical destination",
        "logs",
        "destination:cross-account",
        false,
    ),
];

/// One destination ARN in `region`, in the account another account's resources carry, so a
/// case varies only the Region.
fn arn_in_region(region: &str, service: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{OTHER_ACCOUNT}:{resource}")
}

/// The refusal sentence the service states for a destination outside the Region a request
/// was made in. FIXED text for the error class: it names neither the member nor either
/// Region, so every row below expects exactly this.
const CROSS_REGION_DESTINATION_MESSAGE: &str =
    "Logical destination should be in the same region as caller region";

/// The subscription filters the service reports for one log group, read in `region`.
async fn filters_in(logs: &Logs, region: &str, group: &str) -> Vec<Value> {
    let body = json!({"logGroupName": group});
    let response = logs.ok_in_region(region, DESCRIBE_FILTERS, &body).await;
    response
        .get("subscriptionFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("{DESCRIBE_FILTERS} {body}: must report subscriptionFilters, got {response}")
        })
        .clone()
}

/// The `destinationArn` the service reports for the filter `name` of `group`, read in
/// `region`.
async fn reported_destination(logs: &Logs, region: &str, group: &str, name: &str) -> String {
    filters_in(logs, region, group)
        .await
        .into_iter()
        .find(|filter| filter.get("filterName") == Some(&json!(name)))
        .unwrap_or_else(|| panic!("case {group}/{name}: the filter must be reported"))
        .get("destinationArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {group}/{name}: the filter carries a destinationArn"))
        .to_string()
}

/// Every destination class, in every Region that is not the log group's, is refused with the
/// FIXED sentence the error class carries (it names neither the member nor either Region),
/// and stores nothing — while the same destination in the log group's own Region is accepted
/// and read back.
#[tokio::test]
async fn a_destination_in_another_region_is_refused() {
    let logs = Logs::new();
    let group = "region-rule-group";
    create_group(&logs, group).await;
    let name = "to-destination";
    let role = role_arn("CWLtoTargetRole");

    for (case, vendor, resource, needs_role) in DESTINATION_CLASSES {
        let role = needs_role.then_some(role.as_str());
        for other in OTHER_REGIONS {
            let destination = arn_in_region(other, vendor, resource);
            let body = put_filter_body(group, name, &destination, role);
            let (code, message) = logs.refusal(PUT_FILTER, &body).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?} in {other}: a destination outside the log group's region is \
                 refused, got {code} {message}"
            );
            assert_eq!(
                message,
                CROSS_REGION_DESTINATION_MESSAGE,
                "case {case:?} in {other}: expected \
                 {CROSS_REGION_DESTINATION_MESSAGE:?}, got {message:?}"
            );
            let stored = filter_names(&logs, group).await;
            assert_eq!(
                stored,
                Vec::<String>::new(),
                "case {case:?} in {other}: a refused put stores nothing, got {stored:?}"
            );
        }

        // The same destination class in the log group's own Region is accepted, so what the
        // refusals above turn on is the Region and nothing else.
        let destination = arn_in_region(REGION, vendor, resource);
        logs.ok(PUT_FILTER, &put_filter_body(group, name, &destination, role))
            .await;
        let reported = reported_destination(&logs, REGION, group, name).await;
        assert_eq!(
            reported, destination,
            "case {case:?} in {REGION}: the filter reports the destination it was pointed at, \
             got {reported:?}"
        );
        logs.ok(
            DELETE_FILTER,
            &json!({"logGroupName": group, "filterName": name}),
        )
        .await;
    }
}

/// The Region the destination must be in is the Region the REQUEST was made in — the one
/// holding the log group — rather than any fixed Region: a put made in each Region takes the
/// destination of that Region and refuses the destinations of every other, naming the Region
/// it was made in.
#[tokio::test]
async fn the_region_a_destination_must_be_in_is_the_requests() {
    let logs = Logs::new();
    let name = "to-destination";

    for group_region in [REGION, "us-east-1", "ap-southeast-2"] {
        let group = format!("group-in-{group_region}");
        logs.ok_in_region(group_region, CREATE_GROUP, &json!({"logGroupName": group}))
            .await;

        // The destination in the Region the request was made in: accepted, and reported back
        // by a read made in that Region.
        let destination = arn_in_region(group_region, "lambda", "function:log-consumer");
        logs.ok_in_region(
            group_region,
            PUT_FILTER,
            &put_filter_body(&group, name, &destination, None),
        )
        .await;
        let reported = reported_destination(&logs, group_region, &group, name).await;
        assert_eq!(
            reported, destination,
            "case a request in {group_region}: the destination of that region is configured, \
             got {reported:?}"
        );

        // Every other Region's destination is refused for this request, the Region the rest
        // of this suite makes its requests in included.
        for other in OTHER_REGIONS
            .iter()
            .copied()
            .chain([REGION])
            .filter(|region| *region != group_region)
        {
            let elsewhere = arn_in_region(other, "lambda", "function:log-consumer");
            let (code, message) = logs
                .refusal_in_region(
                    group_region,
                    PUT_FILTER,
                    &put_filter_body(&group, name, &elsewhere, None),
                )
                .await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case a request in {group_region} for {other}: the put is refused, \
                 got {code} {message}"
            );
            assert_eq!(
                message,
                CROSS_REGION_DESTINATION_MESSAGE,
                "case a request in {group_region} for {other}: expected \
                 {CROSS_REGION_DESTINATION_MESSAGE:?}, got {message:?}"
            );
            // The filter configured above is untouched by the refused put.
            let reported = reported_destination(&logs, group_region, &group, name).await;
            assert_eq!(
                reported, destination,
                "case a request in {group_region} for {other}: a refused put changes no stored \
                 filter, got {reported:?}"
            );
        }
    }
}

/// An account-level subscription filter policy is held to the same rule: its
/// `DestinationArn` must be in the Region the policy is put in, because that is the Region
/// whose log groups it applies to. A document naming another Region is refused and stores no
/// policy; the same document with the destination in the policy's own Region is accepted and
/// read back.
#[tokio::test]
async fn an_account_policys_destination_must_be_in_its_region() {
    let logs = Logs::new();
    let policy_name = "account-wide-subscription";
    let role = role_arn("CWLtoTargetRole");

    for (case, vendor, resource, needs_role) in DESTINATION_CLASSES {
        let role = needs_role.then_some(role.as_str());
        for other in OTHER_REGIONS {
            let document =
                subscription_filter_document(&arn_in_region(other, vendor, resource), role);
            let (code, message) = logs
                .refusal(
                    PUT_ACCOUNT_POLICY,
                    &put_body(policy_name, SUBSCRIPTION_FILTER_POLICY, &document, None),
                )
                .await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?} in {other}: an account policy delivering outside its region is \
                 refused, got {code} {message}"
            );
            assert_eq!(
                message,
                CROSS_REGION_DESTINATION_MESSAGE,
                "case {case:?} in {other}: expected \
                 {CROSS_REGION_DESTINATION_MESSAGE:?}, got {message:?}"
            );
            let stored = describe(&logs, &json!({"policyType": SUBSCRIPTION_FILTER_POLICY})).await;
            assert_eq!(
                stored,
                Vec::<Value>::new(),
                "case {case:?} in {other}: a refused put stores no policy, got {stored:?}"
            );
        }

        // The same document with the destination in the policy's own Region is accepted.
        let document = subscription_filter_document(&arn_in_region(REGION, vendor, resource), role);
        logs.ok(
            PUT_ACCOUNT_POLICY,
            &put_body(policy_name, SUBSCRIPTION_FILTER_POLICY, &document, None),
        )
        .await;
        let documents: Vec<String> =
            describe(&logs, &json!({"policyType": SUBSCRIPTION_FILTER_POLICY}))
                .await
                .iter()
                .map(|policy| member(policy, "policyDocument"))
                .collect();
        assert_eq!(
            documents,
            vec![document],
            "case {case:?} in {REGION}: the accepted document is the one stored, got {documents:?}"
        );
        logs.ok(
            DELETE_ACCOUNT_POLICY,
            &json!({"policyName": policy_name, "policyType": SUBSCRIPTION_FILTER_POLICY}),
        )
        .await;
    }
}
