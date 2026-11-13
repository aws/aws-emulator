// SPDX-License-Identifier: CC0-1.0

//! Amazon MSK answers its OWN refusal before it resolves the subnets a create names
//! against Amazon EC2.
//!
//! RULE: an Amazon MSK create decides a request from the request itself and from this
//! service's own registry first — the role the caller may pass, the values the body
//! carries, and whether the name it asks for is already taken in the Region — and only
//! then resolves the placement it asks for against the account's EC2 inventory. So a
//! create MSK refuses on its own terms is answered with THAT refusal, in that refusal's
//! own class (HTTP 409 `ConflictException` for a name already taken, HTTP 403
//! `AccessDeniedException` for a role of another account, HTTP 400 `BadRequestException`
//! for a value MSK does not accept), and never with the placement refusal; only a create
//! MSK has nothing of its own to say about is answered by the subnet resolution.
//!
//! This suite lives beside the router because the subnets are EC2's: the VPC and the
//! subnets below are minted by the real EC2 operations on the SAME engine that serves
//! Amazon MSK, so the resolution this suite orders MSK's own decisions ahead of is the one
//! that really runs — and so an id EC2 holds no subnet for is really unresolvable rather
//! than merely unasked.
//!
//! The rows differ along what the rule generalises over: the OPERATION and the body the
//! name travels in (`CreateCluster`, both of `CreateClusterV2`'s bodies,
//! `CreateReplicator`), WHICH of MSK's own decisions pre-empts the lookup (a name the
//! Region already holds, a service-execution role of another account, a request value the
//! service does not accept), and the FORM of the id EC2 holds no subnet for (EC2's 17-hex
//! id form, its older 8-hex form, and a value that is no subnet id at all).
//!
//! Error source: `smithy_query(query="operation", operation="CreateCluster,CreateClusterV2,
//! CreateReplicator")` on the kafka model — `ConflictException` (`httpError` 409),
//! `BadRequestException` (400) and `ForbiddenException` (403); the `AccessDeniedException`
//! code a 403 carries is the one the real service answers.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account this emulator serves, and therefore the account a role may be passed from.
const ACCOUNT: &str = "000000000000";

/// What Amazon MSK answers a create whose client subnets EC2 holds none of: one fixed
/// sentence (its apostrophe is U+2019), with no subnet id of the request in it. Written out
/// here rather than read from the service's own constant, so the wording is pinned by this
/// suite instead of by the code under test.
const SUBNETS_ABSENT_MESSAGE: &str =
    "One or more of the specified client subnets don\u{2019}t exist.";

/// What Amazon MSK answers a create whose name the Region already holds: fixed text end to
/// end, naming neither the name nor the resource already carrying it.
const NAME_ALREADY_EXISTS_MESSAGE: &str = "A resource with this name already exists.";

/// What Amazon MSK answers a `CreateReplicator` whose service-execution role belongs to
/// another account: fixed text end to end, carrying nothing from the request.
const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// What Amazon MSK answers a `CreateReplicator` naming anything but the one source and the
/// one target a replicator replicates between.
const KAFKA_CLUSTER_COUNT_MESSAGE: &str = "Too many or too few kafka clusters specified.";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The Apache Kafka version every create below asks for, and one no catalog of this
/// service publishes.
const KAFKA_VERSION: &str = "3.6.0";
const UNKNOWN_KAFKA_VERSION: &str = "9.9.9";

/// The broker instance type every provisioned create below asks for.
const INSTANCE_TYPE: &str = "kafka.m5.large";

/// The IAM role of THIS account every replicator below is handed, and one of another
/// account, which no caller of this emulator may pass.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";
const FOREIGN_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/service-role/MskReplicatorRole";

/// One emulator carrying Amazon EC2 and Amazon MSK over ONE shared engine, as the
/// production router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is
/// what routes the request here; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One answer: its HTTP status and its body as JSON (`Value::Null` when it is not JSON).
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type` of the restJson1 error envelope), or `""` on success.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The error envelope's message, or `""` when there is none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The member the refusal names at fault, or `""` when it names none.
    fn invalid_parameter(&self) -> &str {
        self.body
            .get("invalidParameter")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Call one Amazon MSK operation over its restJson1 wire.
async fn kafka(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Reply {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("kafka.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("kafka"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    Reply { status, body }
}

/// One ec2Query action that must succeed, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {text}"
    );
    text
}

/// The network every case runs over: one VPC with two subnets, minted by EC2 itself, so
/// every id an accepted create names is one EC2 handed out.
async fn ec2_subnets(router: &MultiServiceRouter) -> Vec<String> {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.90.0.0/16")]).await,
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (cidr, zone) in [("10.90.1.0/24", "us-west-2a"), ("10.90.2.0/24", "us-west-2b")] {
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", cidr),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    subnets
}

/// Values that name no subnet of the account, whatever operation names them: EC2's 17-hex
/// id form, its older 8-hex form, and a value that is no subnet id at all.
fn unresolvable_subnet_ids() -> Vec<(&'static str, &'static str)> {
    vec![
        ("EC2's 17-hex id form", "subnet-0f1e2d3c4b5a69780"),
        ("EC2's older 8-hex id form", "subnet-1a2b3c4d"),
        ("a value that is no subnet id at all", "not-a-subnet"),
    ]
}

/// A `CreateCluster` body placing two brokers in `subnets`, asking for `version`.
fn v1_body(name: &str, version: &str, subnets: &[String]) -> Value {
    json!({
        "clusterName": name,
        "kafkaVersion": version,
        "numberOfBrokerNodes": 2,
        "brokerNodeGroupInfo": {
            "instanceType": INSTANCE_TYPE,
            "clientSubnets": subnets,
        },
    })
}

/// A `CreateClusterV2` body whose `Provisioned` block places two brokers in `subnets`.
fn v2_provisioned_body(name: &str, subnets: &[String]) -> Value {
    json!({
        "clusterName": name,
        "provisioned": {
            "kafkaVersion": KAFKA_VERSION,
            "numberOfBrokerNodes": 2,
            "brokerNodeGroupInfo": {
                "instanceType": INSTANCE_TYPE,
                "clientSubnets": subnets,
            },
        },
    })
}

/// A `CreateClusterV2` body whose `Serverless` block carries one VPC configuration over
/// `subnets`.
fn v2_serverless_body(name: &str, subnets: &[String]) -> Value {
    json!({
        "clusterName": name,
        "serverless": { "vpcConfigs": [{ "subnetIds": subnets }] },
    })
}

/// A `CreateReplicator` body replicating between the Amazon MSK clusters `clusters` names,
/// with the replicator's own client interfaces placed in `subnets` and `role` handed to the
/// service.
fn replicator_body(name: &str, clusters: &[&str], role: &str, subnets: &[String]) -> Value {
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": role,
        "kafkaClusters": clusters
            .iter()
            .map(|arn| json!({
                "amazonMskCluster": { "mskClusterArn": arn },
                "vpcConfig": { "subnetIds": subnets },
            }))
            .collect::<Vec<Value>>(),
        "replicationInfoList": [{
            "sourceKafkaClusterArn": clusters.first().copied().unwrap_or_default(),
            "targetKafkaClusterArn": clusters.get(1).copied().unwrap_or_default(),
            "targetCompressionType": "NONE",
            "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
            "topicReplication": { "topicsToReplicate": [".*"] },
        }],
    })
}

/// `POST /v1/clusters`, asserted accepted, answering the ARN it minted.
async fn create_v1_ok(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let reply = kafka(router, "POST", "/v1/clusters", Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "setup {case:?}: a create placed in EC2's own subnets must be accepted, got {} with {}",
        reply.status, reply.body
    );
    reply
        .body
        .get("clusterArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: the create reported no clusterArn: {}", reply.body))
        .to_string()
}

/// `POST /replication/v1/replicators`, asserted accepted.
async fn create_replicator_ok(router: &MultiServiceRouter, body: Value, case: &str) {
    let reply = kafka(router, "POST", "/replication/v1/replicators", Some(body)).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (200, ""),
        "setup {case:?}: the create must be accepted, got {} with {}",
        reply.status,
        reply.body
    );
}

/// How the sentence a refusal carries is asserted: in full, or by the opening it must have
/// (used where the rest of the sentence is a catalog this suite does not pin).
enum Sentence {
    Whole(&'static str),
    Opening(String),
}

/// One row: a create that MSK refuses on its OWN terms while ALSO naming a subnet EC2
/// holds none of, and the refusal it must answer.
struct Row {
    case: String,
    uri: &'static str,
    body: Value,
    status: u16,
    code: &'static str,
    /// The member the refusal must name at fault — `""` where the refusal names none.
    parameter: &'static str,
    sentence: Sentence,
}

/// RULE (the ordering half): a create that MSK refuses from the request or from its own
/// registry is answered with that refusal, in its own class, even though the subnets it
/// names are ones EC2 holds none of — so the placement resolution never pre-empts a name
/// conflict, a pass-role denial or a value refusal.
#[tokio::test]
async fn msks_own_refusal_is_answered_before_the_subnets_are_resolved() {
    let router = emulator();
    let subnets = ec2_subnets(&router).await;

    // The state the rows below conflict with: a cluster whose name is then asked for
    // again, the two clusters a replicator can really replicate between (so the setup's
    // clusters RESOLVE and the subnet lookup is the next thing the handler would answer
    // from), and a replicator whose name is then asked for again.
    create_v1_ok(
        &router,
        v1_body("taken-cluster", KAFKA_VERSION, &subnets),
        "the cluster holding the name",
    )
    .await;
    let source = create_v1_ok(
        &router,
        v1_body("replicator-source", KAFKA_VERSION, &subnets),
        "the replicator's source cluster",
    )
    .await;
    let target = create_v1_ok(
        &router,
        v1_body("replicator-target", KAFKA_VERSION, &subnets),
        "the replicator's target cluster",
    )
    .await;
    let ends = [source.as_str(), target.as_str()];
    create_replicator_ok(
        &router,
        replicator_body("taken-replicator", &ends, ROLE_ARN, &subnets),
        "the replicator holding the name",
    )
    .await;

    for (form, unknown) in unresolvable_subnet_ids() {
        // One id EC2 holds a subnet for and one it does not: the placement is
        // unresolvable, and every row below names it.
        let unknown_subnets = vec![subnets[0].clone(), unknown.to_string()];
        let rows: Vec<Row> = vec![
            Row {
                case: format!("CreateCluster under a name the Region holds, {form}"),
                uri: "/v1/clusters",
                body: v1_body("taken-cluster", KAFKA_VERSION, &unknown_subnets),
                status: 409,
                code: "ConflictException",
                parameter: "clusterName",
                sentence: Sentence::Whole(NAME_ALREADY_EXISTS_MESSAGE),
            },
            Row {
                case: format!("CreateClusterV2 (provisioned) under a name the Region holds, {form}"),
                uri: "/api/v2/clusters",
                body: v2_provisioned_body("taken-cluster", &unknown_subnets),
                status: 409,
                code: "ConflictException",
                parameter: "clusterName",
                sentence: Sentence::Whole(NAME_ALREADY_EXISTS_MESSAGE),
            },
            Row {
                case: format!("CreateClusterV2 (serverless) under a name the Region holds, {form}"),
                uri: "/api/v2/clusters",
                body: v2_serverless_body("taken-cluster", &unknown_subnets),
                status: 409,
                code: "ConflictException",
                parameter: "clusterName",
                sentence: Sentence::Whole(NAME_ALREADY_EXISTS_MESSAGE),
            },
            Row {
                case: format!("CreateReplicator under a name the Region holds, {form}"),
                uri: "/replication/v1/replicators",
                body: replicator_body("taken-replicator", &ends, ROLE_ARN, &unknown_subnets),
                status: 409,
                code: "ConflictException",
                parameter: "replicatorName",
                sentence: Sentence::Whole(NAME_ALREADY_EXISTS_MESSAGE),
            },
            Row {
                case: format!("CreateReplicator passing a role of another account, {form}"),
                uri: "/replication/v1/replicators",
                body: replicator_body(
                    &format!("foreign-role-{}", form.len()),
                    &ends,
                    FOREIGN_ROLE_ARN,
                    &unknown_subnets,
                ),
                status: 403,
                code: "AccessDeniedException",
                parameter: "",
                sentence: Sentence::Whole(CROSS_ACCOUNT_PASS_ROLE_MESSAGE),
            },
            Row {
                case: format!("CreateCluster naming a version the catalog does not hold, {form}"),
                uri: "/v1/clusters",
                body: v1_body(
                    &format!("unknown-version-{}", form.len()),
                    UNKNOWN_KAFKA_VERSION,
                    &unknown_subnets,
                ),
                status: 400,
                code: "BadRequestException",
                parameter: "kafkaVersion",
                // The opening only: the rest of MSK's sentence recommends the versions
                // it offers, which this suite does not pin.
                sentence: Sentence::Opening(format!(
                    "Standard instance types are not supported for Kafka version \
                     {UNKNOWN_KAFKA_VERSION}."
                )),
            },
            Row {
                case: format!("CreateReplicator naming one Kafka cluster, {form}"),
                uri: "/replication/v1/replicators",
                body: replicator_body(
                    &format!("one-cluster-{}", form.len()),
                    &[source.as_str()],
                    ROLE_ARN,
                    &unknown_subnets,
                ),
                status: 400,
                code: "BadRequestException",
                parameter: "kafkaClusters",
                sentence: Sentence::Whole(KAFKA_CLUSTER_COUNT_MESSAGE),
            },
        ];

        for row in rows {
            let case = &row.case;
            let reply = kafka(&router, "POST", row.uri, Some(row.body)).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (row.status, row.code),
                "case {case:?}: expected HTTP {} {}, got {} with {}",
                row.status,
                row.code,
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.invalid_parameter(),
                row.parameter,
                "case {case:?}: expected the refusal to name {:?} at fault, got {}",
                row.parameter,
                reply.body
            );
            match &row.sentence {
                Sentence::Whole(expected) => assert_eq!(
                    reply.message(),
                    *expected,
                    "case {case:?}: expected the sentence {expected:?}, got {:?}",
                    reply.message()
                ),
                Sentence::Opening(opening) => assert!(
                    reply.message().starts_with(opening.as_str()),
                    "case {case:?}: expected a sentence opening {opening:?}, got {:?}",
                    reply.message()
                ),
            }
            // The placement refusal is what the ordering keeps out of the answer, so no
            // row may carry it — nor the id EC2 holds no subnet for.
            assert_ne!(
                reply.message(),
                SUBNETS_ABSENT_MESSAGE,
                "case {case:?}: the placement refusal must not pre-empt MSK's own, got {}",
                reply.body
            );
            assert!(
                !reply.message().contains(unknown),
                "case {case:?}: the refusal must not name {unknown:?}, got {}",
                reply.body
            );
        }
    }

    // And every refused create registered nothing: the two names it conflicted with are
    // still carried by the ONE cluster and the ONE replicator that took them.
    let names = listed_cluster_names(&router).await;
    assert_eq!(
        names.iter().filter(|name| *name == "taken-cluster").count(),
        1,
        "case the registry after the refusals: expected one cluster named taken-cluster, \
         got {names:?}"
    );
    let replicators = listed_replicator_names(&router).await;
    assert_eq!(
        replicators,
        vec!["taken-replicator".to_string()],
        "case the registry after the refusals: expected only the replicator that took the \
         name, got {replicators:?}"
    );
}

/// RULE (the other half): a create MSK has nothing of its OWN to refuse is the one the
/// subnet resolution answers — the same requests, under names the Region does not hold,
/// are refused `BadRequestException` carrying MSK's fixed placement sentence, and the same
/// requests again over EC2's own subnets are accepted.
#[tokio::test]
async fn a_create_msk_does_not_refuse_itself_is_answered_by_the_resolution() {
    let router = emulator();
    let subnets = ec2_subnets(&router).await;
    let source = create_v1_ok(
        &router,
        v1_body("control-source", KAFKA_VERSION, &subnets),
        "the replicator's source cluster",
    )
    .await;
    let target = create_v1_ok(
        &router,
        v1_body("control-target", KAFKA_VERSION, &subnets),
        "the replicator's target cluster",
    )
    .await;
    let ends = [source.as_str(), target.as_str()];

    for (index, (form, unknown)) in unresolvable_subnet_ids().into_iter().enumerate() {
        let unknown_subnets = vec![subnets[0].clone(), unknown.to_string()];
        // (case, uri, the body naming the unresolvable placement, the member the refusal
        // names, the body naming EC2's own subnets)
        let rows: Vec<(String, &str, Value, &str, Value)> = vec![
            (
                format!("CreateCluster, {form}"),
                "/v1/clusters",
                v1_body(&format!("free-v1-{index}"), KAFKA_VERSION, &unknown_subnets),
                "brokerNodeGroupInfo",
                v1_body(&format!("held-v1-{index}"), KAFKA_VERSION, &subnets),
            ),
            (
                format!("CreateClusterV2 (provisioned), {form}"),
                "/api/v2/clusters",
                v2_provisioned_body(&format!("free-v2-{index}"), &unknown_subnets),
                "brokerNodeGroupInfo",
                v2_provisioned_body(&format!("held-v2-{index}"), &subnets),
            ),
            (
                format!("CreateClusterV2 (serverless), {form}"),
                "/api/v2/clusters",
                v2_serverless_body(&format!("free-serverless-{index}"), &unknown_subnets),
                "vpcConfigs",
                v2_serverless_body(&format!("held-serverless-{index}"), &subnets),
            ),
            (
                format!("CreateReplicator, {form}"),
                "/replication/v1/replicators",
                replicator_body(
                    &format!("free-replicator-{index}"),
                    &ends,
                    ROLE_ARN,
                    &unknown_subnets,
                ),
                "subnetIds",
                replicator_body(
                    &format!("held-replicator-{index}"),
                    &ends,
                    ROLE_ARN,
                    &subnets,
                ),
            ),
        ];

        for (case, uri, unresolvable, parameter, resolvable) in rows {
            let refused = kafka(&router, "POST", uri, Some(unresolvable)).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, "BadRequestException"),
                "case {case:?}: expected a 400 BadRequestException, got {} with {}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.invalid_parameter(),
                parameter,
                "case {case:?}: expected the refusal to name {parameter:?} at fault, got {}",
                refused.body
            );
            assert_eq!(
                refused.message(),
                SUBNETS_ABSENT_MESSAGE,
                "case {case:?}: expected MSK's placement sentence, got {:?}",
                refused.message()
            );

            let accepted = kafka(&router, "POST", uri, Some(resolvable)).await;
            assert_eq!(
                (accepted.status, accepted.error_code()),
                (200, ""),
                "case {case:?} over EC2's own subnets: expected the create to be accepted, \
                 got {} with {}",
                accepted.status,
                accepted.body
            );
        }
    }
}

/// Every cluster name `ListClustersV2` reports, over every page.
async fn listed_cluster_names(router: &MultiServiceRouter) -> Vec<String> {
    let mut names = Vec::new();
    let mut uri = "/api/v2/clusters".to_string();
    loop {
        let reply = kafka(router, "GET", &uri, None).await;
        assert_eq!(
            reply.status, 200,
            "a cluster listing must answer 200, got {} with {}",
            reply.status, reply.body
        );
        for cluster in reply
            .body
            .get("clusterInfoList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(name) = cluster.get("clusterName").and_then(Value::as_str) {
                names.push(name.to_string());
            }
        }
        match reply.body.get("nextToken").and_then(Value::as_str) {
            Some(token) => uri = format!("/api/v2/clusters?nextToken={}", form_encode(token)),
            None => break,
        }
    }
    names
}

/// Every replicator name `ListReplicators` reports, over every page.
async fn listed_replicator_names(router: &MultiServiceRouter) -> Vec<String> {
    let mut names = Vec::new();
    let mut uri = "/replication/v1/replicators".to_string();
    loop {
        let reply = kafka(router, "GET", &uri, None).await;
        assert_eq!(
            reply.status, 200,
            "a replicator listing must answer 200, got {} with {}",
            reply.status, reply.body
        );
        for replicator in reply
            .body
            .get("replicators")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(name) = replicator.get("replicatorName").and_then(Value::as_str) {
                names.push(name.to_string());
            }
        }
        match reply.body.get("nextToken").and_then(Value::as_str) {
            Some(token) => {
                uri = format!(
                    "/replication/v1/replicators?nextToken={}",
                    form_encode(token)
                )
            }
            None => break,
        }
    }
    names
}

/// The account every role above is read against is the one this emulator serves, so the
/// role the rows pass is of that account and the refused one is not — pinned here, since
/// the pass-role decision rests on exactly that difference.
#[test]
fn the_passed_role_belongs_to_this_emulators_account() {
    assert!(
        ROLE_ARN.contains(&format!(":{ACCOUNT}:")),
        "the role a row passes must belong to {ACCOUNT}, got {ROLE_ARN:?}"
    );
    assert!(
        !FOREIGN_ROLE_ARN.contains(&format!(":{ACCOUNT}:")),
        "the refused role must belong to another account, got {FOREIGN_ROLE_ARN:?}"
    );
}
