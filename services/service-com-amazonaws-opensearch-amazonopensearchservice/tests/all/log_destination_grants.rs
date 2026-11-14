// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what makes a domain's logs REACH the CloudWatch Logs group its
//! `LogPublishingOptions` name, over the real wire path: a `restJson1` HTTP request through
//! the multi-service router into the mounted service, so what is asserted is what a client
//! sees. The log groups, the resource policies on them and the records in them are created
//! and read through the Amazon CloudWatch Logs service's OWN `CreateLogGroup`,
//! `PutResourcePolicy` and `FilterLogEvents`, mounted on the same shared engine
//! ([`Emulator::with_log_groups`]).
//!
//! Three rules, each asserted over a SET of inputs rather than one call:
//!
//! * the PERMISSION the destination has to carry: an option that ENABLES publishing is
//!   accepted only when the CloudWatch Logs resource policies in force for the log group it
//!   names permit the OpenSearch service principal to create a log stream in that group and
//!   to put log events into it; a group whose policies withhold either action is refused
//!   `ValidationException` (HTTP 400), the refusal naming that group, and neither a domain
//!   nor a change to one is stored. A DISABLED option delivers nothing, so no permission is
//!   required of the group it names. The rows differ along whether the group carries a policy
//!   at all, which principal it grants to, which of the two actions it grants, which resource
//!   the grant names (the group's own ARN, the wildcard form that names its streams, a
//!   pattern covering it, another group's ARN), whether the action is named by a wildcard,
//!   whether an explicit deny stands beside the grant, and whether the policy is attached to
//!   the group or held by the account — each row driven through BOTH operations that take the
//!   map;
//! * the NODE a record comes from: a cluster serves its indexes from at least one data node
//!   whatever its `ClusterConfig` states, so a domain created without an explicit instance
//!   count publishes its slow logs like any other;
//! * the FORM a threshold is written in: a slow-log threshold names the same index setting
//!   whether it is written flattened or expanded and whether the index create or the
//!   index-settings update wrote it, so the search it makes slow is recorded either way.

use serde_json::{json, Value};

use super::domain_configuration_rules::{assert_no_domain, assert_refusal};
use super::domain_lifecycle::{
    baseline_create, create_baseline, log_group_arn, settled_domain, Emulator,
    DEFAULT_ACCESS_KEY_ID, REGION,
};
use super::domain_rest_data_plane::{endpoint_of, rest};
use super::log_publishing_delivery::events;
use super::service_software_update::message;

/// The error every refusal below carries: the operation's modelled 400 for a configuration
/// the service will not provision.
const REFUSAL: &str = "ValidationException";

/// The log type every row publishes: the rule is about the DESTINATION, which every log type
/// of the map resolves the same way (`log_publishing_destinations` drives the whole
/// `LogType` catalog against it).
const LOG_TYPE: &str = "INDEX_SLOW_LOGS";

/// The service principal the destination's policy has to grant: Amazon OpenSearch Service
/// delivers a domain's logs itself, under its own principal.
const SERVICE_PRINCIPAL: &str = "es.amazonaws.com";

/// Another service's principal, so a row can show that a policy granting SOMEONE ELSE both
/// actions grants this service nothing.
const OTHER_PRINCIPAL: &str = "route53.amazonaws.com";

/// A group no row publishes to, named only by the grant of the row whose policy points at
/// the wrong group.
const OTHER_GROUP: &str = "/aws/opensearch/grants/elsewhere";

/// How a resource policy document is built for the group under test: the row is handed that
/// group's ARN, because a grant names the resource it is a grant on.
type Document = Box<dyn Fn(&str) -> Value>;

/// Where the policy a row attaches lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Attached to the destination group itself, which is how `PutResourcePolicy` scopes a
    /// policy to one resource (`resourceArn`).
    Group,
    /// Held by the ACCOUNT, covering the log groups its own statements name — the scope a
    /// `PutResourcePolicy` carrying no `resourceArn` stores.
    Account,
}

/// One row: the policy the destination group carries, where that policy lives, and whether
/// the rule lets a domain publish to the group with it in force.
struct Row {
    case: &'static str,
    /// `None` for a group that carries no resource policy at all.
    policy: Option<Document>,
    scope: Scope,
    granted: bool,
}

/// A policy document with one statement, as a caller writes one.
fn statement(effect: &str, principal: Value, actions: Value, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": principal,
            "Action": actions,
            "Resource": resource,
        }],
    })
}

/// The `Principal` element naming one service principal.
fn service(principal: &str) -> Value {
    json!({ "Service": principal })
}

/// The ARN form that names a log group's STREAMS (`LogGroup$arn`): the group's ARN with the
/// trailing `:*`, which is the resource a grant on a group's streams is written on.
fn streams_of(arn: &str) -> String {
    format!("{arn}:*")
}

/// The table: every row is the whole permission state of the group one request publishes to.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no resource policy at all",
            policy: None,
            scope: Scope::Group,
            granted: false,
        },
        Row {
            case: "a document with no statement",
            policy: Some(Box::new(|_| json!({ "Version": "2012-10-17", "Statement": [] }))),
            scope: Scope::Group,
            granted: false,
        },
        Row {
            case: "both actions granted to another service principal",
            policy: Some(Box::new(|arn| {
                statement(
                    "Allow",
                    service(OTHER_PRINCIPAL),
                    json!(["logs:CreateLogStream", "logs:PutLogEvents"]),
                    json!(streams_of(arn)),
                )
            })),
            scope: Scope::Group,
            granted: false,
        },
        Row {
            case: "only logs:PutLogEvents granted",
            policy: Some(Box::new(|arn| {
                statement(
                    "Allow",
                    service(SERVICE_PRINCIPAL),
                    json!("logs:PutLogEvents"),
                    json!(streams_of(arn)),
                )
            })),
            scope: Scope::Group,
            granted: false,
        },
        Row {
            case: "only logs:CreateLogStream granted",
            policy: Some(Box::new(|arn| {
                statement(
                    "Allow",
                    service(SERVICE_PRINCIPAL),
                    json!("logs:CreateLogStream"),
                    json!(streams_of(arn)),
                )
            })),
            scope: Scope::Group,
            granted: false,
        },
        Row {
            case: "both actions on the group's streams",
            policy: Some(Box::new(|arn| {
                statement(
                    "Allow",
                    service(SERVICE_PRINCIPAL),
                    json!(["logs:CreateLogStream", "logs:PutLogEvents"]),
                    json!(streams_of(arn)),
                )
            })),
            scope: Scope::Group,
            granted: true,
        },
        Row {
            case: "both actions on the group's own ARN",
            policy: Some(Box::new(|arn| {
                statement(
                    "Allow",
                    service(SERVICE_PRINCIPAL),
                    json!(["logs:CreateLogStream", "logs:PutLogEvents"]),
                    json!(arn),
                )
            })),
            scope: Scope::Group,
            granted: true,
        },
        Row {
            case: "both actions under a resource pattern covering the group",
            policy: Some(Box::new(|_| {
                statement(
                    "Allow",
                    service(SERVICE_PRINCIPAL),
                    json!(["logs:CreateLogStream", "logs:PutLogEvents"]),
                    json!(format!(
                        "arn:aws:logs:{REGION}:{}:log-group:/aws/opensearch/*",
                        ::engine::emulator_account_id()
                    )),
                )
            })),
            scope: Scope::Group,
            granted: true,
        },
        Row {
            case: "the logs action namespace by wildcard",
            policy: Some(Box::new(|arn| {
                statement(
                    "Allow",
                    service(SERVICE_PRINCIPAL),
                    json!("logs:*"),
                    json!(streams_of(arn)),
                )
            })),
            scope: Scope::Group,
            granted: true,
        },
        Row {
            case: "every principal",
            policy: Some(Box::new(|arn| {
                statement(
                    "Allow",
                    json!("*"),
                    json!(["logs:CreateLogStream", "logs:PutLogEvents"]),
                    json!(streams_of(arn)),
                )
            })),
            scope: Scope::Group,
            granted: true,
        },
        Row {
            case: "both actions granted and one of them denied",
            policy: Some(Box::new(|arn| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": service(SERVICE_PRINCIPAL),
                            "Action": ["logs:CreateLogStream", "logs:PutLogEvents"],
                            "Resource": streams_of(arn),
                        },
                        {
                            "Effect": "Deny",
                            "Principal": service(SERVICE_PRINCIPAL),
                            "Action": "logs:PutLogEvents",
                            "Resource": streams_of(arn),
                        },
                    ],
                })
            })),
            scope: Scope::Group,
            granted: false,
        },
        Row {
            case: "both actions on another group",
            policy: Some(Box::new(|_| {
                statement(
                    "Allow",
                    service(SERVICE_PRINCIPAL),
                    json!(["logs:CreateLogStream", "logs:PutLogEvents"]),
                    json!(streams_of(&log_group_arn(OTHER_GROUP))),
                )
            })),
            scope: Scope::Group,
            granted: false,
        },
        Row {
            case: "an account-held policy covering every group",
            policy: Some(Box::new(|_| {
                statement(
                    "Allow",
                    service(SERVICE_PRINCIPAL),
                    json!(["logs:CreateLogStream", "logs:PutLogEvents"]),
                    json!("*"),
                )
            })),
            scope: Scope::Account,
            granted: true,
        },
        Row {
            case: "an account-held policy granting another principal",
            policy: Some(Box::new(|_| {
                statement(
                    "Allow",
                    service(OTHER_PRINCIPAL),
                    json!(["logs:CreateLogStream", "logs:PutLogEvents"]),
                    json!("*"),
                )
            })),
            scope: Scope::Account,
            granted: false,
        },
    ]
}

/// An enabled option publishing [`LOG_TYPE`] to the group `name`.
fn publishing_to(name: &str) -> Value {
    json!({
        LOG_TYPE: { "Enabled": true, "CloudWatchLogsLogGroupArn": log_group_arn(name) },
    })
}

/// The sentence the service refuses a destination it may not write to with: fixed text whose
/// one variable part is the name of the group whose policy withholds the delivery.
fn ungranted(name: &str) -> String {
    format!(
        "The Resource Access Policy specified for the CloudWatch Logs log group {name} does \
         not grant sufficient permissions for Amazon OpenSearch Service to create a log \
         stream. Please check the Resource Access Policy."
    )
}

/// Create the group a row publishes to and put the row's policy in force, through the
/// CloudWatch Logs service's own operations.
async fn with_destination(emulator: &Emulator, row: &Row, name: &str) {
    emulator.created_log_group_without_grant(name).await;
    let Some(document) = row.policy.as_ref() else {
        return;
    };
    let arn = log_group_arn(name);
    let mut request = json!({
        "policyName": format!("{name}-policy"),
        "policyDocument": document(&arn).to_string(),
    });
    if row.scope == Scope::Group {
        request["resourceArn"] = json!(arn);
    }
    let answer = emulator.logs_call("PutResourcePolicy", request).await;
    assert_eq!(
        answer.status, 200,
        "case {}: expected PutResourcePolicy to be accepted, got {} with {:?}",
        row.case, answer.status, answer.body
    );
}

/// What `DescribeDomain` reports `name` publishing — absent when it publishes nothing.
async fn published(emulator: &Emulator, name: &str) -> Option<Value> {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]
        .get("LogPublishingOptions")
        .cloned()
}

#[tokio::test]
async fn a_create_publishes_only_to_a_group_whose_policy_admits_the_service() {
    for (index, row) in rows().into_iter().enumerate() {
        // One emulator per row: an account-held policy is in force for every group of the
        // account it is held by, so a row carrying one cannot share a store with the next.
        let emulator = Emulator::with_log_groups();
        let group = format!("/aws/opensearch/grants/{index}");
        with_destination(&emulator, &row, &group).await;

        let name = format!("logs-grant-{index}");
        let mut body = baseline_create(&name);
        body["LogPublishingOptions"] = publishing_to(&group);
        let answer = emulator.create(body).await;
        if row.granted {
            assert_eq!(
                answer.status, 200,
                "case CreateDomain({}): expected 200, got {} with {:?}",
                row.case, answer.status, answer.body
            );
            assert_eq!(
                published(&emulator, &name).await,
                Some(publishing_to(&group)),
                "case DescribeDomain({}): expected the created domain to publish to the group",
                row.case
            );
        } else {
            assert_refusal(&answer, REFUSAL, row.case);
            assert_eq!(
                message(&answer, row.case),
                ungranted(&group),
                "case CreateDomain({}): expected the resource-access-policy refusal naming \
                 {group:?}, got {:?}",
                row.case,
                message(&answer, row.case)
            );
            assert_no_domain(&emulator, &name).await;
        }
    }
}

#[tokio::test]
async fn a_change_publishes_only_to_a_group_whose_policy_admits_the_service() {
    for (index, row) in rows().into_iter().enumerate() {
        let emulator = Emulator::with_log_groups();
        let group = format!("/aws/opensearch/grants/{index}");
        with_destination(&emulator, &row, &group).await;

        // The change is made on a settled domain that publishes nothing: a domain with work
        // in flight refuses a further change, so the create's own change lands first.
        let name = format!("logs-grant-up-{index}");
        create_baseline(&emulator, &name).await;
        settled_domain(&emulator, &name).await;
        let before = published(&emulator, &name).await;

        let answer = emulator
            .update_config(&name, json!({ "LogPublishingOptions": publishing_to(&group) }))
            .await;
        if row.granted {
            assert_eq!(
                answer.status, 200,
                "case UpdateDomainConfig({}): expected 200, got {} with {:?}",
                row.case, answer.status, answer.body
            );
            settled_domain(&emulator, &name).await;
            assert_eq!(
                published(&emulator, &name).await,
                Some(publishing_to(&group)),
                "case DescribeDomain({}): expected the changed domain to publish to the group",
                row.case
            );
        } else {
            assert_refusal(&answer, REFUSAL, row.case);
            assert_eq!(
                message(&answer, row.case),
                ungranted(&group),
                "case UpdateDomainConfig({}): expected the resource-access-policy refusal \
                 naming {group:?}, got {:?}",
                row.case,
                message(&answer, row.case)
            );
            // A refused change leaves the domain publishing what it published before.
            assert_eq!(
                published(&emulator, &name).await,
                before,
                "case DescribeDomain({}): a refused change must leave the domain publishing \
                 {before:?}",
                row.case
            );
        }
    }
}

/// RULE: a domain's cluster serves its indexes from at least one data node whatever its
/// `ClusterConfig` STATES, so the work it does is logged from a node and the records reach
/// the group its option names — a domain created without an explicit
/// `ClusterConfig$InstanceCount` publishes its slow logs like any other.
#[tokio::test]
async fn a_cluster_that_states_no_instance_count_still_publishes_its_slow_logs() {
    let emulator = Emulator::with_log_groups();
    let group = "/aws/opensearch/grants/unstated-nodes";
    emulator.created_log_group(group).await;
    let domain = "logs-grant-nodes";
    let mut body = baseline_create(domain);
    // The cluster states its instance TYPE and no count at all, which is the shape of a
    // request that leaves the node count to the service.
    body["ClusterConfig"] = json!({ "InstanceType": "t3.small.search" });
    body["AccessPolicies"] = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "*" },
            "Action": "es:ESHttp*",
            "Resource": "*",
        }],
    })
    .to_string()
    .into();
    body["LogPublishingOptions"] = json!({
        "SEARCH_SLOW_LOGS": {
            "Enabled": true,
            "CloudWatchLogsLogGroupArn": log_group_arn(group),
        },
    });
    let created = emulator.create(body).await;
    let case = "a cluster with no stated instance count";
    assert_eq!(
        created.status, 200,
        "case CreateDomain({case}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    let host = endpoint_of(&emulator, domain);
    let index = "unstated-nodes";
    let made = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        &format!("/{index}"),
        Some(json!({ "settings": { "number_of_shards": 1 } })),
    )
    .await;
    assert_eq!(
        made.status, 200,
        "case {case}: expected PUT /{index} to be taken, got {} with {:?}",
        made.status, made.body
    );
    let set = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        &format!("/{index}/_settings"),
        Some(json!({ "index.search.slowlog.threshold.query.warn": "0ms" })),
    )
    .await;
    assert_eq!(
        set.status, 200,
        "case {case}: expected the threshold to be taken, got {} with {:?}",
        set.status, set.body
    );
    let searched = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        &format!("/{index}/_search"),
        Some(json!({ "query": { "match_all": {} } })),
    )
    .await;
    assert_eq!(
        searched.status, 200,
        "case {case}: expected the search to be served, got {} with {:?}",
        searched.status, searched.body
    );
    let delivered = events(&emulator, group).await;
    assert!(
        delivered
            .iter()
            .any(|message| message.contains("index.search.slowlog.query")
                && message.contains(index)),
        "case {case}: expected the search slow log to state the read, got {delivered:?}"
    );
    // The record comes from a node of the domain's cluster, which is the stream it is
    // written to.
    let streams = emulator
        .logs_call("DescribeLogStreams", json!({ "logGroupName": group }))
        .await;
    assert_eq!(
        streams.body["logStreams"]
            .as_array()
            .map(|streams| streams.len()),
        Some(1),
        "case {case}: expected one stream, for the node that logged, got {:?}",
        streams.body
    );
}

/// One shape of the same configuration: how the index's slow-log threshold was written,
/// and how the search that must be recorded is addressed.
struct Shape {
    case: &'static str,
    /// The body the index is CREATED with.
    create: Value,
    /// The body a `PUT /{index}/_settings` then writes, or `Null` where the create already
    /// carried the threshold.
    settings: Value,
    method: &'static str,
    /// The path the search is addressed to, with `{index}` standing for the index's name.
    path: &'static str,
    /// The body the search carries, or `Null` for one that carries none.
    search: Value,
}

/// The threshold every shape sets, in the flattened form the engine's own `flat_settings`
/// writes a setting in: a duration every search reaches.
fn flattened_threshold() -> Value {
    json!({ "index.search.slowlog.threshold.query.warn": "0ms" })
}

/// The same threshold in the EXPANDED form, the sections its name names.
fn expanded_threshold() -> Value {
    json!({
        "index": {
            "search": { "slowlog": { "threshold": { "query": { "warn": "0ms" } } } },
        },
    })
}

/// RULE: a slow-log threshold names the same index setting whichever FORM it is written in
/// (flattened or expanded) and whichever CALL wrote it (the index create or the
/// index-settings update), so a search of that index is recorded either way — and a search
/// is recorded however it is addressed, including the cluster-wide form that names no index.
#[tokio::test]
async fn a_threshold_is_read_whichever_form_and_call_wrote_it() {
    let emulator = Emulator::with_log_groups();
    let group = "/aws/opensearch/grants/thresholds";
    emulator.created_log_group(group).await;
    let domain = "logs-grant-forms";
    let mut body = baseline_create(domain);
    body["ClusterConfig"] = json!({ "InstanceType": "t3.small.search", "InstanceCount": 1 });
    body["AccessPolicies"] = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "*" },
            "Action": "es:ESHttp*",
            "Resource": "*",
        }],
    })
    .to_string()
    .into();
    body["LogPublishingOptions"] = json!({
        "SEARCH_SLOW_LOGS": {
            "Enabled": true,
            "CloudWatchLogsLogGroupArn": log_group_arn(group),
        },
    });
    let created = emulator.create(body).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({domain:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    let host = endpoint_of(&emulator, domain);

    // The rows differ along what the rule generalises over: the FORM the threshold is
    // written in, the CALL that wrote it, and how the search that must be recorded is
    // addressed (a named index, a URI query, the whole cluster).
    let shapes: Vec<Shape> = vec![
        Shape {
            case: "flattened at the create, searched by name",
            create: json!({ "settings": flattened_threshold() }),
            settings: Value::Null,
            method: "POST",
            path: "/{index}/_search",
            search: json!({ "query": { "match_all": {} } }),
        },
        Shape {
            case: "expanded at the create, searched by name",
            create: json!({ "settings": expanded_threshold() }),
            settings: Value::Null,
            method: "POST",
            path: "/{index}/_search",
            search: json!({ "query": { "match_all": {} } }),
        },
        Shape {
            case: "expanded through the settings API, searched with a URI query",
            create: json!({ "settings": { "number_of_shards": 1 } }),
            settings: expanded_threshold(),
            method: "GET",
            path: "/{index}/_search?q=*",
            search: Value::Null,
        },
        Shape {
            case: "flattened through the settings API, searched with no body",
            create: json!({ "settings": { "number_of_shards": 1 } }),
            settings: flattened_threshold(),
            method: "GET",
            path: "/{index}/_search",
            search: Value::Null,
        },
        Shape {
            case: "flattened at the create, searched across the whole cluster",
            create: json!({ "settings": flattened_threshold() }),
            settings: Value::Null,
            method: "POST",
            path: "/_search",
            search: json!({ "query": { "match_all": {} } }),
        },
    ];

    for (position, shape) in shapes.into_iter().enumerate() {
        let Shape {
            case,
            create,
            settings,
            method,
            path,
            search,
        } = shape;
        // One index per row, so the threshold under test is the only one in force for the
        // index the row reads — the cluster-wide row reads them all, and the record it must
        // produce is the one for its own index.
        let index = format!("threshold-form-{position}");
        let made = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/{index}"),
            Some(create),
        )
        .await;
        assert_eq!(
            made.status, 200,
            "case {case}: expected PUT /{index} to be taken, got {} with {:?}",
            made.status, made.body
        );
        if !settings.is_null() {
            let set = rest(
                &emulator,
                DEFAULT_ACCESS_KEY_ID,
                &host,
                "PUT",
                &format!("/{index}/_settings"),
                Some(settings),
            )
            .await;
            assert_eq!(
                set.status, 200,
                "case {case}: expected the threshold to be taken, got {} with {:?}",
                set.status, set.body
            );
        }
        let before = events(&emulator, group).await;
        let searched = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            method,
            &path.replace("{index}", &index),
            (!search.is_null()).then_some(search),
        )
        .await;
        assert_eq!(
            searched.status, 200,
            "case {case}: expected the search to be served, got {} with {:?}",
            searched.status, searched.body
        );
        let after = events(&emulator, group).await;
        assert!(
            after[before.len()..]
                .iter()
                .any(|message| message.contains("index.search.slowlog.query")
                    && message.contains(&index)),
            "case {case}: expected the search of {index} to be recorded, got {:?}",
            &after[before.len()..]
        );
    }
}

#[tokio::test]
async fn an_option_that_publishes_nothing_needs_no_grant_and_an_absent_group_is_the_earlier_fault()
{
    let emulator = Emulator::with_log_groups();
    let ungranted_group = "/aws/opensearch/grants/quiet";
    emulator
        .created_log_group_without_grant(ungranted_group)
        .await;

    // A DISABLED option delivers nothing, so the permission of the group it names is never
    // asked for: it is stored as written.
    let disabled = json!({
        LOG_TYPE: {
            "Enabled": false,
            "CloudWatchLogsLogGroupArn": log_group_arn(ungranted_group),
        },
    });
    let mut body = baseline_create("logs-grant-disabled");
    body["LogPublishingOptions"] = disabled.clone();
    let answer = emulator.create(body).await;
    assert_eq!(
        answer.status, 200,
        "case \"a disabled option against an ungranted group\": expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        published(&emulator, "logs-grant-disabled").await,
        Some(disabled),
        "case \"a disabled option against an ungranted group\": expected the option to be \
         stored as written"
    );

    // A group nobody created is the EARLIER fault: a caller told to attach a resource policy
    // to a group that is not there is given a remedy that cannot be applied, so the
    // existence sentence is what such a request is refused with — even beside a second
    // destination that exists and grants nothing.
    let mut body = baseline_create("logs-grant-absent");
    body["LogPublishingOptions"] = json!({
        "INDEX_SLOW_LOGS": {
            "Enabled": true,
            "CloudWatchLogsLogGroupArn": log_group_arn(ungranted_group),
        },
        "SEARCH_SLOW_LOGS": {
            "Enabled": true,
            "CloudWatchLogsLogGroupArn": log_group_arn("/aws/opensearch/grants/never-created"),
        },
    });
    let answer = emulator.create(body).await;
    let case = "an ungranted group beside one nobody created";
    assert_refusal(&answer, REFUSAL, case);
    assert_eq!(
        message(&answer, case),
        "The specified CloudWatch Logs log group doesn't exist.",
        "case CreateDomain({case}): expected the existence refusal, got {:?}",
        message(&answer, case)
    );
    assert_no_domain(&emulator, "logs-grant-absent").await;
}
