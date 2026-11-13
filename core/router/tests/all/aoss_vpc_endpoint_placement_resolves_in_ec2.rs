// SPDX-License-Identifier: CC0-1.0

//! OpenSearch Serverless resolves the NETWORK a managed interface endpoint is placed in
//! against Amazon EC2, which owns it.
//!
//! RULE: a VPC and a subnet are Amazon EC2's resources and not this service's, so the
//! `vpcId` and every `subnetIds` entry a `CreateVpcEndpoint` names — and every subnet an
//! `UpdateVpcEndpoint` adds — is resolved against the account's EC2 inventory BEFORE an
//! endpoint id is minted or anything is stored: a request naming an id EC2 holds no VPC or
//! subnet for is refused with the modelled `ValidationException` (HTTP 400) naming that id,
//! and writes nothing, so no later `ListVpcEndpoints` or `BatchGetVpcEndpoint` reports an
//! endpoint in a network that does not exist; a request naming EC2's own VPC and subnets is
//! accepted and is the endpoint those readers then report.
//!
//! This suite lives beside the router because the network is EC2's: the VPC and the subnets
//! below are minted by the real EC2 operations on the SAME engine that serves OpenSearch
//! Serverless, so every id in an accepted request is one EC2 handed out rather than one this
//! suite made up, and every refusal is grounded in what EC2 actually answered.
//!
//! The rows differ along what the rule generalises over: which member names the
//! unresolvable id (`vpcId`, `subnetIds`, `addSubnetIds`), the FORM of that value (EC2's
//! 17-hex id form and its older 8-hex form, which `SubnetId`'s own
//! `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` admits), WHERE in a list it sits, and how many
//! subnets the request names.
//!
//! Sources: `smithy_query(query="operation", operation="CreateVpcEndpoint,UpdateVpcEndpoint")`
//! on the opensearchserverless model — both model `ValidationException`, and this service
//! answers every modelled client error HTTP 400 over `awsJson1_0`;
//! `smithy_query(query="shape", shape="SubnetIds")` for the member's own id pattern.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::ComAmazonawsOpensearchserverlessOpensearchserverlessState;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The `X-Amz-Target` prefix of this service's `awsJson1_0` operations.
const TARGET_PREFIX: &str = "OpenSearchServerless";

/// The HTTP status this service answers every modelled client error with (`awsJson1_0`
/// discriminates by the body's `__type`, not by the status).
const CLIENT_ERROR: u16 = 400;

/// The status a created endpoint settles into, and the one an update requires.
const ACTIVE: &str = "ACTIVE";

/// How long one poll waits before looking again. The `PENDING -> ACTIVE` move is owed by
/// the clock, so the loop below waits for the CONDITION under a generous deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How many times a poll looks before giving up — a generous deadline for a transition the
/// service owes in well under a second.
const POLL_ATTEMPTS: usize = 200;

/// One emulator carrying Amazon EC2 and OpenSearch Serverless over ONE shared engine, as
/// the production router mounts them, plus the engine itself so a case can assert on what a
/// refused create did NOT write.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_opensearchserverless_opensearchserverless::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
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
    /// The bare shape name of the awsJson error code, or `""` on success.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The error envelope's message, or `""` when there is none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Call one OpenSearch Serverless operation over its `awsJson1_0` wire.
async fn aoss(router: &MultiServiceRouter, operation: &str, body: Value) -> Reply {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("aoss.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization("aoss"))
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
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

/// One VPC of the account with the subnets EC2 minted in it.
struct Vpc {
    id: String,
    subnets: Vec<String>,
}

/// Create one VPC carrying one subnet per entry of `zones`, through EC2's own operations.
async fn ec2_vpc(router: &MultiServiceRouter, second_octet: u8, zones: &[&str]) -> Vpc {
    let cidr = format!("10.{second_octet}.0.0/16");
    let id = element(&ec2(router, "CreateVpc", &[("CidrBlock", &cidr)]).await, "vpcId");
    let mut subnets = Vec::new();
    for (index, zone) in zones.iter().enumerate() {
        let subnet_cidr = format!("10.{second_octet}.{}.0/24", index + 1);
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", id.as_str()),
                    ("CidrBlock", subnet_cidr.as_str()),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    Vpc { id, subnets }
}

/// A `CreateVpcEndpoint` body naming `vpc_id` and `subnets`.
fn create_body(name: &str, vpc_id: &str, subnets: &[String]) -> Value {
    json!({ "name": name, "vpcId": vpc_id, "subnetIds": subnets })
}

/// How many managed VPC endpoints the account holds, read off the store this service writes
/// into — so a refused create is checked against the state itself, not only against a
/// listing.
fn stored_endpoints(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<ComAmazonawsOpensearchserverlessOpensearchserverlessState>()
        .lock()
        .vpc_endpoints
        .len()
}

/// The names `ListVpcEndpoints` reports.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let listing = aoss(router, "ListVpcEndpoints", json!({})).await;
    assert_eq!(
        listing.status, 200,
        "ListVpcEndpoints must be served, got {} with {}",
        listing.status, listing.body
    );
    listing
        .body
        .get("vpcEndpointSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    summary
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Poll `BatchGetVpcEndpoint` until `id` reports `ACTIVE`, answering its detail. The move
/// out of `PENDING` is owed by the clock, so this waits for that CONDITION.
async fn await_active(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    for _ in 0..POLL_ATTEMPTS {
        let read = aoss(router, "BatchGetVpcEndpoint", json!({ "ids": [id] })).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: BatchGetVpcEndpoint({id}) must be served, got {} with {}",
            read.status, read.body
        );
        let detail = read
            .body
            .get("vpcEndpointDetails")
            .and_then(Value::as_array)
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        if detail.get("status").and_then(Value::as_str) == Some(ACTIVE) {
            return detail;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the endpoint {id} never reached {ACTIVE}");
}

/// Values that name no VPC of the account, in both of EC2's published id forms (`VpcId`'s
/// own `^vpc-[0-9a-z]*$` admits either).
fn unresolvable_vpc_ids() -> Vec<(&'static str, &'static str)> {
    vec![
        ("EC2's 17-hex id form", "vpc-0f1e2d3c4b5a69780"),
        ("EC2's older 8-hex id form", "vpc-1a2b3c4d"),
        ("the all-zero id form", "vpc-00000000000000000"),
    ]
}

/// Values that name no subnet of the account, in both forms `SubnetId`'s own
/// `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` admits.
fn unresolvable_subnet_ids() -> Vec<(&'static str, &'static str)> {
    vec![
        ("EC2's 17-hex id form", "subnet-0f1e2d3c4b5a69780"),
        ("EC2's older 8-hex id form", "subnet-1a2b3c4d"),
        ("the all-zero id form", "subnet-00000000000000000"),
    ]
}

#[tokio::test]
async fn a_vpc_endpoint_is_placed_only_in_a_network_ec2_holds() {
    let (router, engine) = emulator();
    let first = ec2_vpc(&router, 90, &["us-west-2a", "us-west-2b", "us-west-2c"]).await;
    let second = ec2_vpc(&router, 91, &["us-west-2a"]).await;

    // The accepted side of the rule, over subnet counts and over two of EC2's own VPCs:
    // each create names a VPC and subnets EC2 itself minted, so each is accepted and is
    // what the readers report.
    let mut accepted = 0usize;
    let mut live: Vec<(String, String)> = Vec::new();
    for (case, vpc, subnets) in [
        (
            "one of EC2's subnets",
            &first,
            first.subnets[..1].to_vec(),
        ),
        (
            "two of EC2's subnets",
            &first,
            first.subnets[..2].to_vec(),
        ),
        (
            "all three subnets of the first VPC",
            &first,
            first.subnets.clone(),
        ),
        (
            "the only subnet of a second VPC",
            &second,
            second.subnets.clone(),
        ),
    ] {
        let name = format!("resolved-endpoint-{accepted}");
        let created = aoss(
            &router,
            "CreateVpcEndpoint",
            create_body(&name, &vpc.id, &subnets),
        )
        .await;
        assert_eq!(
            (created.status, created.error_code().as_str()),
            (200, ""),
            "case {case:?}: a create placed in EC2's own network must be accepted, got {} with {}",
            created.status,
            created.body
        );
        let id = created
            .body
            .get("createVpcEndpointDetail")
            .and_then(|detail| detail.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the create reported no endpoint id: {}",
                    created.body
                )
            })
            .to_string();
        accepted += 1;
        assert_eq!(
            stored_endpoints(&engine),
            accepted,
            "case {case:?}: expected the account to hold {accepted} endpoint(s)"
        );

        // The round trip: the reader reports the network the endpoint was placed in.
        let detail = await_active(&router, case, &id).await;
        assert_eq!(
            detail.get("vpcId").and_then(Value::as_str),
            Some(vpc.id.as_str()),
            "case {case:?}: expected the read to report the VPC EC2 minted, got {detail}"
        );
        assert_eq!(
            detail.get("subnetIds").cloned().unwrap_or(Value::Null),
            json!(subnets),
            "case {case:?}: expected the read to report the subnets the endpoint was placed in, \
             got {detail}"
        );
        live.push((name, id));
    }

    // The refusal side. One row per way the network fails to exist: a `vpcId` EC2 holds no
    // VPC for (in each published form), and a `subnetIds` entry it holds no subnet for,
    // alone, last of two and first of three.
    //
    // (case, the vpcId, the subnets, the id the refusal must name)
    let mut rows: Vec<(String, String, Vec<String>, String)> = Vec::new();
    for (form, unknown) in unresolvable_vpc_ids() {
        rows.push((
            format!("a vpcId EC2 does not hold, in {form}"),
            unknown.to_string(),
            first.subnets[..1].to_vec(),
            unknown.to_string(),
        ));
    }
    for (form, unknown) in unresolvable_subnet_ids() {
        rows.push((
            format!("one subnet EC2 does not hold, in {form}"),
            first.id.clone(),
            vec![unknown.to_string()],
            unknown.to_string(),
        ));
        rows.push((
            format!("one subnet EC2 holds and then one it does not, in {form}"),
            first.id.clone(),
            vec![first.subnets[0].clone(), unknown.to_string()],
            unknown.to_string(),
        ));
        rows.push((
            format!("the FIRST of three subnets EC2 does not hold, in {form}"),
            first.id.clone(),
            vec![
                unknown.to_string(),
                first.subnets[0].clone(),
                first.subnets[1].clone(),
            ],
            unknown.to_string(),
        ));
    }

    for (index, (case, vpc_id, subnets, named)) in rows.iter().enumerate() {
        let name = format!("phantom-endpoint-{index}");
        let refused = aoss(
            &router,
            "CreateVpcEndpoint",
            create_body(&name, vpc_id, subnets),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case:?}: expected {CLIENT_ERROR} ValidationException, got {} with {}",
            refused.status,
            refused.body
        );
        assert!(
            refused.message().contains(named.as_str()),
            "case {case:?}: expected the refusal to name {named:?}, got {}",
            refused.body
        );
        // Nothing was created: a refused create leaves the account exactly as it was, so no
        // reader reports an endpoint in a network that does not exist.
        assert_eq!(
            stored_endpoints(&engine),
            accepted,
            "case {case:?}: a refused CreateVpcEndpoint must write no endpoint"
        );
        let names = listed_names(&router).await;
        assert!(
            !names.contains(&name),
            "case {case:?}: a refused create must leave no listed endpoint named {name:?}, got \
             {names:?}"
        );
    }

    // Every accepted endpoint is still listed, and only those: the refusals neither created
    // nor removed anything.
    let names = listed_names(&router).await;
    let mut expected: Vec<String> = live.iter().map(|(name, _)| name.clone()).collect();
    expected.sort();
    let mut listed = names.clone();
    listed.sort();
    assert_eq!(
        listed, expected,
        "case \"after every refusal\": expected exactly the accepted endpoints to be listed"
    );
}

#[tokio::test]
async fn an_update_adds_only_subnets_ec2_holds() {
    let (router, engine) = emulator();
    let vpc = ec2_vpc(&router, 92, &["us-west-2a", "us-west-2b"]).await;

    let created = aoss(
        &router,
        "CreateVpcEndpoint",
        create_body("updated-endpoint", &vpc.id, &vpc.subnets[..1]),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: the create must be accepted, got {} with {}",
        created.status, created.body
    );
    let id = created
        .body
        .get("createVpcEndpointDetail")
        .and_then(|detail| detail.get("id"))
        .and_then(Value::as_str)
        .expect("setup: the create reports an endpoint id")
        .to_string();
    await_active(&router, "setup", &id).await;

    // A subnet EC2 does not hold cannot be added, whichever form the value takes, and the
    // refused update leaves the endpoint's own subnets as they were.
    for (form, unknown) in unresolvable_subnet_ids() {
        let case = format!("addSubnetIds naming a subnet EC2 does not hold, in {form}");
        let refused = aoss(
            &router,
            "UpdateVpcEndpoint",
            json!({ "id": id, "addSubnetIds": [unknown] }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case:?}: expected {CLIENT_ERROR} ValidationException, got {} with {}",
            refused.status,
            refused.body
        );
        assert!(
            refused.message().contains(unknown),
            "case {case:?}: expected the refusal to name {unknown:?}, got {}",
            refused.body
        );
        let detail = await_active(&router, &case, &id).await;
        assert_eq!(
            detail.get("subnetIds").cloned().unwrap_or(Value::Null),
            json!(vpc.subnets[..1].to_vec()),
            "case {case:?}: a refused update must leave the endpoint's subnets alone, got {detail}"
        );
    }

    // And the subnet EC2 DOES hold is added, which is what the reader then reports.
    let updated = aoss(
        &router,
        "UpdateVpcEndpoint",
        json!({ "id": id, "addSubnetIds": [vpc.subnets[1].clone()] }),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case \"addSubnetIds naming EC2's own subnet\": expected 200, got {} with {}",
        updated.status, updated.body
    );
    let detail = await_active(&router, "after the accepted update", &id).await;
    let mut reported: Vec<String> = detail
        .get("subnetIds")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    let mut expected = vpc.subnets.clone();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case \"addSubnetIds naming EC2's own subnet\": expected both of EC2's subnets to be \
         reported, got {detail}"
    );
    assert_eq!(
        stored_endpoints(&engine),
        1,
        "case \"after the accepted update\": expected the account to still hold one endpoint"
    );
}
