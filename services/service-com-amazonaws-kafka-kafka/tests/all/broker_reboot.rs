// SPDX-License-Identifier: CC0-1.0

//! `RebootBroker` end to end, over the same `MultiServiceRouter` the emulator
//! serves from, through the harness in `tests/all/kafka_harness.rs`.
//!
//! The rules under test, each asserted over a TABLE rather than one request:
//!
//! 1. an accepted reboot is ASYNCHRONOUS: it files a `REBOOT_NODE` cluster
//!    operation, leaves the cluster `REBOOTING_BROKER` with that operation as its
//!    `ActiveOperationArn`, and the cluster is `ACTIVE` again once the clock owes
//!    the operation its completion — at the SAME `CurrentVersion`, holding the same
//!    broker nodes with the same endpoints, because a reboot restarts the brokers
//!    and changes no cluster attribute (which is why the request quotes no
//!    `CurrentVersion`);
//! 2. the request must name EXACTLY ONE broker the cluster holds — "the
//!    reboot-broker operation supports rebooting one broker at a time"
//!    (`RebootBrokerRequest$BrokerIds`);
//! 3. the cluster the URI names must be a provisioned one this endpoint holds and
//!    must be `ACTIVE`, so a reboot while an operation is in flight is refused;
//! 4. authorization is decided at the IAM layer FIRST: a caller whose
//!    identity-based policy explicitly denies the action is refused
//!    `AccessDeniedException` naming the principal, the action and the resource —
//!    before the cluster is resolved, so a denied caller learns nothing about
//!    whether it exists.
//!
//! The rows differ along what each rule generalises over: how many brokers the
//! cluster holds and which of them is rebooted, how many ids the request carries
//! and whether they name nodes at all, the kind of ARN and the state the cluster is
//! in, and — for the IAM rule — the action and the resource the stored document
//! names.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_kafka_kafka::state::{
    OPERATION_STATE_REBOOT_COMPLETE, OPERATION_STATE_REBOOT_IN_PROGRESS,
    OPERATION_TYPE_REBOOT_NODE, PROVISIONED_ONLY_MESSAGE, RESOURCE_NOT_FOUND_MESSAGE,
};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::kafka_harness::{
    assert_cluster_state_refusal, pct, sleep_past_activation, sleep_past_broker_reboot,
    ClusterRequest, Emulator, Reply, ACCOUNT, DEFAULT_ACCESS_KEY_ID, REGION,
};

/// A cluster id of the form this service mints, for an ARN naming a cluster no
/// registry of this emulator holds.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// The wire error code an authorization refusal carries, and the status it carries,
/// spelled out here rather than read from the service so a drift in the service's
/// own constants fails this suite.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The role an assumed-role session of the rows below speaks for, the session's
/// name, the inline policy put on that role, and the access key id the session was
/// minted for.
const ROLE: &str = "kafka-reboot-caller";
const SESSION: &str = "reboot-broker-session";
const POLICY_NAME: &str = "reboot-broker-access";
const SESSION_KEY_ID: &str = "ASIAREBOOTTESTONLY00";

/// How long the session the rows register lasts: long enough that none expires
/// while the suite runs.
const SESSION_SECONDS: i32 = 3_600;

/// `PUT /v1/clusters/{ClusterArn}/reboot-broker` — the route this suite drives,
/// signed with the account's own published credential.
async fn reboot(emulator: &Emulator, cluster_arn: &str, broker_ids: &[&str]) -> Reply {
    reboot_as(emulator, DEFAULT_ACCESS_KEY_ID, cluster_arn, broker_ids).await
}

/// The same call, signed with `access_key_id` — the credential whose principal the
/// service authorizes the request as.
async fn reboot_as(
    emulator: &Emulator,
    access_key_id: &str,
    cluster_arn: &str,
    broker_ids: &[&str],
) -> Reply {
    emulator
        .call_as_in(
            access_key_id,
            REGION,
            "PUT",
            &format!("/v1/clusters/{}/reboot-broker", pct(cluster_arn)),
            Some(json!({ "brokerIds": broker_ids })),
        )
        .await
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}` — the
/// reader that reports the operation a reboot filed.
async fn describe_operation(emulator: &Emulator, operation_arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(operation_arn)), None)
        .await
}

/// A cluster created with `brokers` nodes across `subnets` and waited into
/// `ACTIVE`, as (ARN, `CurrentVersion`): the state a reboot is accepted from.
async fn active_cluster(
    emulator: &Emulator,
    name: &str,
    brokers: i64,
    subnets: Vec<&'static str>,
) -> (String, String) {
    let request = ClusterRequest {
        brokers,
        subnets,
        ..ClusterRequest::named(name)
    };
    let arn = emulator.create_ok(&request).await;
    sleep_past_activation();
    let described = emulator.describe(&arn).await;
    assert_eq!(
        described.at("/clusterInfo/state").as_str(),
        Some("ACTIVE"),
        "case {name}: expected the new cluster to settle ACTIVE, got {}",
        described.body
    );
    let version = described
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: DescribeCluster reported no currentVersion"))
        .to_string();
    (arn, version)
}

/// The broker ids `ListNodes` reports for the cluster, in the order it reports
/// them, each with the endpoints the node is reached on: the nodes the cluster
/// actually holds, which a reboot must bring back unchanged.
async fn listed_nodes(emulator: &Emulator, cluster_arn: &str, case: &str) -> Vec<(i64, Value)> {
    let reply = emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/nodes", pct(cluster_arn)),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListNodes to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/nodeInfoList")
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: ListNodes reported no nodeInfoList: {}", reply.body)
        })
        .iter()
        .map(|node| {
            let id = node
                .pointer("/brokerNodeInfo/brokerId")
                .and_then(Value::as_f64)
                .map(|id| id as i64)
                .unwrap_or_else(|| panic!("case {case}: a node reported no brokerId: {node}"));
            let endpoints = node
                .pointer("/brokerNodeInfo/endpoints")
                .cloned()
                .unwrap_or(Value::Null);
            (id, endpoints)
        })
        .collect()
}

/// `arn:aws:kafka:<region>:<account>:cluster-operation/<name>/<cluster-id>/<operation-id>`:
/// a cluster operation hangs off the resource tail of the cluster it belongs to.
fn assert_operation_arn(operation_arn: &str, cluster_arn: &str, case: &str) {
    let cluster_tail = cluster_arn
        .rsplit(':')
        .next()
        .and_then(|resource| resource.strip_prefix("cluster/"))
        .unwrap_or_else(|| panic!("case {case}: {cluster_arn:?} is not a cluster ARN"));
    let prefix = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{cluster_tail}/");
    assert!(
        operation_arn.starts_with(&prefix),
        "case {case}: expected an operation ARN starting {prefix:?}, got {operation_arn:?}"
    );
}

/// RULE: an accepted `RebootBroker` does not rewrite the cluster where it stands —
/// it files a cluster operation of type `REBOOT_NODE`, so until the clock owes that
/// operation its completion the cluster reads `REBOOTING_BROKER` with the operation
/// as its `ActiveOperationArn`; once it lands the cluster is `ACTIVE` again at the
/// version it already was, holding the same broker nodes on the same endpoints,
/// since a reboot restarts the brokers and changes no cluster attribute.
#[tokio::test]
async fn an_accepted_reboot_runs_as_a_cluster_operation_and_changes_no_attribute() {
    // The rows differ along what the rule generalises over: how many brokers the
    // cluster holds (one, several, and the published maximum the model's
    // `__integerMin1Max15` range allows), which of them the request names (the
    // first, a middle one, the last), and how many client subnets the nodes are
    // spread over.
    let rows: &[(&str, i64, &str, Vec<&'static str>)] = &[
        ("the only broker", 1, "1", vec!["subnet-a"]),
        (
            "the first of three",
            3,
            "1",
            vec!["subnet-a", "subnet-b", "subnet-c"],
        ),
        (
            "the middle of three",
            3,
            "2",
            vec!["subnet-a", "subnet-b", "subnet-c"],
        ),
        (
            "the last of three",
            3,
            "3",
            vec!["subnet-a", "subnet-b", "subnet-c"],
        ),
        ("the last of fifteen", 15, "15", vec!["subnet-a", "subnet-b"]),
    ];

    let emulator = Emulator::new();
    for (case, brokers, broker_id, subnets) in rows {
        let (arn, version) = active_cluster(&emulator, case, *brokers, subnets.clone()).await;
        let before = emulator.describe(&arn).await.at("/clusterInfo").clone();
        let nodes_before = listed_nodes(&emulator, &arn, case).await;

        let reply = reboot(&emulator, &arn, &[broker_id]).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected RebootBroker to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {case}: RebootBroker reported the wrong clusterArn, got {}",
            reply.body["clusterArn"]
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: RebootBroker returned no clusterOperationArn"))
            .to_string();
        assert_operation_arn(&operation_arn, &arn, case);

        // Still rebooting: the operation is the cluster's active one, the cluster
        // is in the state a reboot puts it in, and its version has not moved.
        let rebooting = emulator.describe(&arn).await;
        assert_eq!(
            rebooting.at("/clusterInfo/state").as_str(),
            Some("REBOOTING_BROKER"),
            "case {case}: expected the cluster to read REBOOTING_BROKER, got {}",
            rebooting.body
        );
        assert_eq!(
            rebooting.at("/clusterInfo/activeOperationArn").as_str(),
            Some(operation_arn.as_str()),
            "case {case}: expected activeOperationArn {operation_arn:?}, got {}",
            rebooting.at("/clusterInfo/activeOperationArn")
        );
        assert_eq!(
            rebooting.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: a reboot generates no new cluster version, got {}",
            rebooting.at("/clusterInfo/currentVersion")
        );

        // The operation the call filed is readable while it runs, under this
        // call's own operation type.
        let running = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            (
                running.status,
                running.at("/clusterOperationInfo/operationType").as_str(),
                running.at("/clusterOperationInfo/operationState").as_str(),
                running.at("/clusterOperationInfo/clusterArn").as_str(),
            ),
            (
                200,
                Some(OPERATION_TYPE_REBOOT_NODE),
                Some(OPERATION_STATE_REBOOT_IN_PROGRESS),
                Some(arn.as_str()),
            ),
            "case {case}: expected a running {OPERATION_TYPE_REBOOT_NODE} operation on this \
             cluster, got {} {}",
            running.status,
            running.body
        );
        assert!(
            running.at("/clusterOperationInfo/endTime").is_null(),
            "case {case}: an operation still running has not ended, got {}",
            running.body
        );

        // A second reboot while the first is in flight is refused: a cluster runs
        // one operation at a time, so the cluster is not in a state a change is
        // accepted from.
        let contended = reboot(&emulator, &arn, &[broker_id]).await;
        assert_cluster_state_refusal(&contended, case);

        sleep_past_broker_reboot();

        // Landed: ACTIVE again, no active operation, the SAME version, and every
        // other member of the record exactly as it was.
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: expected the cluster to settle ACTIVE again, got {}",
            after.body
        );
        assert!(
            after.at("/clusterInfo/activeOperationArn").is_null(),
            "case {case}: a completed reboot leaves no active operation, got {}",
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo").clone(),
            before,
            "case {case}: a reboot must leave every cluster attribute as it was, got {}",
            after.body
        );
        let nodes_after = listed_nodes(&emulator, &arn, case).await;
        assert_eq!(
            nodes_after, nodes_before,
            "case {case}: the rebooted cluster must hold the same nodes on the same endpoints, \
             got {nodes_after:?}"
        );

        // The operation is complete, and it reports when it ended.
        let finished = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            finished
                .at("/clusterOperationInfo/operationState")
                .as_str(),
            Some(OPERATION_STATE_REBOOT_COMPLETE),
            "case {case}: expected the operation to read {OPERATION_STATE_REBOOT_COMPLETE}, got {}",
            finished.body
        );
        assert!(
            finished.at("/clusterOperationInfo/endTime").is_string(),
            "case {case}: a finished operation reports its end time, got {}",
            finished.body
        );

        // And it is in the cluster's own operation history.
        let history = emulator
            .call("GET", &format!("/v1/clusters/{}/operations", pct(&arn)), None)
            .await;
        let listed: Vec<&str> = history
            .at("/clusterOperationInfoList")
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: ListClusterOperations reported no list"))
            .iter()
            .filter_map(|entry| entry.pointer("/operationArn").and_then(Value::as_str))
            .collect();
        assert!(
            listed.contains(&operation_arn.as_str()),
            "case {case}: expected {operation_arn:?} in the cluster's operation history, got \
             {listed:?}"
        );
    }
}

/// RULE: each accepted reboot files an operation of ITS OWN, so a cluster rebooted
/// twice holds two operations a reader can tell apart — even though neither moved
/// the cluster's version.
#[tokio::test]
async fn every_reboot_files_its_own_operation() {
    let emulator = Emulator::new();
    let (arn, version) = active_cluster(&emulator, "twice-rebooted", 2, vec!["subnet-a"]).await;

    let mut operations = Vec::new();
    for round in ["first", "second", "third"] {
        let reply = reboot(&emulator, &arn, &["1"]).await;
        assert_eq!(
            reply.status, 200,
            "case {round}: expected RebootBroker to return 200, got {} {}",
            reply.status, reply.body
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {round}: RebootBroker returned no clusterOperationArn"))
            .to_string();
        let described = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            described.status, 200,
            "case {round}: the operation a reboot answers with must resolve, got {} {}",
            described.status, described.body
        );
        operations.push(operation_arn);
        sleep_past_broker_reboot();
    }
    let distinct: std::collections::BTreeSet<&String> = operations.iter().collect();
    assert_eq!(
        distinct.len(),
        operations.len(),
        "each reboot must mint its own operation ARN, got {operations:?}"
    );
    let settled = emulator.describe(&arn).await;
    assert_eq!(
        settled.at("/clusterInfo/currentVersion").as_str(),
        Some(version.as_str()),
        "no reboot generates a new cluster version, got {}",
        settled.body
    );
}

/// RULE: the request must name exactly one broker the cluster holds — "the
/// reboot-broker operation supports rebooting one broker at a time" — so a request
/// naming none, several, or a value that is no node of this cluster is refused with
/// the modelled client-input error naming `BrokerIds`, and the cluster is left
/// exactly as it was.
#[tokio::test]
async fn a_reboot_names_exactly_one_broker_the_cluster_holds() {
    let emulator = Emulator::new();
    let (arn, version) = active_cluster(&emulator, "broker-ids", 3, vec!["subnet-a"]).await;

    // The rows differ along what the rule generalises over: how many ids the
    // request carries, and whether the id it carries names a node of this cluster
    // (below the first, past the last, and a value that denotes no number at all).
    let rows: &[(&str, &[&str])] = &[
        ("no broker at all", &[]),
        ("two brokers", &["1", "2"]),
        ("every broker", &["1", "2", "3"]),
        ("the same broker twice", &["1", "1"]),
        ("the id below the first", &["0"]),
        ("a negative id", &["-1"]),
        ("the id past the last", &["4"]),
        ("an id far past the last", &["99"]),
        ("a value that is no number", &["one"]),
        ("an id padded with whitespace", &[" 1 "]),
        ("an empty id", &[""]),
    ];

    for (case, broker_ids) in rows {
        let reply = reboot(&emulator, &arn, broker_ids).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case:?}: expected a 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("BrokerIds"),
            "case {case:?}: expected the refusal to name BrokerIds, got {}",
            reply.body
        );
        let described = emulator.describe(&arn).await;
        assert_eq!(
            (
                described.at("/clusterInfo/state").as_str(),
                described.at("/clusterInfo/currentVersion").as_str(),
            ),
            (Some("ACTIVE"), Some(version.as_str())),
            "case {case:?}: a refused reboot must leave the cluster as it was, got {}",
            described.body
        );
    }

    // The control: the same call naming one broker the cluster holds is served.
    let served = reboot(&emulator, &arn, &["2"]).await;
    assert_eq!(
        served.status, 200,
        "case \"one broker the cluster holds\": expected 200, got {} {}",
        served.status, served.body
    );
}

/// RULE: a reboot is served only for a PROVISIONED cluster this endpoint holds that
/// is `ACTIVE` — the brokers it restarts are a provisioned cluster's own nodes, and
/// a cluster still being provisioned or already running an operation has no settled
/// brokers to restart. A value that is not a cluster ARN is a bad request, and a
/// well-formed ARN this endpoint holds no cluster for is the not-found refusal.
#[tokio::test]
async fn a_reboot_is_refused_unless_the_cluster_is_an_active_provisioned_one() {
    let emulator = Emulator::new();

    // A cluster still being provisioned: created and NOT waited for.
    let creating = emulator
        .create_ok(&ClusterRequest::named("still-creating"))
        .await;
    let refused = reboot(&emulator, &creating, &["1"]).await;
    assert_cluster_state_refusal(&refused, "a CREATING cluster");

    // A serverless cluster has no customer broker nodes to reboot.
    let serverless = emulator
        .create_serverless_ok("serverless-reboot", &[vec!["subnet-a"]], None)
        .await;
    sleep_past_activation();
    let serverless_reply = reboot(&emulator, &serverless, &["1"]).await;
    assert_eq!(
        (
            serverless_reply.status,
            serverless_reply.error_code(),
            serverless_reply.error_message()
        ),
        (400, "BadRequestException", PROVISIONED_ONLY_MESSAGE),
        "case \"a serverless cluster\": expected the provisioned-only refusal, got {} {}",
        serverless_reply.status,
        serverless_reply.body
    );

    // A well-formed cluster ARN of this account and region that names no cluster.
    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/{GHOST_ID}");
    let not_found = reboot(&emulator, &absent, &["1"]).await;
    assert_eq!(
        (not_found.status, not_found.error_code(), not_found.error_message()),
        (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
        "case \"an absent cluster\": expected the not-found refusal, got {} {}",
        not_found.status,
        not_found.body
    );

    // Values that are not cluster ARNs at all: the rows differ along which field
    // of the ARN form is wrong.
    let malformed: &[(&str, String)] = &[
        ("not an ARN", "a-cluster".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:cluster/orders/{GHOST_ID}"),
        ),
        (
            "another resource type of this service",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/orders/{GHOST_ID}"),
        ),
        (
            "a cluster ARN with no id segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/orders"),
        ),
    ];
    for (case, value) in malformed {
        let reply = reboot(&emulator, value, &["1"]).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case:?}: expected a 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("clusterArn"),
            "case {case:?}: expected the refusal to name clusterArn, got {}",
            reply.body
        );
    }
}

/// The session ARN `sts:AssumeRole` reports for the session the rows below sign
/// with, which is the principal a refusal names.
fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION}")
}

/// The ARN the account holds the role under — the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the in-memory engine carries IAM's store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEKAFKAREB0".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: ACCOUNT.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let sts_store = sts::service_state(&**engine).expect("the in-memory engine carries STS's store");
    sts_store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session registry has room for one session");
}

/// Attach `document` to the role as its inline policy, replacing whatever it
/// carried — what `iam:PutRolePolicy` does. `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the in-memory engine carries IAM's store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(ROLE));
        }
        Some(document) => {
            store.role_inline_policies.insert(
                iam::name_key(ROLE),
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// The sentence IAM refuses a request an identity-based policy explicitly denies
/// with: fixed text around the principal the credential speaks for, the IAM action
/// the operation is authorized under, and the resource the request addressed.
fn explicit_deny_message(resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: kafka:RebootBroker on resource: {resource} \
         with an explicit deny in an identity-based policy",
        session_arn()
    )
}

/// RULE: a request is authorized at the IAM layer before the service evaluates the
/// resource, so a caller whose identity-based policy explicitly denies
/// `kafka:RebootBroker` on the cluster the request names is refused
/// `AccessDeniedException` carrying IAM's sentence — and a caller no readable
/// document denies reaches this operation's own handling, whatever it answers.
#[tokio::test]
async fn an_identity_policy_that_denies_the_action_refuses_the_reboot() {
    let emulator = Emulator::new();
    given_session(&emulator.engine);
    let (arn, _version) = active_cluster(&emulator, "identity-gate", 2, vec!["subnet-a"]).await;
    let other = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/other/{GHOST_ID}");

    // The rows differ along what the rule generalises over: whether a document is
    // in force at all, which ACTIONS it names (this one, every action of this
    // service, every action at all, another action of this service, another
    // service's), which RESOURCE it is scoped to (this cluster, every resource,
    // another cluster), and whether a `Deny` sits beside an `Allow` of the same
    // action. `true` is a row the gate refuses.
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("no policy at all", None, false),
        (
            "a deny of exactly this action on every resource",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Deny", "Action": "kafka:RebootBroker", "Resource": "*" }
                ],
            })),
            true,
        ),
        (
            "a deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": "kafka:RebootBroker", "Resource": "*" },
                    { "Effect": "Deny", "Action": "kafka:*", "Resource": "*" },
                ],
            })),
            true,
        ),
        (
            "a deny of every action",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{ "Effect": "Deny", "Action": "*", "Resource": "*" }],
            })),
            true,
        ),
        (
            "a deny scoped to this cluster",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Deny", "Action": "kafka:*", "Resource": arn.clone() }
                ],
            })),
            true,
        ),
        (
            "a deny scoped to another cluster",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Deny", "Action": "kafka:*", "Resource": other.clone() }
                ],
            })),
            false,
        ),
        (
            "a deny of another action of this service",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Deny", "Action": "kafka:DeleteCluster", "Resource": "*" }
                ],
            })),
            false,
        ),
        (
            "a deny of another service's actions",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{ "Effect": "Deny", "Action": "s3:*", "Resource": "*" }],
            })),
            false,
        ),
        (
            "an allow of every action of this service",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{ "Effect": "Allow", "Action": "kafka:*", "Resource": "*" }],
            })),
            false,
        ),
    ];

    for (case, document, refused) in rows {
        given_role_policy(&emulator.engine, document.as_ref());
        let reply = reboot_as(&emulator, SESSION_KEY_ID, &arn, &["1"]).await;
        if refused {
            assert_eq!(
                (reply.status, reply.error_code(), reply.error_message()),
                (
                    ACCESS_DENIED_STATUS,
                    ACCESS_DENIED_CODE,
                    explicit_deny_message(&arn).as_str()
                ),
                "case {case:?}: expected IAM's explicit-deny refusal, got {} {}",
                reply.status,
                reply.body
            );
            // Refused at the gate: nothing was filed, so the cluster is untouched.
            let described = emulator.describe(&arn).await;
            assert_eq!(
                described.at("/clusterInfo/state").as_str(),
                Some("ACTIVE"),
                "case {case:?}: a refused reboot must file no operation, got {}",
                described.body
            );
        } else {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: nothing denies this caller, so the reboot must be served, got {} \
                 {}",
                reply.status, reply.body
            );
            sleep_past_broker_reboot();
        }
    }
}

/// RULE: the IAM decision comes FIRST, so a denied caller is told it is not
/// authorized rather than told whether the cluster exists — and a caller nothing
/// denies reaches this operation's own answer for the same request.
#[tokio::test]
async fn the_identity_decision_precedes_resolving_the_cluster() {
    let emulator = Emulator::new();
    given_session(&emulator.engine);
    given_role_policy(
        &emulator.engine,
        Some(&json!({
            "Version": "2012-10-17",
            "Statement": [{ "Effect": "Deny", "Action": "kafka:*", "Resource": "*" }],
        })),
    );

    // Requests this operation would otherwise refuse for a reason of its own: a
    // cluster that does not exist, a value that is not a cluster ARN, and a body
    // naming no broker. Every one of them is answered with the denial instead.
    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/{GHOST_ID}");
    let rows: &[(&str, &str, &[&str])] = &[
        ("a cluster this endpoint holds nothing for", absent.as_str(), &["1"]),
        ("a value that is not a cluster ARN", "a-cluster", &["1"]),
        ("a request naming no broker", absent.as_str(), &[]),
    ];

    for (case, target, broker_ids) in rows {
        let denied = reboot_as(&emulator, SESSION_KEY_ID, target, broker_ids).await;
        assert_eq!(
            (denied.status, denied.error_code(), denied.error_message()),
            (
                ACCESS_DENIED_STATUS,
                ACCESS_DENIED_CODE,
                explicit_deny_message(target).as_str()
            ),
            "case {case:?}: expected the denial before anything resolves the target, got {} {}",
            denied.status,
            denied.body
        );
        assert!(
            !denied.error_message().contains(RESOURCE_NOT_FOUND_MESSAGE),
            "case {case:?}: the refusal must not report whether the cluster exists, got {}",
            denied.body
        );

        // The control: the same request from the account's own credential, which no
        // document denies, reaches this operation's own refusal.
        let own = reboot(&emulator, target, broker_ids).await;
        assert_ne!(
            own.error_code(),
            ACCESS_DENIED_CODE,
            "case {case:?}: nothing denies the account's own credential, so the gate must not \
             refuse it, got {}",
            own.body
        );
    }
}
