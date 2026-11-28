// SPDX-License-Identifier: CC0-1.0

//! SageMaker resolves the EBS VOLUME a HyperPod node attachment names against Amazon EC2,
//! which owns it.
//!
//! RULE: an EBS volume is Amazon EC2's resource and not SageMaker's, so the `VolumeId` an
//! `AttachClusterNodeVolume` names is resolved against the account's EC2 inventory BEFORE the
//! cluster the request addresses — a request naming an identifier EC2 answered it holds no
//! volume for fails this operation's documented precondition on the volume with the 400
//! `ValidationException` a bad request parameter gets, and the message relays what EC2
//! answered: SageMaker's own fixed opening, the EC2 error code and EC2's own sentence. A
//! request naming a volume EC2 DOES hold is served, and the attachment it answers is the one
//! the HyperPod node then holds.
//!
//! This suite lives beside the router because the volume is EC2's: the volume below is minted
//! by the real `ec2:CreateVolume` on the SAME engine that serves SageMaker, so the accepted
//! request names an identifier EC2 handed out rather than one this suite made up, and every
//! refusal is grounded in what EC2 actually answered — the expected sentence is read off EC2's
//! own refusal of the same identifier rather than written out here.
//!
//! The rows differ along what the rule generalises over: the FORM of the identifier EC2 holds
//! no volume for (its short 8-hex id form, which EC2 reads and finds nothing for, and a
//! long-form id EC2 never issued, which it refuses as a parameter it cannot read), and whether
//! the cluster the same request addresses exists — which is what shows the volume is resolved
//! first.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// SageMaker's own fixed opening for an answer it relays from EC2, up to the error code.
const RELAY_OPENING: &str = "HyperPod - EC2ErrCode:";

/// What separates the relayed EC2 error code from the sentence EC2 worded.
const RELAY_SEPARATOR: &str = " : ";

/// A deadline far longer than any span the SageMaker state gives a transient status, so a
/// loaded runner never decides whether a poll loop terminates.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One emulator carrying Amazon EC2 and SageMaker over ONE shared engine, as the production
/// router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is part
/// of what routes the request here; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One ec2Query action, answered with its HTTP status and XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
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
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One SageMaker operation over its awsJson1_1 wire, answered with its status and JSON body.
async fn sagemaker(router: &MultiServiceRouter, target: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("api.sagemaker.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("SageMaker.{target}"))
        .header("authorization", authorization("sagemaker"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// A SageMaker call that must be served, answered with its output.
async fn served(router: &MultiServiceRouter, target: &str, body: &Value) -> Value {
    let (status, answered) = sagemaker(router, target, body).await;
    assert_eq!(
        status, 200,
        "setup: SageMaker {target} must be served, got {status} with {answered}"
    );
    answered
}

/// The error code of a SageMaker awsJson1_1 error body, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The message of a SageMaker error body.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A string member of a JSON answer.
fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

/// How EC2 refuses a `DescribeVolumes` of `volume_id`, as `(error code, its own sentence)` —
/// the two parts a relayed answer carries, read off the OWNER's refusal rather than written
/// out here.
async fn ec2_volume_refusal(router: &MultiServiceRouter, volume_id: &str) -> (String, String) {
    let (status, body) = ec2(router, "DescribeVolumes", &[("VolumeId.1", volume_id)]).await;
    assert_eq!(
        status, 400,
        "setup: EC2 must refuse the volume {volume_id}, got {status} with {body}"
    );
    let code = element(&body, "Code");
    let sentence = elements(&body, "Message")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no <Message> element in {body}"));
    (code, sentence)
}

/// Create the cluster `name` with one instance group and LOOK AGAIN until the status the clock
/// owes it has been materialised, so the attach is past the documented `InService`
/// precondition. Answers its `ClusterArn`.
async fn in_service_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let created = served(
        router,
        "CreateCluster",
        &json!({
            "ClusterName": name,
            "InstanceGroups": [{
                "InstanceGroupName": "workers",
                "InstanceType": "ml.m5.large",
                "InstanceCount": 0,
                "ExecutionRole": format!(
                    "arn:aws:iam::{}:role/hyperpod-execution",
                    engine::EMULATOR_ACCOUNT_ID
                ),
            }],
            // Orchestrated by Amazon EKS, which is the other half of the precondition the
            // attach documents on `ClusterArn`: "Your cluster must use EKS as the
            // orchestration and be in the InService state" (smithy_query query="doc"
            // shape=AttachClusterNodeVolumeRequest member=ClusterArn).
            "Orchestrator": {
                "Eks": {
                    "ClusterArn": format!(
                        "arn:aws:eks:{REGION}:{}:cluster/hyperpod",
                        engine::EMULATOR_ACCOUNT_ID
                    ),
                },
            },
        }),
    )
    .await;
    let arn = str_field(&created, "ClusterArn");
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = served(router, "DescribeCluster", &json!({"ClusterName": name})).await;
        let status = str_field(&described, "ClusterStatus");
        if status == "InService" {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the cluster {name} never reached InService, last {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Add one node to `cluster` and LOOK AGAIN until the instance id the transition mints is
/// reported, so the attach addresses a node the cluster really owns. Answers that id.
async fn running_node(router: &MultiServiceRouter, cluster: &str) -> String {
    let added = served(
        router,
        "BatchAddClusterNodes",
        &json!({
            "ClusterName": cluster,
            "NodesToAdd": [{"InstanceGroupName": "workers", "IncrementTargetCountBy": 1}],
        }),
    )
    .await;
    let logical_id = added
        .get("Successful")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .map(|entry| str_field(entry, "NodeLogicalId"))
        .unwrap_or_else(|| panic!("setup: the add answered no node, got {added}"));

    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = served(
            router,
            "DescribeClusterNode",
            &json!({"ClusterName": cluster, "NodeLogicalId": logical_id}),
        )
        .await;
        let details = described.get("NodeDetails").cloned().unwrap_or(Value::Null);
        if let Some(instance_id) = details.get("InstanceId").and_then(Value::as_str) {
            return instance_id.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "setup: the node never reported an instance id, last {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_node_volume_attachment_resolves_its_volume_in_ec2() {
    let router = emulator();
    let cluster = "hyperpod-volumes";
    let cluster_arn = in_service_cluster(&router, cluster).await;
    let node = running_node(&router, cluster).await;

    // The volume is EC2's own: the id below is the one `ec2:CreateVolume` handed out.
    let (status, created) = ec2(
        &router,
        "CreateVolume",
        &[("AvailabilityZone", "us-west-2a"), ("Size", "1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: EC2 CreateVolume must be served, got {status} with {created}"
    );
    let volume = element(&created, "volumeId");

    // A volume EC2 holds is attached, and the answer names it.
    let attached = served(
        &router,
        "AttachClusterNodeVolume",
        &json!({"ClusterArn": cluster_arn, "NodeId": node, "VolumeId": volume}),
    )
    .await;
    assert_eq!(
        str_field(&attached, "VolumeId"),
        volume,
        "case a volume EC2 holds: the answer must name the volume, got {attached}"
    );
    assert_eq!(
        str_field(&attached, "NodeId"),
        node,
        "case a volume EC2 holds: the answer must name the node, got {attached}"
    );

    // Rows: the two forms of an identifier EC2 holds no volume for, each with the cluster the
    // request addresses existing and not existing — the volume is resolved first, so the
    // answer is the same either way.
    // `VolumeId` is `^vol-[a-f0-9]{8}(?:[a-f0-9]{9})?$` (smithy_query query="operation"
    // operation="AttachClusterNodeVolume"), so both the 8- and the 17-digit forms are tested.
    let absent_cluster_arn = format!(
        "arn:aws:sagemaker:{REGION}:{}:cluster/abcdef123456",
        engine::EMULATOR_ACCOUNT_ID
    );
    let unknown_volumes = ["vol-aabbccdd", "vol-0123456789abcdef0"];
    for unknown in unknown_volumes {
        let (code, sentence) = ec2_volume_refusal(&router, unknown).await;
        let expected = format!("{RELAY_OPENING} {code}{RELAY_SEPARATOR}{sentence}");
        for (case, addressed) in [
            ("the cluster of the request exists", cluster_arn.as_str()),
            (
                "the cluster of the request does not",
                absent_cluster_arn.as_str(),
            ),
        ] {
            let case = format!("{unknown} while {case}");
            let (status, refused) = sagemaker(
                &router,
                "AttachClusterNodeVolume",
                &json!({"ClusterArn": addressed, "NodeId": node, "VolumeId": unknown}),
            )
            .await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ValidationException"),
                "case {case}: expected 400 ValidationException, got {status} with {refused}"
            );
            let reported = message(&refused);
            assert_eq!(
                reported, expected,
                "case {case}: expected {expected:?}, got {reported:?}"
            );
        }
    }

    // The refused attaches changed nothing: the node still holds the one volume EC2 handed
    // out, and attaching it again answers that same attachment rather than a second one.
    let again = served(
        &router,
        "AttachClusterNodeVolume",
        &json!({"ClusterArn": cluster_arn, "NodeId": node, "VolumeId": volume}),
    )
    .await;
    assert_eq!(
        str_field(&again, "DeviceName"),
        str_field(&attached, "DeviceName"),
        "case a re-attach after the refusals: expected the device the first attach gave, got \
         {again}"
    );
}
